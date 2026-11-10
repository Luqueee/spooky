use self::framing::{check_pong, decode_response, encode_request, read_frame};
use self::local::{LocalTransport, connect_local_socket, local_server_executable};
use self::ssh::{
    RemoteTransport, WriteCommand, drain_stderr, remote_reader, remote_writer, ssh_command_with,
    ssh_failure, terminate_child, validate_destination,
};
use crate::{Request, Response};
use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::io::{BufReader, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self};
use std::time::{Duration, Instant};

mod errors;
mod framing;
mod local;
mod ssh;
#[cfg(test)]
mod tests;
mod upload;

pub use errors::{Cancelled, Unrecoverable, is_unrecoverable, was_cancelled};
pub use upload::upload;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const SSH_COMMAND_TIMEOUT: Duration = Duration::from_secs(25);
const SSH_CONNECT_TIMEOUT_SECS: u8 = 10;
const MAX_SSH_DIAGNOSTIC_BYTES: usize = 64 * 1024;
/// How often a waiting connection attempt checks whether it was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(100);
/// How long dropping an SSH transport waits for its helper threads before leaving them to finish alone.
const THREAD_JOIN_GRACE: Duration = Duration::from_millis(500);
/// A transfer always gets this long, however small the file.
const TRANSFER_BASE_TIMEOUT: Duration = Duration::from_secs(30);
/// The slowest sustained rate a transfer is expected to keep up, in bytes per second.
const TRANSFER_MIN_RATE: u64 = 256 * 1024;
/// No transfer may run longer than this, whatever the file size.
const TRANSFER_MAX_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Removing a failed upload's directory must not hold the caller up for long.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
/// Upload directories this many days old are removed from the remote account.
const STALE_UPLOAD_DAYS: u32 = 7;

/// A local server or an SSH destination in OpenSSH's `user@host` form.
#[derive(Clone, Debug)]
pub enum Target {
    /// Connect to the per-user local server.
    Local,
    /// Connect to an installed remote `spooky-server` using the user's SSH configuration.
    Ssh(String),
}

/// A blocking, ordered request/reply connection to a Spooky server.
pub struct Client {
    transport: Transport,
}

enum Transport {
    Local(LocalTransport),
    Remote(RemoteTransport),
}

/// Connects to a local or SSH server. Local connections start a sibling server
/// executable (or `SPOOKY_SERVER`) when needed; SSH host keys are never bypassed.
impl Client {
    /// Opens the target, launching the local server if it is not already reachable.
    pub fn connect(target: &Target) -> Result<Self> {
        Self::connect_cancellable(target, &|| false)
    }

    /// Like [`Client::connect`], but abandons the attempt with [`Cancelled`] soon
    /// after `cancelled` returns true, so a slow or hung connection never holds up
    /// a caller that has moved on to another target.
    pub fn connect_cancellable(target: &Target, cancelled: &dyn Fn() -> bool) -> Result<Self> {
        match target {
            Target::Local => Self::connect_local(cancelled),
            Target::Ssh(destination) => {
                Self::connect_ssh(OsStr::new("ssh"), destination, cancelled)
            }
        }
    }

    /// Connects to the local server only if one is already running; never
    /// starts one. `None` means no server is listening.
    pub fn attach_local() -> Result<Option<Self>> {
        let socket = crate::paths::socket_path().context("resolving local spooky socket")?;
        match connect_local_socket(&socket)? {
            Some(stream) => Self::from_local_stream(stream).map(Some),
            None => Ok(None),
        }
    }

    /// Sends one request and waits for its corresponding response.
    pub fn request(&mut self, request: &Request) -> Result<Response> {
        let bytes = encode_request(request)?;
        match &mut self.transport {
            Transport::Local(local) => {
                local
                    .writer
                    .write_all(&bytes)
                    .context("writing request to local spooky server")?;
                let frame = read_frame(&mut local.reader)
                    .context("reading response from local spooky server")?;
                decode_response(&frame).context("decoding response from local spooky server")
            }
            Transport::Remote(remote) => {
                remote.write_frame(bytes)?;
                let frame = remote.read_frame()?;
                decode_response(&frame).context("decoding response from remote spooky server")
            }
        }
    }

