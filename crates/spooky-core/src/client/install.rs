//! Installing `spooky-server` on a remote machine, with progress.
//!
//! The release archive is downloaded on this machine, checked against its published
//! SHA-256 and its minisign signature (see [`super::signature`]) and sent over SSH to
//! `~/.local/bin`. Nothing is fetched or built by the remote
//! account, so it needs no internet access and no toolchain. Every SSH call keeps the
//! rules of the rest of the client: no prompts, host keys verified, nothing forwarded.

use super::fetch::{Workspace, check_sha256, download, fetch_text, protocol_of};
use super::signature::{self, MAX_SIGNATURE_BYTES, TrustedKeys};
use super::ssh::{
    drain_stderr, run_ssh_command, ssh_command_with, terminate_child, validate_destination,
};
use super::{Cancelled, Client, Problem, SSH_COMMAND_TIMEOUT, remote_problem};
use anyhow::{Context, Result, bail};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// Where the official releases live; `SPOOKY_RELEASE_URL` replaces it (a mirror, or a
/// directory of release files for development).
const OFFICIAL_BASE: &str = "https://github.com/Luqueee/spooky/releases/download";
const BASE_ENV: &str = "SPOOKY_RELEASE_URL";
/// The server binary is the only thing in the archive that is wanted, and bounded.
const MAX_SERVER_BYTES: u64 = 256 * 1024 * 1024;
const MAX_SHA_FILE_BYTES: u64 = 1024;
/// A release archive holds one server binary; this bounds the download itself.
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const UPLOAD_CHUNK: usize = 64 * 1024;
/// Where on the remote account the server goes, relative to `$HOME`.
const REMOTE_DIR: &str = ".local/bin";
const REMOTE_NAME: &str = "spooky-server";

/// Which release of `spooky-server` to install.
#[derive(Clone, Debug)]
pub struct Release {
    base: String,
    version: String,
    /// The keys the archive must be signed with.
    keys: TrustedKeys,
}

/// Where releases are downloaded from: the official location, or `SPOOKY_RELEASE_URL`.
pub(crate) fn official_base() -> String {
    std::env::var(BASE_ENV)
        .ok()
        .filter(|base| !base.is_empty())
        .unwrap_or_else(|| OFFICIAL_BASE.to_owned())
}

impl Release {
    /// The release that matches this program's version, from the official location or
    /// `SPOOKY_RELEASE_URL`.
    pub fn official() -> Self {
        Self::new(official_base(), env!("CARGO_PKG_VERSION"))
    }

    /// A release of `version` under `base` (an `https://` or `file://` location).
    pub fn new(base: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            base: base.into().trim_end_matches('/').to_owned(),
            version: version.into(),
            keys: TrustedKeys::release(),
        }
    }

    fn asset_name(&self, triple: &str) -> String {
        format!("spooky-server-{}-{triple}.tar.gz", self.version)
    }

    fn asset_url(&self, triple: &str) -> String {
        format!(
            "{}/v{}/{}",
            self.base,
            self.version,
            self.asset_name(triple)
        )
    }

    /// The `curl` protocol restriction for this location: plain HTTP is never used.
    fn protocol(&self) -> Result<&'static str> {
        protocol_of(&self.base)
    }
}

/// What the installation is doing, for a progress display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Progress {
    /// A new step began; the text says what it does.
    Step(String),
    /// Bytes received of the release archive.
    Downloaded(u64),
    /// Bytes sent to the remote machine, of a known total.
    Uploaded {
        /// Bytes sent so far.
        sent: u64,
        /// Size of the file being sent.
        total: u64,
    },
}

/// How an installation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The new server answers over SSH.
    Ready,
    /// The new binary is in place, but the old server still runs there and speaks another
    /// protocol version: it must be restarted, which ends its shells and agents.
    NeedsRestart {
        /// The protocol version of the server that is running.
        running_protocol: u32,
    },
}

/// The release target triple for what `uname -sm` printed.
fn target_triple(os: &str, arch: &str) -> Result<&'static str> {
    match (os, arch) {
        ("Linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("Linux", "aarch64" | "arm64") => Ok("aarch64-unknown-linux-musl"),
        ("Darwin", "arm64") => Ok("aarch64-apple-darwin"),
        ("Darwin", "x86_64") => Ok("x86_64-apple-darwin"),
        _ => bail!("there is no release of spooky-server for {os} on {arch}"),
    }
}

