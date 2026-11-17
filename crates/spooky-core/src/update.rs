//! Updating the application itself from its published releases.
//!
//! A release is a tag `v<version>` with, for each supported platform, an application
//! archive and its SHA-256 (see `.github/workflows/release.yml`). Two channels follow
//! them: [`Channel::Stable`] only the releases that are not pre-releases, and
//! [`Channel::Dev`] every release, so a development build is offered the newer of the two.
//!
//! An update is done in three steps the caller sequences, so nothing about the
//! installation changes before the person agrees:
//!
//! 1. [`check`] asks which version, if any, is newer.
//! 2. [`prepare`] downloads it, checks the SHA-256, unpacks it beside the installation
//!    and runs the new server's `version` command, which says whether it speaks another
//!    protocol than the one running (restarting the server then ends its shells).
//! 3. [`apply`] swaps the installation for the prepared files, undoing what it did if it
//!    cannot finish, and [`relaunch`] starts the new application after this one exits.
//!
//! Only an installation a release made can be replaced, and only where this account can
//! write ([`support`]): the `.app` bundle on macOS, and a tarball installation on Linux.
//! A package-manager install or a development build is left alone.
//!
//! Every release file is signed with minisign, and the signature is checked against the
//! keys compiled into the program before anything is unpacked (the `client::signature`
//! module); the SHA-256 beside it only tells a damaged download from a good one.

use crate::PROTOCOL_VERSION;
use crate::client::fetch::{
    Workspace, check_sha256, download, fetch_text, protocol_of, run_with_timeout,
};
use crate::client::signature::{self, MAX_SIGNATURE_BYTES, TrustedKeys};
use crate::client::{Client, Progress, official_base};
use crate::{Request, Response};
use anyhow::{Context, Result, bail};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// The version of this build.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Where a person can read about and download every release.
pub const RELEASES_PAGE: &str = "https://github.com/Luqueee/spooky/releases";

const OFFICIAL_INDEX: &str = "https://api.github.com/repos/Luqueee/spooky/releases?per_page=30";
const INDEX_ENV: &str = "SPOOKY_RELEASE_INDEX";
const MAX_INDEX_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SHA_FILE_BYTES: u64 = 1024;
const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// The files that make the application, in an archive and in an installation.
const APP_NAME: &str = "spooky";
const SERVER_NAME: &str = "spooky-server";
const BUNDLE_NAME: &str = "Spooky.app";
/// A file a release's Linux tarball installs, which a development build lacks.
const LINUX_MARKER: &str = "share/applications/dev.spooky.terminal.desktop";

/// Which releases an installation follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Releases that are not pre-releases.
    #[default]
    Stable,
    /// Every release, pre-releases (`0.2.0-dev.3`) included.
    Dev,
}

impl Channel {
    /// The name the settings page shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Stable => "Stable",
            Self::Dev => "Development",
        }
    }
}

/// Where releases are listed and downloaded from.
#[derive(Clone, Debug)]
pub struct Source {
    index: String,
    base: String,
    /// The keys a release must be signed with.
    keys: TrustedKeys,
}

impl Source {
    /// The official releases, unless `SPOOKY_RELEASE_URL` (the files) and
    /// `SPOOKY_RELEASE_INDEX` (the list, in GitHub's releases format) name a mirror or a
    /// directory for development.
    pub fn official() -> Self {
        let index = std::env::var(INDEX_ENV)
            .ok()
            .filter(|index| !index.is_empty())
            .unwrap_or_else(|| OFFICIAL_INDEX.to_owned());
        Self::new(index, official_base())
    }

    /// Releases listed at `index` and downloaded from under `base`; both start with
    /// `https://` (or `file://` for development).
    pub fn new(index: impl Into<String>, base: impl Into<String>) -> Self {
        Self {
            index: index.into(),
            base: base.into().trim_end_matches('/').to_owned(),
            keys: TrustedKeys::release(),
        }
    }

    fn asset_url(&self, version: &str, asset: &str) -> String {
        format!("{}/v{version}/{asset}", self.base)
    }
}

/// A release newer than the one running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    /// Its version, without the leading `v`.
    pub version: String,
    /// Whether it is a pre-release.
    pub prerelease: bool,
}