    fn connect_local(cancelled: &dyn Fn() -> bool) -> Result<Self> {
        let socket = crate::paths::socket_path().context("resolving local spooky socket")?;
        if let Some(stream) = connect_local_socket(&socket)? {
            return Self::from_local_stream(stream);
        }

        let server = local_server_executable()?;
        let mut command = Command::new(&server);
        command
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.process_group(0);
        let mut child = command.spawn().with_context(|| {
            format!(
                "starting local spooky server at {}; set SPOOKY_SERVER to its executable",
                server.display()
            )
        })?;

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let mut exit_status = None;
        while Instant::now() < deadline {
            match connect_local_socket(&socket) {
                Ok(Some(stream)) => return Self::from_local_stream(stream),
                Ok(None) => {}
                Err(error) => {
                    terminate_child(&mut child);
                    return Err(error).context("checking the local spooky server socket");
                }
            }
            if exit_status.is_none() {
                exit_status = match child.try_wait() {
                    Ok(status) => status,
                    Err(error) => {
                        terminate_child(&mut child);
                        return Err(error).context("checking local spooky server startup");
                    }
                };
            }
            if exit_status.is_some() {
                break;
            }
            if cancelled() {
                // The server keeps starting: it is the user's persistent local
                // server, not part of this attempt.
                return Err(Cancelled.into());
            }
            thread::sleep(Duration::from_millis(25));
        }

        if exit_status.is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let status_text = exit_status
            .map(|status| format!("server exited with {status}"))
            .unwrap_or_else(|| {
                format!(
                    "server did not create {} before startup timed out",
                    socket.display()
                )
            });
        bail!(
            "{status_text}; check that the server binary can run and that SPOOKY_SOCKET is writable"
        )
    }

    fn from_local_stream(stream: UnixStream) -> Result<Self> {
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .context("setting local socket read timeout")?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .context("setting local socket write timeout")?;
        let writer = stream.try_clone().context("cloning local server socket")?;
        let mut client = Self {
            transport: Transport::Local(LocalTransport {
                reader: BufReader::new(stream),
                writer,
            }),
        };
        client.verify_protocol()?;
        Ok(client)
    }

    fn connect_ssh(
        program: &OsStr,
        destination: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self> {
        validate_destination(destination)?;
        let mut command = ssh_command_with(program, destination);
        command
            .arg("spooky-server bridge")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            anyhow::Error::new(Unrecoverable(format!(
                "cannot start SSH for {destination}: {error}. Install OpenSSH."
            )))
        })?;
        let stdin = child.stdin.take().context("opening SSH bridge input")?;
        let stdout = child.stdout.take().context("opening SSH bridge output")?;
        let stderr_reader = child.stderr.take().context("opening SSH diagnostics")?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_thread = match drain_stderr(stderr_reader, Arc::clone(&stderr)) {
            Ok(thread) => thread,
            Err(error) => {
                terminate_child(&mut child);
                return Err(error).context("starting SSH diagnostic reader");
            }
        };
        let (write_sender, write_receiver) = mpsc::channel::<WriteCommand>();
        let (response_sender, response_receiver) = mpsc::channel();
        let writer_thread = match thread::Builder::new()
            .name("spooky-ssh-writer".into())
            .spawn(move || remote_writer(stdin, write_receiver))
        {
            Ok(thread) => thread,
            Err(error) => {
                terminate_child(&mut child);
                let _ = stderr_thread.join();
                return Err(error).context("starting SSH bridge writer");
            }
        };
        let reader_thread = match thread::Builder::new()
            .name("spooky-ssh-reader".into())
            .spawn(move || remote_reader(stdout, response_sender))
        {
            Ok(thread) => thread,
            Err(error) => {
                terminate_child(&mut child);
                drop(write_sender);
                let _ = writer_thread.join();
                let _ = stderr_thread.join();
                return Err(error).context("starting SSH bridge reader");
            }
        };
        let mut client = Self {
            transport: Transport::Remote(RemoteTransport {
                child,
                writes: Some(write_sender),
                responses: response_receiver,
                stderr,
                threads: vec![writer_thread, reader_thread, stderr_thread],
            }),
        };
        if let Err(error) = client.handshake(cancelled) {
            if was_cancelled(&error) || is_unrecoverable(&error) {
                return Err(error);
            }
            let diagnostics = match &mut client.transport {
                Transport::Remote(remote) => remote.settled_diagnostics(),
                Transport::Local(_) => String::new(),
            };
            return Err(ssh_failure(destination, &error, &diagnostics));
        }
        Ok(client)
    }

    fn verify_protocol(&mut self) -> Result<()> {
        let response = self.request(&Request::Ping)?;
        check_pong(response)
    }

    /// The protocol handshake, abandoned early if `cancelled` turns true while an
    /// SSH transport is still waiting for the remote side to answer.
    fn handshake(&mut self, cancelled: &dyn Fn() -> bool) -> Result<()> {
        let Transport::Remote(remote) = &mut self.transport else {
            return self.verify_protocol();
        };
        remote.write_frame(encode_request(&Request::Ping)?)?;
        let frame = remote.read_frame_with(cancelled)?;
        check_pong(decode_response(&frame).context("decoding handshake response")?)
    }
}
