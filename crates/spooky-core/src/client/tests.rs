use super::ssh::{posix_quote, validate_destination};
use super::upload::{
    SshTools, prepared_upload_directory, sftp_quote, transfer_timeout, upload_ssh,
    validate_remote_absolute_path,
};
use super::{Client, is_unrecoverable, was_cancelled};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Scripts that stand in for `ssh` are written and run one test at a time, so
/// no other test's `fork` can hold a script's file open while it executes.
static FAKE_SSH: Mutex<()> = Mutex::new(());

/// Writes an executable `ssh` replacement running `body`; returns its directory and path.
fn fake_ssh(body: &str) -> (PathBuf, PathBuf) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spooky-fake-ssh-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).expect("create fake ssh directory");
    let script = dir.join("ssh");
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write fake ssh");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("make it executable");
    (dir, script)
}

/// The error from connecting through `ssh`, which must fail.
fn failure(ssh: &OsStr, cancelled: &dyn Fn() -> bool) -> anyhow::Error {
    match Client::connect_ssh(ssh, "user@host", cancelled) {
        Ok(_) => panic!("the fake ssh cannot complete the handshake"),
        Err(error) => error,
    }
}

#[test]
fn validates_ssh_destinations_without_interpreting_shell_syntax() {
    assert!(validate_destination("alice@host.example").is_ok());
    assert!(validate_destination("[2001:db8::1]").is_ok());
    assert!(validate_destination("alice@[2001:db8::1]").is_ok());
    assert!(validate_destination("-oProxyCommand=evil").is_err());
    assert!(validate_destination("host name").is_err());
    assert!(validate_destination("host;command").is_err());
}

#[test]
fn quotes_sftp_paths_and_validates_remote_absolute_paths() {
    assert!(matches!(sftp_quote("/tmp/a b\"c"), Ok(value) if value == "\"/tmp/a b\\\"c\""));
    assert!(matches!(sftp_quote("/tmp/a\\b"), Ok(value) if value == "\"/tmp/a\\\\b\""));
    assert!(validate_remote_absolute_path("/home/user/.spooky-upload.abc123/file", "path").is_ok());
    assert!(validate_remote_absolute_path("/home/user/../other", "path").is_err());
}

#[test]
fn a_hung_ssh_connection_is_abandoned_promptly_when_cancelled() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (dir, ssh) = fake_ssh("exec sleep 30");
    let cancel = Arc::new(AtomicBool::new(false));
    let canceller = {
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::Release);
        })
    };
    let started = Instant::now();
    let error = failure(ssh.as_os_str(), &|| cancel.load(Ordering::Acquire));
    let waited = started.elapsed();
    canceller.join().expect("canceller thread");
    let _ = fs::remove_dir_all(dir);
    assert!(was_cancelled(&error), "{error:#}");
    assert!(
        waited < Duration::from_secs(5),
        "cancelling took {waited:?} against a 30 s hang"
    );
}

#[test]
fn a_rejected_login_is_unrecoverable_and_says_what_to_do() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (dir, ssh) =
        fake_ssh("echo 'user@host: Permission denied (publickey,password).' >&2\nexit 255");
    let error = failure(ssh.as_os_str(), &|| false);
    let _ = fs::remove_dir_all(dir);
    assert!(is_unrecoverable(&error), "{error:#}");
    let text = error.to_string();
    assert!(text.contains("Permission denied"), "{text}");
    assert!(text.contains("authorize a key"), "{text}");
}

#[test]
fn a_refused_connection_is_worth_retrying() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (dir, ssh) =
        fake_ssh("echo 'ssh: connect to host h port 22: Connection refused' >&2\nexit 255");
    let error = failure(ssh.as_os_str(), &|| false);
    let _ = fs::remove_dir_all(dir);
    assert!(!is_unrecoverable(&error), "{error:#}");
    assert!(!was_cancelled(&error), "{error:#}");
    assert!(error.to_string().contains("Connection refused"), "{error}");
}

/// A fake remote account. Its `ssh` runs the requested command locally with
/// `HOME` set to a private directory; its `sftp` runs `sftp_body`.
struct FakeRemote {
    root: PathBuf,
    home: PathBuf,
    ssh: PathBuf,
    sftp: PathBuf,
}

impl Drop for FakeRemote {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Copies the file named by the batch's `put` to its destination.
const COPYING_SFTP: &str = "IFS= read -r line\neval \"set -- $line\"\ncp \"$2\" \"$3\"";

fn fake_remote(sftp_body: &str) -> FakeRemote {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spooky-fake-remote-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let home = root.join("home");
    fs::create_dir_all(&home).expect("create fake remote home");
    let write_script = |name: &str, body: &str| {
        let script = root.join(name);
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).expect("write fake script");
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make it executable");
        script
    };
    let ssh = write_script(
        "ssh",
        &format!(
            "HOME='{}'; export HOME\nfor last in \"$@\"; do :; done\nexec sh -c \"$last\"",
            home.display()
        ),
    );
    let sftp = write_script("sftp", sftp_body);
    FakeRemote {
        root,
        home,
        ssh,
        sftp,
    }
}

fn upload_through(
    remote: &FakeRemote,
    source: &Path,
    cancelled: &dyn Fn() -> bool,
) -> anyhow::Result<String> {
    let size = fs::metadata(source).expect("source metadata").len();
    upload_ssh(
        &SshTools {
            ssh: remote.ssh.as_os_str(),
            sftp: remote.sftp.as_os_str(),
        },
        "user@host",
        source,
        size,
        cancelled,
    )
}