#[derive(Deserialize)]
struct Entry {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// The newest release `channel` follows, among `entries`.
fn newest(entries: &[Entry], channel: Channel) -> Option<(Version, bool)> {
    entries
        .iter()
        .filter(|entry| !entry.draft)
        .filter_map(|entry| {
            let version = Version::parse(entry.tag_name.strip_prefix('v')?).ok()?;
            Some((version, entry.prerelease))
        })
        .filter(|(version, prerelease)| {
            channel == Channel::Dev || (!prerelease && version.pre.is_empty())
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
}

/// The newest release of `channel` if it is newer than `current`; an installation is
/// never offered an older one.
pub fn check(
    source: &Source,
    channel: Channel,
    current: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<Available>> {
    let current = Version::parse(current)
        .with_context(|| format!("this program's version {current:?} is not a version number"))?;
    let protocol = protocol_of(&source.index)?;
    let text = fetch_text(
        OsStr::new("curl"),
        protocol,
        &source.index,
        MAX_INDEX_BYTES,
        cancelled,
    )?;
    let entries: Vec<Entry> =
        serde_json::from_str(&text).context("the list of releases is not what was expected")?;
    Ok(newest(&entries, channel)
        .filter(|(version, _)| *version > current)
        .map(|(version, prerelease)| Available {
            version: version.to_string(),
            prerelease,
        }))
}

/// How this installation holds the application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// A macOS application bundle: the `Spooky.app` directory.
    Bundle(PathBuf),
    /// Binaries of a release's Linux tarball: the directory holding `spooky` and
    /// `spooky-server`.
    Binaries(PathBuf),
}

impl Install {
    /// The directory the new files are unpacked in and moved from, on the same
    /// filesystem as the installation so the swap is a rename.
    fn parent(&self) -> Option<&Path> {
        match self {
            Self::Bundle(app) => app.parent(),
            Self::Binaries(directory) => Some(directory),
        }
    }

    /// The name of the release archive for this build of the application.
    fn asset(&self, version: &str) -> Result<String> {
        let triple = match (self, std::env::consts::OS, std::env::consts::ARCH) {
            (Self::Bundle(_), "macos", "aarch64") => "aarch64-apple-darwin",
            (Self::Binaries(_), "linux", "x86_64") => "x86_64-unknown-linux-gnu",
            (_, os, arch) => bail!("there is no release of Spooky for {os} on {arch}"),
        };
        let extension = match self {
            Self::Bundle(_) => "zip",
            Self::Binaries(_) => "tar.gz",
        };
        Ok(format!("spooky-app-{version}-{triple}.{extension}"))
    }
}

/// Whether this installation can update itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// It can, as [`Install`] describes.
    Update(Install),
    /// It cannot; the text says why, and a person can still download the release.
    Manual(String),
}

/// Whether the running application can update itself.
pub fn support() -> Support {
    match std::env::current_exe() {
        Ok(executable) => support_of(&executable),
        Err(error) => Support::Manual(format!("Spooky cannot find where it is installed: {error}")),
    }
}

fn support_of(executable: &Path) -> Support {
    let Some(install) = installation_of(executable) else {
        return Support::Manual(
            "This is a development build, not an installed release, so it is not replaced.".into(),
        );
    };
    let Some(parent) = install.parent() else {
        return Support::Manual("Spooky cannot tell where it is installed.".into());
    };
    if !writable(parent) {
        return Support::Manual(format!(
            "Spooky is installed in {}, where this account cannot write. Update it with the \
             tool that installed it, or download the release.",
            parent.display()
        ));
    }
    Support::Update(install)
}

/// The installation `executable` belongs to, when a release made it.
fn installation_of(executable: &Path) -> Option<Install> {
    let directory = executable.parent()?;
    // `Spooky.app/Contents/MacOS/spooky`
    if let Some(contents) = directory.parent()
        && directory.file_name() == Some(OsStr::new("MacOS"))
        && contents.file_name() == Some(OsStr::new("Contents"))
        && let Some(app) = contents.parent()
        && app.file_name() == Some(OsStr::new(BUNDLE_NAME))
    {
        return Some(Install::Bundle(app.to_owned()));
    }
    // `<prefix>/bin/spooky` beside `<prefix>/share/applications/<our desktop file>`
    let prefix = directory.parent()?;
    if directory.file_name() == Some(OsStr::new("bin"))
        && executable.file_name() == Some(OsStr::new(APP_NAME))
        && directory.join(SERVER_NAME).is_file()
        && prefix.join(LINUX_MARKER).is_file()
    {
        return Some(Install::Binaries(directory.to_owned()));
    }
    None
}

/// Whether a file can be created in `directory` by this account right now.
fn writable(directory: &Path) -> bool {
    let probe = directory.join(format!(".spooky-write-check-{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// The text `spooky-server version` prints: the version and the protocol it speaks.
pub fn version_line() -> String {
    format!("{SERVER_NAME} {CURRENT} protocol {PROTOCOL_VERSION}")
}

fn parse_version_line(line: &str) -> Option<(&str, u32)> {
    let mut words = line.split_whitespace();
    (words.next()? == SERVER_NAME).then_some(())?;
    let version = words.next()?;
    (words.next()? == "protocol").then_some(())?;
    let protocol = words.next()?.parse().ok()?;
    words.next().is_none().then_some((version, protocol))
}

/// A downloaded, checked and unpacked release, waiting to replace the installation.
/// Dropping it removes the unpacked files and leaves the installation as it was.
pub struct Prepared {
    install: Install,
    version: String,
    protocol: u32,
    unpacked: PathBuf,
    // Removed on drop; declared last so `unpacked` is not used after it goes.
    _stage: Workspace,
}

impl Prepared {
    /// The version that will be installed.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The protocol the new server speaks. When it differs from
    /// [`PROTOCOL_VERSION`], the server running now cannot be used by the new
    /// application, and stopping it ends its shells and agents.
    pub fn protocol(&self) -> u32 {
        self.protocol
    }
}

/// Downloads release `version`, checks it and unpacks it next to the installation.
/// Nothing of the installation changes.
pub fn prepare(
    source: &Source,
    version: &str,
    install: &Install,
    progress: &mut dyn FnMut(Progress),
    cancelled: &dyn Fn() -> bool,
) -> Result<Prepared> {
    let asset = install.asset(version)?;
    prepare_asset(source, version, install, &asset, progress, cancelled)
}

fn prepare_asset(
    source: &Source,
    version: &str,
    install: &Install,
    asset: &str,
    progress: &mut dyn FnMut(Progress),
    cancelled: &dyn Fn() -> bool,
) -> Result<Prepared> {
    let protocol = protocol_of(&source.base)?;
    let parent = install
        .parent()
        .context("Spooky cannot tell where it is installed")?;
    let curl = OsStr::new("curl");

    let stage = Workspace::create_in(parent, ".spooky-update")?;
    let archive = stage.path().join(asset);
    let url = source.asset_url(version, asset);
    progress(Progress::Step(format!("Downloading Spooky {version}")));
    download(
        curl,
        protocol,
        &url,
        &archive,
        MAX_ARCHIVE_BYTES,
        &mut |bytes| progress(Progress::Downloaded(bytes)),
        cancelled,
    )?;
    let published = fetch_text(
        curl,
        protocol,
        &format!("{url}.sha256"),
        MAX_SHA_FILE_BYTES,
        cancelled,
    )?;
    progress(Progress::Step("Checking the download".into()));
    check_sha256(&archive, &published, asset)?;
    let signature = fetch_text(
        curl,
        protocol,
        &format!("{url}.minisig"),
        MAX_SIGNATURE_BYTES,
        cancelled,
    )
    .with_context(|| {
        format!("{asset} has no signature to check; a release that is not signed is not installed")
    })?;
    signature::verify(&source.keys, &archive, &signature, asset)?;

    progress(Progress::Step("Unpacking it".into()));
    let unpacked = stage.path().join("unpacked");
    fs::create_dir(&unpacked).context("creating the directory to unpack in")?;
    unpack(install, &archive, &unpacked)?;
    let server = match install {
        Install::Bundle(_) => unpacked.join(BUNDLE_NAME).join("Contents/MacOS"),
        Install::Binaries(_) => unpacked.join("bin"),
    };
    for name in [APP_NAME, SERVER_NAME] {
        let metadata = fs::symlink_metadata(server.join(name))
            .with_context(|| format!("the release does not hold {name}"))?;
        if !metadata.is_file() {
            bail!("{name} in the release is not a regular file");
        }
    }
    if let Install::Bundle(_) = install {
        verify_signature(&unpacked.join(BUNDLE_NAME), cancelled)?;
    }

    progress(Progress::Step("Checking that it runs".into()));
    let mut command = Command::new(server.join(SERVER_NAME));
    command.arg("version").stdin(Stdio::null());
    let output = run_with_timeout(command, VERSION_TIMEOUT, "the new server", cancelled)?;
    let printed = String::from_utf8_lossy(&output);
    let (running, new_protocol) = parse_version_line(printed.trim())
        .with_context(|| format!("the new server answered {:?}", printed.trim()))?;
    if running != version {
        bail!("the release holds Spooky {running}, not {version}");
    }
    Ok(Prepared {
        install: install.clone(),
        version: version.to_owned(),
        protocol: new_protocol,
        unpacked,
        _stage: stage,
    })
}

fn unpack(install: &Install, archive: &Path, into: &Path) -> Result<()> {
    let mut command = match install {
        // `ditto` is what made the archive; it keeps the bundle's attributes intact.
        Install::Bundle(_) => {
            let mut command = Command::new("ditto");
            command.arg("-x").arg("-k").arg(archive).arg(into);
            command
        }
        Install::Binaries(_) => {
            let mut command = Command::new("tar");
            command.arg("-xzf").arg(archive).arg("-C").arg(into);
            command
        }
    };
    command.stdin(Stdio::null());
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .output()
        .with_context(|| format!("starting {program}"))?;
    if !output.status.success() {
        bail!(
            "unpacking the release failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// A bundle that does not verify would be refused by the system when it is opened.
fn verify_signature(app: &Path, cancelled: &dyn Fn() -> bool) -> Result<()> {
    let mut command = Command::new("codesign");
    command
        .arg("--verify")
        .arg("--strict")
        .arg(app)
        .stdin(Stdio::null());
    run_with_timeout(command, VERSION_TIMEOUT, "codesign", cancelled)
        .map(drop)
        .context("the new application's signature does not verify")
}

/// What was installed, and what to start now.
#[derive(Debug)]
pub struct Applied {
    start: PathBuf,
    is_bundle: bool,
}

/// Replaces the installation with the prepared files. If it cannot finish, what it
/// moved is put back and the installation is as it was.
pub fn apply(prepared: Prepared) -> Result<Applied> {
    match &prepared.install {
        Install::Bundle(app) => {
            let parent = app.parent().context("the application has no directory")?;
            let backup = parent.join(format!(".{BUNDLE_NAME}.old-{}", std::process::id()));
            swap(
                &[(app.clone(), prepared.unpacked.join(BUNDLE_NAME))],
                &backup,
            )?;
            Ok(Applied {
                start: app.clone(),
                is_bundle: true,
            })
        }
        Install::Binaries(directory) => {
            let backup = directory.join(format!(".spooky.old-{}", std::process::id()));
            let new = prepared.unpacked.join("bin");
            swap(
                &[
                    (directory.join(SERVER_NAME), new.join(SERVER_NAME)),
                    (directory.join(APP_NAME), new.join(APP_NAME)),
                ],
                &backup,
            )?;
            Ok(Applied {
                start: directory.join(APP_NAME),
                is_bundle: false,
            })
        }
    }
}

/// For each `(installed, new)`: moves `installed` into the `backup` directory and `new`
/// to its place. Any failure moves everything back.
fn swap(pairs: &[(PathBuf, PathBuf)], backup: &Path) -> Result<()> {
    fs::create_dir(backup).context("creating the directory for the old files")?;
    let mut moved_out: Vec<(&Path, PathBuf)> = Vec::new();
    let mut moved_in: Vec<&Path> = Vec::new();
    let mut result = Ok(());
    for (index, (installed, new)) in pairs.iter().enumerate() {
        let saved = backup.join(index.to_string());
        let step = fs::rename(installed, &saved)
            .with_context(|| format!("moving {} aside", installed.display()))
            .and_then(|()| {
                moved_out.push((installed, saved));
                fs::rename(new, installed)
                    .with_context(|| format!("putting the new {} in place", installed.display()))
            });
        match step {
            Ok(()) => moved_in.push(installed),
            Err(error) => {
                result = Err(error);
                break;
            }
        }
    }
    if result.is_err() {
        for installed in moved_in {
            let _ = fs::remove_dir_all(installed).or_else(|_| fs::remove_file(installed));
        }
        for (installed, saved) in moved_out.into_iter().rev() {
            let _ = fs::rename(saved, installed);
        }
    }
    let _ = fs::remove_dir_all(backup);
    result
}

/// Starts the new application a moment from now, detached, so it begins after this
/// process has exited. The caller exits right after.
pub fn relaunch(applied: &Applied) -> Result<()> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(if applied.is_bundle {
            "sleep 1; exec open \"$0\""
        } else {
            "sleep 1; exec \"$0\""
        })
        .arg(&applied.start)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    command.spawn().context("starting the new Spooky")?;
    Ok(())
}

/// How many shells and agents the local server is running, or `None` when no server is.
pub fn local_sessions() -> Result<Option<u32>> {
    let Some(mut client) = Client::attach_local()? else {
        return Ok(None);
    };
    match client.request(&Request::Status)? {
        Response::Status(status) => Ok(Some(status.running + status.agents)),
        other => bail!("unexpected reply to a status request: {other:?}"),
    }
}

/// Stops the local server, which ends its shells and agents; its workspaces come back,
/// with new shells, when a server starts again. Only call this after the person agreed.
pub fn stop_local_server() -> Result<()> {
    let Some(mut client) = Client::attach_local()? else {
        return Ok(());
    };
    match client.request(&Request::Shutdown)? {
        Response::Ok => {}
        Response::Error(message) => bail!("the server refused to stop: {message}"),
        other => bail!("unexpected reply to a shutdown request: {other:?}"),
    }
    drop(client);
    for _ in 0..200 {
        if matches!(Client::attach_local(), Ok(None)) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    bail!("the server acknowledged the request but is still running after 5 seconds")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::signature::tests::TestKey;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    fn entry(tag: &str, prerelease: bool) -> Entry {
        Entry {
            tag_name: tag.into(),
            draft: false,
            prerelease,
        }
    }

    fn version_of(found: Option<(Version, bool)>) -> Option<String> {
        found.map(|(version, _)| version.to_string())
    }

    #[test]
    fn the_stable_channel_never_sees_a_prerelease_and_the_dev_channel_sees_the_newest() {
        let entries = [
            entry("v0.2.0-dev.9", true),
            entry("v0.2.0-dev.10", true),
            entry("v0.1.5", false),
            entry("v0.1.4", false),
            Entry {
                tag_name: "v9.9.9".into(),
                draft: true,
                prerelease: false,
            },
            entry("nightly", true),
        ];
        assert_eq!(
            version_of(newest(&entries, Channel::Stable)).as_deref(),
            Some("0.1.5")
        );
        // Numbers, not text: dev.10 is after dev.9, and drafts and odd tags are ignored.
        assert_eq!(
            version_of(newest(&entries, Channel::Dev)).as_deref(),
            Some("0.2.0-dev.10")
        );
        let released = [entry("v0.2.0", false), entry("v0.2.0-dev.3", true)];
        assert_eq!(
            version_of(newest(&released, Channel::Dev)).as_deref(),
            Some("0.2.0"),
            "the release is after its own pre-releases"
        );
        assert_eq!(newest(&[], Channel::Dev), None);
    }

    /// A directory with the list of releases and the files they hold.
    struct Published {
        root: Workspace,
        source: Source,
        /// The key the releases are signed with, and the source trusts.
        key: TestKey,
    }

    impl Published {
        fn new(entries: &str) -> Self {
            let root = Workspace::create().expect("scratch directory");
            fs::write(root.path().join("index.json"), entries).expect("write index");
            let key = TestKey::new();
            let mut source = Source::new(
                format!("file://{}/index.json", root.path().display()),
                format!("file://{}/download", root.path().display()),
            );
            source.keys = key.trusted();
            Self { root, source, key }
        }

        /// Publishes `asset` for `version`: a tarball holding the stand-in programs.
        fn tarball(&self, version: &str, asset: &str, server: &str) {
            let staging = self.root.path().join(format!("staging-{version}"));
            fs::create_dir_all(staging.join("bin")).expect("staging");
            for (name, body) in [(APP_NAME, "#!/bin/sh\nexit 0\n"), (SERVER_NAME, server)] {
                let file = staging.join("bin").join(name);
                fs::write(&file, body).expect("write program");
                fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).expect("chmod");
            }
            let directory = self
                .root
                .path()
                .join("download")
                .join(format!("v{version}"));
            fs::create_dir_all(&directory).expect("release directory");
            let archive = directory.join(asset);
            let status = Command::new("tar")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(&staging)
                .arg(".")
                .status()
                .expect("tar");
            assert!(status.success());
            self.checksum(version, asset, None);
            self.sign_with(&self.key, version, asset);
        }

        /// Writes the published signature of `asset` as `key` makes it.
        fn sign_with(&self, key: &TestKey, version: &str, asset: &str) {
            let directory = self
                .root
                .path()
                .join("download")
                .join(format!("v{version}"));
            let signature = key.sign(&fs::read(directory.join(asset)).expect("read"), asset);
            fs::write(directory.join(format!("{asset}.minisig")), signature)
                .expect("write signature");
        }

        /// Writes the published checksum of `asset`, or `wrong` instead of the real one.
        fn checksum(&self, version: &str, asset: &str, wrong: Option<&str>) {
            let directory = self
                .root
                .path()
                .join("download")
                .join(format!("v{version}"));
            let hash: String = Sha256::digest(fs::read(directory.join(asset)).expect("read"))
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            fs::write(
                directory.join(format!("{asset}.sha256")),
                format!("{}  {asset}\n", wrong.unwrap_or(&hash)),
            )
            .expect("write checksum");
        }
    }

    fn server_printing(version: &str, protocol: u32) -> String {
        format!("#!/bin/sh\necho 'spooky-server {version} protocol {protocol}'\n")
    }

    #[test]
    fn a_newer_release_is_offered_and_an_older_or_equal_one_never() {
        let published = Published::new(
            r#"[{"tag_name":"v0.3.0-dev.1","prerelease":true},{"tag_name":"v0.2.0","prerelease":false}]"#,
        );
        let check = |channel, current| {
            super::check(&published.source, channel, current, &|| false).expect("check")
        };
        assert_eq!(
            check(Channel::Stable, "0.1.0"),
            Some(Available {
                version: "0.2.0".into(),
                prerelease: false
            })
        );
        assert_eq!(
            check(Channel::Dev, "0.2.0"),
            Some(Available {
                version: "0.3.0-dev.1".into(),
                prerelease: true
            })
        );
        assert_eq!(check(Channel::Stable, "0.2.0"), None);
        assert_eq!(
            check(Channel::Stable, "0.3.0-dev.1"),
            None,
            "a development build is not taken back to an older release"
        );
        assert_eq!(check(Channel::Dev, "0.3.0-dev.1"), None);
        assert!(
            super::check(&published.source, Channel::Stable, "not a version", &|| {
                false
            })
            .is_err()
        );
    }

    #[test]
    fn a_list_that_is_not_releases_is_an_error_not_no_update() {
        let published = Published::new(r#"{"message":"API rate limit exceeded"}"#);
        let error = super::check(&published.source, Channel::Stable, "0.1.0", &|| false)
            .expect_err("not a list of releases");
        assert!(
            format!("{error:#}").contains("list of releases"),
            "{error:#}"
        );
    }

    #[test]
    fn only_an_installed_release_in_a_writable_place_updates_itself() {
        let root = Workspace::create().expect("scratch directory");
        let prefix = root.path().join("opt/spooky");
        let bin = prefix.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let executable = bin.join("spooky");
        fs::write(&executable, "").expect("app");
        fs::write(bin.join("spooky-server"), "").expect("server");
        // A development build has no installed desktop file beside it.
        assert!(
            matches!(support_of(&executable), Support::Manual(reason) if reason.contains("development"))
        );
        let applications = prefix.join("share/applications");
        fs::create_dir_all(&applications).expect("applications");
        fs::write(applications.join("dev.spooky.terminal.desktop"), "").expect("desktop file");
        assert_eq!(
            support_of(&executable),
            Support::Update(Install::Binaries(bin.clone()))
        );
        if !rustix::process::geteuid().is_root() {
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o555)).expect("read-only");
            assert!(
                matches!(support_of(&executable), Support::Manual(reason) if reason.contains("cannot write"))
            );
            fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).expect("restore");
        }
        let app = root
            .path()
            .join("Applications/Spooky.app/Contents/MacOS/spooky");
        assert_eq!(
            installation_of(&app),
            Some(Install::Bundle(root.path().join("Applications/Spooky.app")))
        );
    }

    fn installed(root: &Path) -> (Install, PathBuf) {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        fs::write(bin.join("spooky"), "old app").expect("app");
        fs::write(bin.join("spooky-server"), "old server").expect("server");
        (Install::Binaries(bin.clone()), bin)
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .expect("read directory")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    const ASSET: &str = "spooky-app-0.2.0-test.tar.gz";

    fn prepare_test(
        published: &Published,
        install: &Install,
        steps: &mut Vec<Progress>,
    ) -> Result<Prepared> {
        prepare_asset(
            &published.source,
            "0.2.0",
            install,
            ASSET,
            &mut |progress| steps.push(progress),
            &|| false,
        )
    }

    #[test]
    fn a_release_is_downloaded_checked_and_swapped_in_leaving_nothing_behind() {
        let published = Published::new("[]");
        published.tarball(
            "0.2.0",
            ASSET,
            &server_printing("0.2.0", PROTOCOL_VERSION + 1),
        );
        let target = Workspace::create().expect("scratch directory");
        let (install, bin) = installed(target.path());

        let mut steps = Vec::new();
        let prepared = prepare_test(&published, &install, &mut steps).expect("prepared");
        assert_eq!(prepared.version(), "0.2.0");
        assert_eq!(
            prepared.protocol(),
            PROTOCOL_VERSION + 1,
            "the new server's protocol is known before anything is replaced"
        );
        assert_eq!(
            fs::read_to_string(bin.join("spooky")).expect("app"),
            "old app"
        );
        assert!(
            steps
                .iter()
                .any(|step| matches!(step, Progress::Downloaded(bytes) if *bytes > 0))
        );

        let applied = apply(prepared).expect("applied");
        assert_eq!(applied.start, bin.join("spooky"));
        assert!(
            fs::read_to_string(bin.join("spooky-server"))
                .expect("server")
                .contains("0.2.0")
        );
        assert_eq!(
            fs::metadata(bin.join("spooky"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        assert_eq!(
            names(&bin),
            ["spooky", "spooky-server"],
            "no scratch or backup files"
        );
    }

    #[test]
    fn a_prepared_release_that_is_dropped_leaves_the_installation_as_it_was() {
        let published = Published::new("[]");
        published.tarball("0.2.0", ASSET, &server_printing("0.2.0", PROTOCOL_VERSION));
        let target = Workspace::create().expect("scratch directory");
        let (install, bin) = installed(target.path());
        drop(prepare_test(&published, &install, &mut Vec::new()).expect("prepared"));
        assert_eq!(names(&bin), ["spooky", "spooky-server"]);
        assert_eq!(
            fs::read_to_string(bin.join("spooky")).expect("app"),
            "old app"
        );
    }

    #[test]
    fn a_download_that_does_not_match_its_checksum_is_never_unpacked() {
        let published = Published::new("[]");
        published.tarball("0.2.0", ASSET, &server_printing("0.2.0", PROTOCOL_VERSION));
        published.checksum("0.2.0", ASSET, Some(&"0".repeat(64)));
        let target = Workspace::create().expect("scratch directory");
        let (install, bin) = installed(target.path());
        let error = prepare_test(&published, &install, &mut Vec::new())
            .err()
            .expect("refused");
        assert!(format!("{error:#}").contains("checksum"), "{error:#}");
        assert_eq!(
            names(&bin),
            ["spooky", "spooky-server"],
            "nothing left beside the install"
        );
    }

    #[test]
    fn a_release_nobody_trusted_signed_is_never_unpacked() {
        let published = Published::new("[]");
        published.tarball("0.2.0", ASSET, &server_printing("0.2.0", PROTOCOL_VERSION));
        // The archive and its checksum are consistent; the signature is a stranger's.
        published.sign_with(&TestKey::new(), "0.2.0", ASSET);
        let target = Workspace::create().expect("scratch directory");
        let (install, bin) = installed(target.path());
        let error = prepare_test(&published, &install, &mut Vec::new())
            .err()
            .expect("refused");
        assert!(
            format!("{error:#}").contains("not signed by a release key"),
            "{error:#}"
        );
        assert_eq!(names(&bin), ["spooky", "spooky-server"]);
    }

    #[test]
    fn a_release_without_a_signature_is_never_unpacked() {
        let published = Published::new("[]");
        published.tarball("0.2.0", ASSET, &server_printing("0.2.0", PROTOCOL_VERSION));
        fs::remove_file(
            published
                .root
                .path()
                .join(format!("download/v0.2.0/{ASSET}.minisig")),
        )
        .expect("remove the signature");
        let target = Workspace::create().expect("scratch directory");
        let (install, bin) = installed(target.path());
        let error = prepare_test(&published, &install, &mut Vec::new())
            .err()
            .expect("refused");
        assert!(format!("{error:#}").contains("not signed"), "{error:#}");
        assert_eq!(names(&bin), ["spooky", "spooky-server"]);
    }

    #[test]
    fn an_archive_that_holds_another_version_is_refused() {
        let published = Published::new("[]");
        published.tarball("0.2.0", ASSET, &server_printing("0.1.9", PROTOCOL_VERSION));
        let target = Workspace::create().expect("scratch directory");
        let (install, bin) = installed(target.path());
        let error = prepare_test(&published, &install, &mut Vec::new())
            .err()
            .expect("refused");
        assert!(format!("{error:#}").contains("0.1.9"), "{error:#}");
        assert_eq!(names(&bin), ["spooky", "spooky-server"]);
    }

    #[test]
    fn a_swap_that_cannot_finish_puts_everything_back() {
        let root = Workspace::create().expect("scratch directory");
        let dir = root.path();
        fs::write(dir.join("server"), "old server").expect("server");
        fs::write(dir.join("app"), "old app").expect("app");
        fs::write(dir.join("new-server"), "new server").expect("new server");
        // The second new file is missing, so the second step fails after the first worked.
        let error = swap(
            &[
                (dir.join("server"), dir.join("new-server")),
                (dir.join("app"), dir.join("new-app-missing")),
            ],
            &dir.join("backup"),
        )
        .expect_err("cannot finish");
        assert!(format!("{error:#}").contains("in place"), "{error:#}");
        assert_eq!(
            fs::read_to_string(dir.join("server")).expect("server"),
            "old server"
        );
        assert_eq!(fs::read_to_string(dir.join("app")).expect("app"), "old app");
        assert!(!dir.join("backup").exists());
    }

    #[test]
    fn the_version_line_round_trips_and_nothing_else_parses() {
        assert_eq!(
            parse_version_line(&version_line()),
            Some((CURRENT, PROTOCOL_VERSION))
        );
        for bad in [
            "",
            "spooky-server 1.0.0",
            "other 1.0.0 protocol 3",
            "spooky-server 1.0.0 protocol x",
            "spooky-server 1.0.0 protocol 3 extra",
        ] {
            assert_eq!(parse_version_line(bad), None, "{bad:?}");
        }
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod bundle_tests {
    use super::*;
    use crate::client::signature::tests::TestKey;
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    const INFO: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
        <plist version=\"1.0\"><dict>\
        <key>CFBundleExecutable</key><string>spooky</string>\
        <key>CFBundleIdentifier</key><string>dev.spooky.test</string>\
        <key>CFBundlePackageType</key><string>APPL</string>\
        </dict></plist>\n";

    fn bundle(at: &Path, server: &str) {
        let macos = at.join("Spooky.app/Contents/MacOS");
        fs::create_dir_all(&macos).expect("bundle");
        fs::write(at.join("Spooky.app/Contents/Info.plist"), INFO).expect("plist");
        for (name, body) in [("spooky", "#!/bin/sh\nexit 0\n"), ("spooky-server", server)] {
            fs::write(macos.join(name), body).expect("program");
            fs::set_permissions(macos.join(name), fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
    }

    fn sign(app: &Path) {
        // Inside out, as scripts/package.sh does: the programs, then the bundle.
        for target in [
            app.join("Contents/MacOS/spooky-server"),
            app.join("Contents/MacOS/spooky"),
            app.to_owned(),
        ] {
            let status = Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(&target)
                .status()
                .expect("codesign");
            assert!(status.success(), "{} can be signed", target.display());
        }
    }

    fn publish(root: &Path, signed: bool) -> Source {
        let staging = root.join("staging");
        bundle(
            &staging,
            "#!/bin/sh\necho 'spooky-server 0.2.0 protocol 18'\n",
        );
        if signed {
            sign(&staging.join("Spooky.app"));
        }
        let directory = root.join("download/v0.2.0");
        fs::create_dir_all(&directory).expect("release directory");
        let archive = directory.join("spooky-app-0.2.0-test.zip");
        let status = Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(staging.join("Spooky.app"))
            .arg(&archive)
            .status()
            .expect("ditto");
        assert!(status.success());
        let hash: String = Sha256::digest(fs::read(&archive).expect("archive"))
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        fs::write(
            directory.join("spooky-app-0.2.0-test.zip.sha256"),
            format!("{hash}  spooky-app-0.2.0-test.zip\n"),
        )
        .expect("checksum");
        let key = TestKey::new();
        fs::write(
            directory.join("spooky-app-0.2.0-test.zip.minisig"),
            key.sign(
                &fs::read(&archive).expect("archive"),
                "spooky-app-0.2.0-test.zip",
            ),
        )
        .expect("signature");
        let mut source = Source::new(
            "file:///unused",
            format!("file://{}/download", root.display()),
        );
        source.keys = key.trusted();
        source
    }

    fn applications(root: &Path) -> PathBuf {
        let applications = root.join("Applications");
        bundle(&applications, "old server");
        fs::write(applications.join("Spooky.app/old-marker"), "").expect("marker");
        applications.join("Spooky.app")
    }

    #[test]
    fn a_signed_bundle_replaces_the_installed_one_whole() {
        let root = Workspace::create().expect("scratch directory");
        let source = publish(root.path(), true);
        let app = applications(root.path());
        let prepared = prepare_asset(
            &source,
            "0.2.0",
            &Install::Bundle(app.clone()),
            "spooky-app-0.2.0-test.zip",
            &mut |_| {},
            &|| false,
        )
        .expect("prepared");
        assert_eq!(prepared.protocol(), 18);
        assert!(
            app.join("old-marker").exists(),
            "nothing changes before apply"
        );
        let applied = apply(prepared).expect("applied");
        assert_eq!(applied.start, app);
        assert!(!app.join("old-marker").exists(), "the old bundle is gone");
        assert!(
            fs::read_to_string(app.join("Contents/MacOS/spooky-server"))
                .expect("server")
                .contains("0.2.0")
        );
        let mut left: Vec<_> = fs::read_dir(app.parent().expect("parent"))
            .expect("read")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["Spooky.app"], "no scratch or backup directories");
    }

    #[test]
    fn a_bundle_whose_signature_does_not_verify_is_refused() {
        let root = Workspace::create().expect("scratch directory");
        let source = publish(root.path(), false);
        let app = applications(root.path());
        let error = prepare_asset(
            &source,
            "0.2.0",
            &Install::Bundle(app.clone()),
            "spooky-app-0.2.0-test.zip",
            &mut |_| {},
            &|| false,
        )
        .err()
        .expect("refused");
        assert!(format!("{error:#}").contains("signature"), "{error:#}");
        assert!(app.join("old-marker").exists());
    }
}
