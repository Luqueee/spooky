use super::ssh::{posix_quote, run_sftp_batch, run_ssh_command, single_line, validate_destination};
use super::{
    CLEANUP_TIMEOUT, SSH_COMMAND_TIMEOUT, STALE_UPLOAD_DAYS, TRANSFER_BASE_TIMEOUT,
    TRANSFER_MAX_TIMEOUT, TRANSFER_MIN_RATE, Target,
};
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

/// Uploads one regular file. Local targets return its canonical local path.
///
/// SSH targets use an exclusive private directory under the remote account's
/// home, which is removed again if the transfer fails; upload directories older
/// than seven days are swept on each upload. A transfer may run for 30 s plus the
/// time the file needs at 256 KiB/s, up to 30 minutes. `cancelled` abandons the
/// transfer with [`super::Cancelled`].
pub fn upload(target: &Target, path: &Path, cancelled: &dyn Fn() -> bool) -> Result<String> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolving upload file {}", path.display()))?;
    let metadata = canonical
        .metadata()
        .with_context(|| format!("checking upload file {}", canonical.display()))?;
    if !metadata.is_file() {
        bail!(
            "upload source {} is not a regular file",
            canonical.display()
        );
    }
    match target {
        Target::Local => canonical.to_str().map(str::to_owned).context(
            "local upload path is not valid UTF-8 and cannot be returned as a protocol path",
        ),
        Target::Ssh(destination) => upload_ssh(
            &SshTools {
                ssh: OsStr::new("ssh"),
                sftp: OsStr::new("sftp"),
            },
            destination,
            &canonical,
            metadata.len(),
            cancelled,
        ),
    }
}

/// The programs an SSH upload runs; tests substitute scripts for them.
pub(super) struct SshTools<'a> {
    pub(super) ssh: &'a OsStr,
    pub(super) sftp: &'a OsStr,
}

/// How long a transfer of `bytes` may run: a base allowance plus the time the
/// file needs at the slowest rate worth waiting for, so a large file on a slow
/// link is not cut off by the limit meant for a stalled one.
pub(super) fn transfer_timeout(bytes: u64) -> Duration {
    TRANSFER_BASE_TIMEOUT
        .saturating_add(Duration::from_secs(bytes / TRANSFER_MIN_RATE))
        .min(TRANSFER_MAX_TIMEOUT)
}

/// One remote round trip: sweep stale uploads, then print `$HOME` and a fresh
/// private directory under it.
fn prepare_upload_command() -> String {
    format!(
        "umask 077; find \"$HOME\" -maxdepth 1 -type d -name '.spooky-upload.*' \
         -mtime +{STALE_UPLOAD_DAYS} -exec rm -rf {{}} + 2>/dev/null; \
         printf '%s\\n' \"$HOME\"; mktemp -d \"$HOME/.spooky-upload.XXXXXX\""
    )
}

pub(super) fn upload_ssh(
    tools: &SshTools<'_>,
    destination: &str,
    local_path: &Path,
    size: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<String> {
    validate_destination(destination)?;
    let local_text = local_path
        .to_str()
        .context("upload source path is not valid UTF-8, which OpenSSH sftp batch mode requires")?;
    if local_text.chars().any(char::is_control) {
        bail!(
            "upload source path contains control characters and cannot be represented safely in an sftp batch"
        );
    }
    let extension = local_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });

    let prepared = run_ssh_command(
        tools.ssh,
        destination,
        &prepare_upload_command(),
        SSH_COMMAND_TIMEOUT,
        cancelled,
    )
    .with_context(|| format!("preparing an upload directory on {destination}"))?;
    let directory = prepared_upload_directory(&prepared)?;
    let remote_path = match extension {
        Some(extension) => format!("{directory}/attachment.{extension}"),
        None => format!("{directory}/attachment"),
    };
    let batch = format!(
        "put {} {}\n",
        sftp_quote(local_text)?,
        sftp_quote(&remote_path)?
    );
    let transfer = run_sftp_batch(
        tools.sftp,
        destination,
        &batch,
        transfer_timeout(size),
        cancelled,
    )
    .with_context(|| format!("uploading {} to {destination}", local_path.display()));
    if let Err(error) = transfer {
        remove_upload_directory(tools, destination, directory);
        return Err(error);
    }
    Ok(remote_path)
}

/// Checks the output of [`prepare_upload_command`] and returns the directory it
/// created, which must be a Spooky upload directory directly inside the reported home.
pub(super) fn prepared_upload_directory(output: &[u8]) -> Result<&str> {
    let text = std::str::from_utf8(output).context("remote upload setup output was not UTF-8")?;
    let text = text.strip_suffix('\n').unwrap_or(text);
    let (home, directory) = text
        .split_once('\n')
        .context("remote upload setup did not report a directory")?;
    let home = single_line(home, "remote home directory")?;
    let directory = single_line(directory, "remote upload directory")?;
    validate_remote_absolute_path(home, "remote home directory")?;
    validate_remote_absolute_path(directory, "remote upload directory")?;
    let expected_prefix = format!("{}/.spooky-upload.", home.trim_end_matches('/'));
    let suffix = directory
        .strip_prefix(&expected_prefix)
        .context("remote upload directory was not created inside the reported remote home")?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        bail!("remote upload directory has an unexpected name");
    }
    Ok(directory)
}

/// Removes a validated upload directory after a failed transfer. Best effort: one
/// that cannot be removed now is swept once it is [`STALE_UPLOAD_DAYS`] old.
fn remove_upload_directory(tools: &SshTools<'_>, destination: &str, directory: &str) {
    let command = format!("rm -rf -- {}", posix_quote(directory));
    let _ = run_ssh_command(tools.ssh, destination, &command, CLEANUP_TIMEOUT, &|| false);
}

pub(super) fn validate_remote_absolute_path(path: &str, label: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| part == "." || part == "..")
    {
        bail!("{label} was not a safe absolute path");
    }
    Ok(())
}

pub(super) fn sftp_quote(path: &str) -> Result<String> {
    if path.chars().any(char::is_control) {
        bail!("sftp path contains control characters");
    }
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for character in path.chars() {
        if character == '\\' || character == '"' {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Ok(quoted)
}