fn source_file(remote: &FakeRemote, name: &str, bytes: &[u8]) -> PathBuf {
    let path = remote.root.join(name);
    fs::write(&path, bytes).expect("write upload source");
    path
}

fn upload_directories(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(home)
        .expect("read fake home")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(".spooky-upload."))
        .collect();
    names.sort();
    names
}

#[test]
fn an_uploaded_file_lands_in_a_private_directory_and_is_reported_by_path() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let remote = fake_remote(COPYING_SFTP);
    let source = source_file(&remote, "shot.png", b"\x89PNG not really");
    let path = upload_through(&remote, &source, &|| false).expect("upload succeeds");
    let remote_file = PathBuf::from(&path);
    assert!(remote_file.starts_with(&remote.home), "{path}");
    assert_eq!(
        remote_file.file_name().and_then(|n| n.to_str()),
        Some("attachment.png")
    );
    assert_eq!(
        fs::read(&remote_file).expect("uploaded bytes"),
        b"\x89PNG not really"
    );
    let directory = remote_file.parent().expect("upload directory");
    let mode = fs::metadata(directory)
        .expect("directory metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "upload directory is private: {mode:o}");
}

#[test]
fn a_failed_transfer_leaves_nothing_on_the_remote_account() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let remote = fake_remote("cat >/dev/null\necho 'sftp: disk full' >&2\nexit 1");
    let source = source_file(&remote, "a.txt", b"data");
    let error = upload_through(&remote, &source, &|| false).expect_err("transfer fails");
    assert!(format!("{error:#}").contains("disk full"), "{error:#}");
    assert_eq!(upload_directories(&remote.home), Vec::<String>::new());
}

#[test]
fn a_cancelled_transfer_stops_promptly_and_cleans_up() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let remote = fake_remote("cat >/dev/null\nexec sleep 30");
    let source = source_file(&remote, "big.bin", b"data");
    let cancel = Arc::new(AtomicBool::new(false));
    let canceller = {
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(400));
            cancel.store(true, Ordering::Release);
        })
    };
    let started = Instant::now();
    let error = upload_through(&remote, &source, &|| cancel.load(Ordering::Acquire))
        .expect_err("transfer is cancelled");
    let waited = started.elapsed();
    canceller.join().expect("canceller thread");
    assert!(was_cancelled(&error), "{error:#}");
    assert!(
        waited < Duration::from_secs(5),
        "cancelling took {waited:?}"
    );
    assert_eq!(upload_directories(&remote.home), Vec::<String>::new());
}

#[test]
fn only_stale_upload_directories_are_swept() {
    let _serial = FAKE_SSH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let remote = fake_remote(COPYING_SFTP);
    let old = remote.home.join(".spooky-upload.OLD111");
    let recent = remote.home.join(".spooky-upload.NEW222");
    let unrelated = remote.home.join(".spooky-notes");
    for directory in [&old, &recent, &unrelated] {
        fs::create_dir(directory).expect("create fixture directory");
    }
    let ten_days = Duration::from_secs(10 * 24 * 3600);
    fs::File::open(&old)
        .and_then(|file| file.set_modified(std::time::SystemTime::now() - ten_days))
        .expect("age the old directory");
    fs::File::open(&unrelated)
        .and_then(|file| file.set_modified(std::time::SystemTime::now() - ten_days))
        .expect("age the unrelated directory");
    let source = source_file(&remote, "a.txt", b"data");
    upload_through(&remote, &source, &|| false).expect("upload succeeds");
    assert!(!old.exists(), "a stale upload directory is removed");
    assert!(recent.exists(), "a recent upload directory is kept");
    assert!(
        unrelated.exists(),
        "only Spooky's own directories are touched"
    );
}

#[test]
fn the_reported_upload_directory_is_only_trusted_inside_the_reported_home() {
    let good = b"/home/u\n/home/u/.spooky-upload.aB3xYz\n";
    assert_eq!(
        prepared_upload_directory(good).expect("valid output"),
        "/home/u/.spooky-upload.aB3xYz"
    );
    for bad in [
        &b"/home/u\n/tmp/.spooky-upload.aB3xYz\n"[..],
        b"/home/u\n/home/u/.spooky-upload.a/../b\n",
        b"/home/u\n/home/u/.spooky-upload.a b\n",
        b"/home/u\n/home/u/.spooky-upload.\n",
        b"/home/u\n/home/u/.spooky-upload.abc\nextra\n",
        b"/home/u/.spooky-upload.abc\n",
    ] {
        assert!(
            prepared_upload_directory(bad).is_err(),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn a_transfer_gets_time_in_proportion_to_its_size_up_to_a_ceiling() {
    assert_eq!(transfer_timeout(0), Duration::from_secs(30));
    assert_eq!(transfer_timeout(100 * 256 * 1024), Duration::from_secs(130));
    assert_eq!(transfer_timeout(u64::MAX), Duration::from_secs(30 * 60));
}

#[test]
fn quoted_paths_reach_the_remote_shell_as_one_literal_word() {
    for text in [
        "plain",
        "it's",
        "a b",
        "$HOME `id` \"q\"",
        "x;rm -rf y",
        "'",
    ] {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", posix_quote(text)))
            .output()
            .expect("run sh");
        assert_eq!(String::from_utf8_lossy(&output.stdout), text);
    }
}
