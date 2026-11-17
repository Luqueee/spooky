//! Downloading release files with `curl`, and checking them. Shared by the installation
//! of the server on a remote machine and the update of the application itself.
//!
//! Every transfer is bounded in time and size, restricted to one URL scheme (plain HTTP
//! is never used) and abandoned soon after `cancelled` turns true.

use super::CANCEL_POLL;
use super::errors::Cancelled;
use super::ssh::{drain_stderr, terminate_child};
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// How long a download may take, whatever its size.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How long a small text file may take.
const TEXT_TIMEOUT: Duration = Duration::from_secs(25);

/// The `curl` protocol restriction for the location `base`: plain HTTP is never used.
pub(crate) fn protocol_of(base: &str) -> Result<&'static str> {
    if base.starts_with("https://") {
        Ok("=https")
    } else if base.starts_with("file://") {
        Ok("=file")
    } else {
        bail!("the release location must start with https:// (or file:// for development)")
    }
}

/// A private scratch directory, removed when dropped.
pub(crate) struct Workspace(PathBuf);

impl Workspace {
    /// A new directory inside `parent` (mode 0700), named after this process and unique
    /// among the calls it makes.
    pub(crate) fn create_in(parent: &Path, prefix: &str) -> Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let path = parent.join(format!(
            "{prefix}-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path)
            .with_context(|| format!("creating a scratch directory in {}", parent.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(error) = fs::set_permissions(&path, fs::Permissions::from_mode(0o700)) {
                let _ = fs::remove_dir(&path);
                return Err(error).context("restricting the scratch directory");
            }
        }
        Ok(Self(path))
    }

    /// A new directory in the system's temporary directory.
    pub(crate) fn create() -> Result<Self> {
        Self::create_in(&std::env::temp_dir(), "spooky-install")
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn curl_command(curl: &OsStr, protocol: &str, url: &str) -> Command {
    let mut command = Command::new(curl);
    command
        .arg("--fail")
        .arg("--location")
        .arg("--silent")
        .arg("--show-error")
        .arg("--proto")
        .arg(protocol)
        .arg("--proto-redir")
        .arg(protocol)
        .arg("--connect-timeout")
        .arg("15")
        .arg(url);
    command
}

fn captured(diagnostics: &Arc<Mutex<Vec<u8>>>) -> String {
    diagnostics
        .lock()
        .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
        .unwrap_or_default()
}

/// Downloads `url` to `target`, which may be at most `max_bytes`, reporting the bytes
/// received so far through `received`.
pub(crate) fn download(
    curl: &OsStr,
    protocol: &str,
    url: &str,
    target: &Path,
    max_bytes: u64,
    received: &mut dyn FnMut(u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let mut command = curl_command(curl, protocol, url);
    command
        .arg("--max-filesize")
        .arg(max_bytes.to_string())
        .arg("--output")
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .context("starting curl; it is needed to download the release")?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let reader = drain_stderr(
        child.stderr.take().context("opening curl diagnostics")?,
        Arc::clone(&diagnostics),
    )?;
    let deadline = Instant::now() + DOWNLOAD_TIMEOUT;
    let mut last = 0;
    let status = loop {
        if cancelled() {
            terminate_child(&mut child);
            let _ = reader.join();
            return Err(Cancelled.into());
        }
        if let Some(status) = child.try_wait().context("waiting for curl")? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            let _ = reader.join();
            bail!("the download took longer than {DOWNLOAD_TIMEOUT:?}");
        }
        let size = fs::metadata(target).map_or(0, |metadata| metadata.len());
        if size != last {
            last = size;
            received(size);
        }
        std::thread::sleep(CANCEL_POLL);
    };
    let _ = reader.join();
    if !status.success() {
        bail!("downloading {url} failed: {}", captured(&diagnostics));
    }
    let size = fs::metadata(target).map_or(0, |metadata| metadata.len());
    if size > max_bytes {
        bail!("the download of {url} is larger than the {max_bytes}-byte limit");
    }
    received(size);
    Ok(())
}

/// Fetches a text file of at most `max_bytes`.
pub(crate) fn fetch_text(
    curl: &OsStr,
    protocol: &str,
    url: &str,
    max_bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<String> {
    let mut command = curl_command(curl, protocol, url);
    command
        .arg("--header")
        .arg("Accept: application/vnd.github+json")
        .arg("--max-filesize")
        .arg(max_bytes.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().context("starting curl")?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let reader = drain_stderr(
        child.stderr.take().context("opening curl diagnostics")?,
        Arc::clone(&diagnostics),
    )?;
    let stdout = child.stdout.take().context("opening curl output")?;
    // One byte more than allowed, so a body that is too long is noticed, not cut.
    let body = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(max_bytes + 1).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + TEXT_TIMEOUT;
    let status = loop {
        if cancelled() {
            terminate_child(&mut child);
            return Err(Cancelled.into());
        }
        if let Some(status) = child.try_wait().context("waiting for curl")? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            bail!("fetching {url} timed out");
        }
        std::thread::sleep(CANCEL_POLL);
    };
    let _ = reader.join();
    let bytes = body.join().unwrap_or_default();
    if !status.success() {
        bail!("fetching {url} failed: {}", captured(&diagnostics));
    }
    if bytes.len() as u64 > max_bytes {
        bail!("{url} is larger than the {max_bytes}-byte limit");
    }
    String::from_utf8(bytes).with_context(|| format!("{url} is not text"))
}

/// Runs `command` to completion and returns what it printed (at most 64 KiB), killing
/// it when `timeout` passes or `cancelled` turns true. `what` names it in errors.
pub(crate) fn run_with_timeout(
    mut command: Command,
    timeout: Duration,
    what: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {what}"))?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    let reader = drain_stderr(
        child.stderr.take().context("opening diagnostics")?,
        Arc::clone(&diagnostics),
    )?;
    let stdout = child.stdout.take().context("opening output")?;
    let body = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.take(64 * 1024).read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if cancelled() {
            terminate_child(&mut child);
            return Err(Cancelled.into());
        }
        if let Some(status) = child.try_wait().context("waiting for the program")? {
            break status;
        }
        if Instant::now() >= deadline {
            terminate_child(&mut child);
            bail!("{what} did not finish within {timeout:?}");
        }
        std::thread::sleep(CANCEL_POLL);
    };
    let _ = reader.join();
    let bytes = body.join().unwrap_or_default();
    if !status.success() {
        bail!("{what} failed: {}", captured(&diagnostics));
    }
    Ok(bytes)
}

/// Compares the file's SHA-256 with the published line (`<hex>  <name>`).
pub(crate) fn check_sha256(archive: &Path, published: &str, name: &str) -> Result<()> {
    let expected = published
        .split_whitespace()
        .next()
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .with_context(|| format!("the published checksum for {name} is not a SHA-256"))?
        .to_ascii_lowercase();
    let mut file = File::open(archive).context("opening the download")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).context("reading the download")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if actual != expected {
        bail!("the download of {name} does not match its published checksum; it was not used");
    }
    Ok(())
}
