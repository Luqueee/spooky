use anyhow::{Context, Result, bail};
use std::env;
use std::fs::{self, Metadata};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

use std::path::{Path, PathBuf};

const SOCKET_NAME: &str = "spooky.sock";

/// Returns Spooky's private runtime directory, creating it with mode 0700.
pub fn runtime_dir() -> Result<PathBuf> {
    let home = home_dir()?;
    let preferred = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Spooky/run")
    } else if let Some(xdg) = env::var_os("XDG_RUNTIME_DIR") {
        let base = PathBuf::from(xdg);
        if !base.is_absolute() {
            bail!("XDG_RUNTIME_DIR must be an absolute path");
        }
        validate_private_dir(&base).context("XDG_RUNTIME_DIR must be a private, user-owned directory without a symlink at its final component")?;
        base.join("spooky")
    } else {
        home.join(".local/state/spooky/run")
    };
    let socket_candidate = preferred.join(SOCKET_NAME);
    let selected = if socket_path_fits(&socket_candidate) {
        preferred
    } else {
        home.join(".spooky/run")
    };
    let socket_candidate = selected.join(SOCKET_NAME);
    if !socket_path_fits(&socket_candidate) {
        bail!(
            "Spooky's Unix socket path is too long; set SPOOKY_SOCKET to a shorter path inside a private directory"
        );
    }
    ensure_private_runtime_dir(&selected)?;
    Ok(selected)
}

/// Returns the Unix-domain socket path. SPOOKY_SOCKET is intended for tests and
/// must name a socket in an existing, private, user-owned directory.
pub fn socket_path() -> Result<PathBuf> {
    let path = match env::var_os("SPOOKY_SOCKET") {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                bail!("SPOOKY_SOCKET must be an absolute path");
            }
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .context("SPOOKY_SOCKET must include a parent directory")?;
            validate_private_dir(parent).context("SPOOKY_SOCKET parent must be an existing, user-owned private directory without a symlink at its final component")?;
            path
        }
        None => runtime_dir()?.join(SOCKET_NAME),
    };
    if !socket_path_fits(&path) {
        bail!("Unix socket path is too long: {}", path.display());
    }
    Ok(path)
}

/// Returns the private directory for state that should outlive a login: the
/// server log and the saved workspaces, created with mode 0700. With
/// SPOOKY_SOCKET set it is the socket's own directory, so an isolated instance
/// never touches the user's real state.
pub fn state_dir() -> Result<PathBuf> {
    if env::var_os("SPOOKY_SOCKET").is_some() {
        return socket_path()?
            .parent()
            .map(Path::to_path_buf)
            .context("socket path has no parent directory");
    }
    let home = home_dir()?;
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Spooky")
    } else {
        match env::var_os("XDG_STATE_HOME").map(PathBuf::from) {
            Some(base) if base.is_absolute() => base.join("spooky"),
            _ => home.join(".local/state/spooky"),
        }
    };
    ensure_private_runtime_dir(&dir)?;
    Ok(dir)
}

/// Validates an existing socket path without following symlinks. Returns false
/// only when the socket does not exist yet.
pub(crate) fn validate_socket_if_present(path: &Path) -> Result<bool> {
    let parent = path
        .parent()
        .context("socket path has no parent directory")?;
    validate_private_dir(parent).context("socket parent directory is not private")?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("checking socket {}", path.display()));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        bail!("{} exists but is not a Unix socket", path.display());
    }
    validate_owner(&metadata, "socket")?;
    if metadata.mode() & 0o7777 != 0o600 {
        bail!("socket {} must have mode 0600", path.display());
    }
    Ok(true)
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
        .context("HOME must be set to an absolute path")
}

fn socket_path_fits(path: &Path) -> bool {
    rustix::net::SocketAddrUnix::new(path).is_ok()
}

fn ensure_private_runtime_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating runtime parent {}", parent.display()))?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("runtime directory {} must not be a symlink", path.display())
        }
        Ok(metadata) if !metadata.is_dir() => {
            bail!("runtime path {} is not a directory", path.display())
        }
        Ok(metadata) => {
            validate_owner(&metadata, "runtime directory")?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("securing runtime directory {}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)
                .with_context(|| format!("creating runtime directory {}", path.display()))?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("securing runtime directory {}", path.display()))?;
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("checking runtime directory {}", path.display()));
        }
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("checking runtime directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "runtime directory {} changed while being secured",
            path.display()
        );
    }
    validate_owner(&metadata, "runtime directory")?;
    if metadata.mode() & 0o7777 != 0o700 {
        bail!("runtime directory {} is not mode 0700", path.display());
    }
    Ok(())
}

fn validate_private_dir(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("checking directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{} is not a real directory", path.display());
    }
    validate_owner(&metadata, "directory")?;
    if metadata.mode() & 0o7777 != 0o700 {
        bail!("directory {} must have mode 0700", path.display());
    }
    Ok(())
}

fn validate_owner(metadata: &Metadata, kind: &str) -> Result<()> {
    let uid = rustix::process::geteuid().as_raw();
    if metadata.uid() != uid {
        bail!(
            "{kind} is owned by uid {}, expected current uid {uid}",
            metadata.uid()
        );
    }
    Ok(())
}
