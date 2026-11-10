use crate::pane::Size;
use crate::server::{SAVE_DELAY, Server};
use crate::settings::Settings;
use crate::{
    log,
    persist::{self},
};
use anyhow::{Context, Result, bail};
use spooky_core::{MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Response};
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    sync::{Arc, atomic::Ordering},
    thread,
};

pub(crate) fn serve() -> Result<()> {
    let path = spooky_core::paths::socket_path()?;
    let lock_path = path.with_extension("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock_path)
        .context("opening server lock")?;
    let metadata = lock.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        bail!("server lock is not a private file owned by this user");
    }
    lock.try_lock()
        .context("another server owns the socket lock")?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket()
                || metadata.uid() != rustix::process::geteuid().as_raw()
                || metadata.mode() & 0o777 != 0o600
            {
                bail!("existing socket path is not a private socket owned by this user");
            }
            match UnixStream::connect(&path) {
                Ok(_) => bail!("server already listening"),
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                    fs::remove_file(&path).context("removing stale socket")?
                }
                Err(e) => return Err(e).context("cannot establish whether socket is stale"),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("checking socket path"),
    }
    let state_dir = spooky_core::paths::state_dir().context("preparing the state directory")?;
    log::init(&state_dir).context("opening the server log")?;
    info!(
        "starting server (protocol {PROTOCOL_VERSION}, pid {})",
        std::process::id()
    );
    let settings = Settings::load().unwrap_or_else(|error| {
        info!("using default settings: {error:#}");
        Settings::default()
    });
    let saved = persist::load(&state_dir).unwrap_or_else(|error| {
        info!("{error:#}");
        None
    });
    let fallback = saved.as_ref().map_or(Size { cols: 80, rows: 24 }, |saved| {
        let (cols, rows) = saved.size();
        Size { cols, rows }
    });
    let server = Arc::new(Server::new(settings, state_dir, fallback));
    if let Some(saved) = saved {
        let count = saved.workspaces.len();
        match server.restore(saved) {
            Ok(()) => info!("restored {count} saved workspace(s)"),
            Err(error) => info!("restoring saved workspaces failed: {error:#}"),
        }
    }
    let listener = UnixListener::bind(&path).context("binding server socket")?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let saver = server.clone();
    thread::spawn(move || save_loop(&saver));
    info!("listening on {}", path.display());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let server = server.clone();
                thread::spawn(move || {
                    if let Err(error) = handle_client(stream, server) {
                        info!("client connection ended: {error:#}");
                    }
                });
            }
            Err(error) => info!("accept failed: {error}"),
        }
    }
    Ok(())
}
/// Writes the saved workspaces after each burst of layout changes.
fn save_loop(server: &Server) {
    while server.saver.take() {
        thread::sleep(SAVE_DELAY);
        if let Err(error) = server.save() {
            info!("saving workspaces failed: {error:#}");
        }
    }
}
fn handle_client(mut stream: UnixStream, server: Arc<Server>) -> Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut line = Vec::new();
        let n = (&mut reader)
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_until(b'\n', &mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_MESSAGE_BYTES || !line.ends_with(b"\n") {
            write_response(
                &mut stream,
                &Response::Error("request too large or unterminated".into()),
            )?;
            return Ok(());
        }
        let response = match serde_json::from_slice(&line) {
            Ok(request) => server.dispatch(request),
            Err(e) => Response::Error(format!("invalid request: {e}")),
        };
        write_response(&mut stream, &response)?;
        if server.stop_requested.load(Ordering::Acquire) {
            server.stop();
        }
    }
}
pub(crate) fn write_response(output: &mut impl Write, response: &Response) -> Result<()> {
    let mut bytes = serde_json::to_vec(response)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MESSAGE_BYTES {
        bytes = serde_json::to_vec(&Response::Error(
            "response exceeds protocol size limit".into(),
        ))?;
        bytes.push(b'\n');
    }
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}