/// Installs the release's `spooky-server` on `destination` and checks that it answers.
///
/// `progress` is called from this thread. `cancelled` abandons the work with
/// [`Cancelled`], leaving the remote account as it was or with a finished install.
pub fn install(
    destination: &str,
    release: &Release,
    progress: &mut dyn FnMut(Progress),
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome> {
    install_with(
        OsStr::new("ssh"),
        OsStr::new("curl"),
        destination,
        release,
        progress,
        cancelled,
    )
}

pub(super) fn install_with(
    ssh: &OsStr,
    curl: &OsStr,
    destination: &str,
    release: &Release,
    progress: &mut dyn FnMut(Progress),
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome> {
    validate_destination(destination)?;
    let protocol = release.protocol()?;

    progress(Progress::Step(format!("Checking {destination}")));
    let probe = run_ssh_command(
        ssh,
        destination,
        "uname -sm",
        SSH_COMMAND_TIMEOUT,
        cancelled,
    )
    .with_context(|| format!("reaching {destination} over SSH"))?;
    let probe = String::from_utf8_lossy(&probe);
    let mut words = probe.split_whitespace();
    let (os, arch) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
    let triple = target_triple(os, arch)?;

    let work = Workspace::create()?;
    let archive = work.path().join("spooky-server.tar.gz");
    progress(Progress::Step(format!(
        "Downloading spooky-server {} for {triple}",
        release.version
    )));
    download(
        curl,
        protocol,
        &release.asset_url(triple),
        &archive,
        MAX_ARCHIVE_BYTES,
        &mut |bytes| progress(Progress::Downloaded(bytes)),
        cancelled,
    )?;
    let published = fetch_text(
        curl,
        protocol,
        &format!("{}.sha256", release.asset_url(triple)),
        MAX_SHA_FILE_BYTES,
        cancelled,
    )?;
    progress(Progress::Step("Checking the download".into()));
    check_sha256(&archive, &published, &release.asset_name(triple))?;
    let signature = fetch_text(
        curl,
        protocol,
        &format!("{}.minisig", release.asset_url(triple)),
        MAX_SIGNATURE_BYTES,
        cancelled,
    )
    .context(
        "the release has no signature to check; a release that is not signed is not installed",
    )?;
    signature::verify(
        &release.keys,
        &archive,
        &signature,
        &release.asset_name(triple),
    )?;
    let binary = extract(&archive, work.path())?;

    progress(Progress::Step(format!("Sending it to {destination}")));
    upload(ssh, destination, &binary, progress, cancelled)?;

    progress(Progress::Step("Checking that it answers".into()));
    match Client::connect_ssh(ssh, destination, cancelled) {
        Ok(client) => {
            drop(client);
            Ok(Outcome::Ready)
        }
        Err(error) => match remote_problem(&error) {
            Some(Problem::Incompatible { server, .. }) => Ok(Outcome::NeedsRestart {
                running_protocol: server,
            }),
            _ => Err(error).context("the installed server does not answer"),
        },
    }
}

/// Ends the server running on `destination`, and with it every shell and agent it
/// holds; the next connection starts the installed one. Only call this once the person
/// has agreed.
pub fn restart_server(destination: &str, cancelled: &dyn Fn() -> bool) -> Result<()> {
    restart_server_with(OsStr::new("ssh"), destination, cancelled)
}

pub(super) fn restart_server_with(
    ssh: &OsStr,
    destination: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    validate_destination(destination)?;
    // The bracket keeps the pattern from matching the shell that runs this very line.
    let command = "pkill -u \"$(id -u)\" -f '[s]pooky-server serve'; \
         for i in 1 2 3 4 5 6 7 8 9 10; do \
         pgrep -u \"$(id -u)\" -f '[s]pooky-server serve' >/dev/null || exit 0; sleep 0.5; \
         done; echo 'the server did not stop' >&2; exit 1";
    run_ssh_command(ssh, destination, command, SSH_COMMAND_TIMEOUT, cancelled)
        .with_context(|| format!("stopping the server on {destination}"))?;
    Ok(())
}

/// Extracts `spooky-server` from the archive into `directory`.
fn extract(archive: &Path, directory: &Path) -> Result<PathBuf> {
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(directory)
        .arg(REMOTE_NAME)
        .stdin(Stdio::null())
        .output()
        .context("starting tar")?;
    if !output.status.success() {
        bail!(
            "the release archive does not hold {REMOTE_NAME}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let binary = directory.join(REMOTE_NAME);
    let metadata = fs::symlink_metadata(&binary).context("looking for the extracted server")?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_SERVER_BYTES {
        bail!("the server in the release archive is not a regular file of a sensible size");
    }
    Ok(binary)
}

/// Streams `binary` to `~/.local/bin/spooky-server` on the remote account: written to a
/// temporary name and moved into place, so a failed transfer never leaves half a server.
fn upload(
    ssh: &OsStr,
    destination: &str,
    binary: &Path,
    progress: &mut dyn FnMut(Progress),
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let total = fs::metadata(binary).context("sizing the server")?.len();
    let temporary = format!("$HOME/{REMOTE_DIR}/.{REMOTE_NAME}.upload");
    let target = format!("$HOME/{REMOTE_DIR}/{REMOTE_NAME}");
    // Only constants and `$HOME`, which the remote shell expands, reach the command.
    let remote = format!(
        "umask 077; mkdir -p \"$HOME/{REMOTE_DIR}\" && cat > \"{temporary}\" \
         && chmod 755 \"{temporary}\" && mv -f \"{temporary}\" \"{target}\" \
         || {{ rm -f \"{temporary}\"; exit 1; }}"
    );
    let mut command = ssh_command_with(ssh, destination);
    command
        .arg(remote)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("starting SSH for the upload")?;
    let mut stdin = child.stdin.take().context("opening the upload stream")?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let reader = drain_stderr(
        child.stderr.take().context("opening SSH diagnostics")?,
        Arc::clone(&diagnostics),
    )?;
    let mut file = File::open(binary).context("opening the server")?;
    let mut buffer = vec![0_u8; UPLOAD_CHUNK];
    let deadline = Instant::now() + UPLOAD_TIMEOUT;
    let mut sent = 0_u64;
    let mut failure: Option<anyhow::Error> = None;
    loop {
        if cancelled() {
            failure = Some(Cancelled.into());
            break;
        }
        if Instant::now() >= deadline {
            failure = Some(anyhow::anyhow!(
                "the upload took longer than {UPLOAD_TIMEOUT:?}"
            ));
            break;
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) => {
                failure = Some(error.into());
                break;
            }
        };
        if let Err(error) = stdin.write_all(&buffer[..read]) {
            failure = Some(anyhow::Error::new(error).context("sending the server"));
            break;
        }
        sent += read as u64;
        progress(Progress::Uploaded { sent, total });
    }
    drop(stdin);
    if let Some(error) = failure {
        terminate_child(&mut child);
        let _ = reader.join();
        return Err(error);
    }
    let status = child.wait().context("waiting for the upload to finish")?;
    let _ = reader.join();
    if !status.success() {
        let message = diagnostics
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
            .unwrap_or_default();
        bail!("the upload to {destination} failed: {message}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PROTOCOL_VERSION;
    use crate::client::signature::tests::TestKey;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Executing a script while another test's fork still holds its write handle fails
    /// with "Text file busy", so the tests that write and run scripts take turns, with
    /// the other client tests that do so.
    use crate::client::tests::FAKE_SSH as SCRIPTS;

    struct Fixture {
        _turn: std::sync::MutexGuard<'static, ()>,
        root: PathBuf,
        home: PathBuf,
        ssh: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write script");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    /// A fake remote account (its `ssh` runs the command here, with its own `HOME`, and
    /// `uname` as given) and a release directory holding a stand-in server whose
    /// handshake answers `protocol`. Returns the fixture and the release.
    fn fixture(uname: &str, protocol: u32) -> (Fixture, Release) {
        fixture_serving(
            uname,
            &format!("read line\necho '{{\"Pong\":{{\"version\":{protocol}}}}}'"),
        )
    }

    /// Like [`fixture`], with a stand-in server that runs the shell `server` script.
    fn fixture_serving(uname: &str, server: &str) -> (Fixture, Release) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let turn = SCRIPTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = std::env::temp_dir().join(format!(
            "spooky-install-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        fs::create_dir_all(&home).expect("home");
        let ssh = root.join("ssh");
        // `uname` is answered by the script; everything else runs for real.
        script(
            &ssh,
            &format!(
                "HOME='{}'; export HOME\nfor last in \"$@\"; do :; done\n\
                 if [ \"$last\" = 'uname -sm' ]; then echo '{uname}'; exit 0; fi\n\
                 exec sh -c \"$last\"",
                home.display()
            ),
        );
        let releases = root.join("releases");
        let version = "9.9.9";
        let triple = "x86_64-unknown-linux-musl";
        let directory = releases.join(format!("v{version}"));
        fs::create_dir_all(&directory).expect("release directory");
        let staging = root.join("staging");
        fs::create_dir_all(&staging).expect("staging");
        script(&staging.join(REMOTE_NAME), server);
        let name = format!("spooky-server-{version}-{triple}.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(directory.join(&name))
            .arg("-C")
            .arg(&staging)
            .arg(REMOTE_NAME)
            .status()
            .expect("tar");
        assert!(status.success());
        let sum = Command::new("sha256sum")
            .arg(directory.join(&name))
            .output()
            .expect("sha256sum");
        let hash = String::from_utf8_lossy(&sum.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        fs::write(
            directory.join(format!("{name}.sha256")),
            format!("{hash}  {name}\n"),
        )
        .expect("checksum");
        let key = TestKey::new();
        fs::write(
            directory.join(format!("{name}.minisig")),
            key.sign(&fs::read(directory.join(&name)).expect("archive"), &name),
        )
        .expect("signature");
        let mut release = Release::new(format!("file://{}", releases.display()), version);
        release.keys = key.trusted();
        (
            Fixture {
                _turn: turn,
                root,
                home,
                ssh,
            },
            release,
        )
    }

    fn run(fixture: &Fixture, release: &Release, events: &mut Vec<Progress>) -> Result<Outcome> {
        install_with(
            fixture.ssh.as_os_str(),
            OsStr::new("curl"),
            "user@host",
            release,
            &mut |event| events.push(event),
            &|| false,
        )
    }

    #[test]
    fn the_server_is_downloaded_checked_sent_and_found_answering() {
        let (fixture, release) = fixture("Linux x86_64", PROTOCOL_VERSION);
        let mut events = Vec::new();
        assert_eq!(
            run(&fixture, &release, &mut events).expect("installed"),
            Outcome::Ready
        );
        let installed = fixture.home.join(".local/bin/spooky-server");
        assert!(installed.is_file());
        assert_eq!(
            fs::metadata(&installed)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert!(
            !fixture
                .home
                .join(".local/bin/.spooky-server.upload")
                .exists(),
            "no temporary file is left"
        );
        let steps: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                Progress::Step(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(steps.len(), 5, "{steps:?}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Progress::Uploaded { sent, total } if sent == total))
        );
    }

    #[test]
    fn an_old_server_still_running_means_a_restart_is_needed() {
        let (fixture, release) = fixture("Linux x86_64", PROTOCOL_VERSION + 1);
        let outcome = run(&fixture, &release, &mut Vec::new()).expect("installed");
        assert_eq!(
            outcome,
            Outcome::NeedsRestart {
                running_protocol: PROTOCOL_VERSION + 1
            }
        );
    }

    #[test]
    fn a_new_bridge_refused_by_the_old_running_server_means_a_restart_is_needed() {
        // The real bridge: it fails its own handshake with the server running on its
        // machine and exits with anyhow's report, without answering the client.
        let old = PROTOCOL_VERSION - 1;
        let (fixture, release) = fixture_serving(
            "Linux x86_64",
            &format!(
                "echo 'Error: server protocol version {old} is incompatible with client \
                 version {PROTOCOL_VERSION}. Update both ends and restart the server, \
                 which ends its shells.' >&2\nexit 1"
            ),
        );
        let outcome = run(&fixture, &release, &mut Vec::new()).expect("installed");
        assert_eq!(
            outcome,
            Outcome::NeedsRestart {
                running_protocol: old
            }
        );
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_never_sent() {
        let (fixture, release) = fixture("Linux x86_64", PROTOCOL_VERSION);
        let directory = fixture.root.join("releases/v9.9.9");
        let sum = directory.join("spooky-server-9.9.9-x86_64-unknown-linux-musl.tar.gz.sha256");
        fs::write(&sum, format!("{}  x\n", "0".repeat(64))).expect("tamper");
        let error = run(&fixture, &release, &mut Vec::new()).expect_err("refused");
        assert!(format!("{error:#}").contains("checksum"), "{error:#}");
        assert!(
            !fixture.home.join(".local").exists(),
            "nothing reached the remote account"
        );
    }

    #[test]
    fn a_download_nobody_trusted_signed_is_never_sent() {
        let (fixture, release) = fixture("Linux x86_64", PROTOCOL_VERSION);
        let directory = fixture.root.join("releases/v9.9.9");
        let name = "spooky-server-9.9.9-x86_64-unknown-linux-musl.tar.gz";
        // The checksum matches the archive; the signature is a stranger's.
        let stranger = TestKey::new();
        fs::write(
            directory.join(format!("{name}.minisig")),
            stranger.sign(&fs::read(directory.join(name)).expect("archive"), name),
        )
        .expect("replace the signature");
        let error = run(&fixture, &release, &mut Vec::new()).expect_err("refused");
        assert!(
            format!("{error:#}").contains("not signed by a release key"),
            "{error:#}"
        );
        assert!(
            !fixture.home.join(".local").exists(),
            "nothing reached the remote account"
        );
    }

    #[test]
    fn a_download_without_a_signature_is_never_sent() {
        let (fixture, release) = fixture("Linux x86_64", PROTOCOL_VERSION);
        fs::remove_file(
            fixture.root.join(
                "releases/v9.9.9/spooky-server-9.9.9-x86_64-unknown-linux-musl.tar.gz.minisig",
            ),
        )
        .expect("remove the signature");
        let error = run(&fixture, &release, &mut Vec::new()).expect_err("refused");
        assert!(format!("{error:#}").contains("not signed"), "{error:#}");
        assert!(
            !fixture.home.join(".local").exists(),
            "nothing reached the remote account"
        );
    }

    #[test]
    fn a_machine_without_a_release_is_said_so_before_anything_is_downloaded() {
        let (fixture, release) = fixture("FreeBSD amd64", PROTOCOL_VERSION);
        let mut events = Vec::new();
        let error = run(&fixture, &release, &mut events).expect_err("unsupported");
        assert!(format!("{error:#}").contains("FreeBSD"), "{error:#}");
        assert_eq!(events.len(), 1, "only the check ran: {events:?}");
    }

    #[test]
    fn plain_http_locations_are_refused() {
        let release = Release::new("http://example.test", "1.0.0");
        assert!(release.protocol().is_err());
        assert_eq!(
            Release::new("https://example.test/", "1.0.0")
                .protocol()
                .ok(),
            Some("=https")
        );
    }

    #[test]
    fn a_cancelled_install_stops_before_touching_the_remote() {
        let (fixture, release) = fixture("Linux x86_64", PROTOCOL_VERSION);
        let error = install_with(
            fixture.ssh.as_os_str(),
            OsStr::new("curl"),
            "user@host",
            &release,
            &mut |_| {},
            &|| true,
        )
        .expect_err("cancelled");
        assert!(super::super::was_cancelled(&error), "{error:#}");
        assert!(!fixture.home.join(".local").exists());
    }

    #[test]
    fn restarting_stops_only_this_accounts_server_and_waits_for_it_to_go() {
        // `pkill` and `pgrep` are stand-ins: the real ones would end the server of whoever
        // runs this test.
        let (fixture, _) = fixture("Linux x86_64", PROTOCOL_VERSION);
        let bin = fixture.root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let log = fixture.root.join("pkill.log");
        script(
            &bin.join("pkill"),
            &format!("echo \"$@\" >> '{}'", log.display()),
        );
        script(&bin.join("pgrep"), "exit 1");
        let ssh = fixture.root.join("ssh-with-stubs");
        script(
            &ssh,
            &format!(
                "PATH='{}':\"$PATH\"; export PATH\nfor last in \"$@\"; do :; done\nexec sh -c \"$last\"",
                bin.display()
            ),
        );
        restart_server_with(ssh.as_os_str(), "user@host", &|| false).expect("restarted");
        let logged = fs::read_to_string(&log).expect("pkill ran");
        assert!(
            logged.contains("-u") && logged.contains("[s]pooky-server serve"),
            "{logged}"
        );
    }
}
