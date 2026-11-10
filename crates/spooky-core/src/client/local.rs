use anyhow::{Context, Result};
use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub(super) struct LocalTransport {
    pub(super) reader: BufReader<UnixStream>,
    pub(super) writer: UnixStream,
}

pub(super) fn connect_local_socket(path: &Path) -> Result<Option<UnixStream>> {
    if !crate::paths::validate_socket_if_present(path)? {
        return Ok(None);
    }
    match UnixStream::connect(path) {
        Ok(stream) => Ok(Some(stream)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::NotFound
                    | io::ErrorKind::WouldBlock
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error)
            .with_context(|| format!("connecting to local spooky socket {}", path.display())),
    }
}

pub(super) fn local_server_executable() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SPOOKY_SERVER") {
        return Ok(PathBuf::from(path));
    }
    let executable =
        std::env::current_exe().context("locating the Spooky application executable")?;
    let directory = executable
        .parent()
        .context("application executable has no parent directory")?;
    Ok(directory.join("spooky-server"))
}
