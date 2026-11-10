use crate::{MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Response};
use anyhow::{Context, Result, anyhow, bail};
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

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

/// A connection attempt abandoned because the caller no longer wants it.
#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("connection attempt cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A connection failure that retrying cannot fix until the user changes
/// something, such as authorizing a key or installing `spooky-server`. It
/// displays as the explanation to show the user.
#[derive(Debug)]
pub struct Unrecoverable(String);

impl fmt::Display for Unrecoverable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Unrecoverable {}

/// Whether `error` is a [`Cancelled`] connection attempt rather than a failure.
pub fn was_cancelled(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Cancelled>().is_some()
}

/// Whether `error` is [`Unrecoverable`]: retrying the same target is pointless.
pub fn is_unrecoverable(error: &anyhow::Error) -> bool {
    error.downcast_ref::<Unrecoverable>().is_some()
}

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

struct LocalTransport {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

struct RemoteTransport {
    child: Child,
    writes: Option<mpsc::Sender<WriteCommand>>,
    responses: mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    threads: Vec<JoinHandle<()>>,
}

struct WriteCommand {
    bytes: Vec<u8>,
    completed: mpsc::Sender<std::result::Result<(), String>>,
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

impl RemoteTransport {
    fn write_frame(&mut self, bytes: Vec<u8>) -> Result<()> {
        let (completed, result) = mpsc::channel();
        self.writes
            .as_ref()
            .context("SSH bridge writer is closed")?
            .send(WriteCommand { bytes, completed })
            .context("sending request to SSH bridge writer")?;
        match result.recv_timeout(IO_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                let diagnostic = self.diagnostic_text();
                self.terminate();
                bail!(
                    "writing request to SSH bridge failed: {error}{}",
                    diagnostic_suffix(&diagnostic)
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let diagnostic = self.diagnostic_text();
                self.terminate();
                bail!(
                    "timed out writing request to SSH bridge{}",
                    diagnostic_suffix(&diagnostic)
                );
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let diagnostic = self.diagnostic_text();
                self.terminate();
                bail!(
                    "SSH bridge writer stopped unexpectedly{}",
                    diagnostic_suffix(&diagnostic)
                );
            }
        }
    }

    fn read_frame(&mut self) -> Result<Vec<u8>> {
        self.read_frame_with(&|| false)
    }

    /// Waits for one response frame, polling `cancelled` so a hung remote side can
    /// be abandoned: the SSH child is killed and [`Cancelled`] returned.
    fn read_frame_with(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>> {
        let deadline = Instant::now() + IO_TIMEOUT;
        loop {
            match self.responses.recv_timeout(CANCEL_POLL) {
                Ok(Ok(frame)) => return Ok(frame),
                Ok(Err(error)) => {
                    let diagnostic = self.diagnostic_text();
                    self.terminate();
                    bail!(
                        "reading response from SSH bridge failed: {error}{}",
                        diagnostic_suffix(&diagnostic)
                    );
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancelled() {
                        self.terminate();
                        return Err(Cancelled.into());
                    }
                    if Instant::now() < deadline {
                        continue;
                    }
                    let diagnostic = self.diagnostic_text();
                    self.terminate();
                    bail!(
                        "timed out waiting for SSH bridge response{}",
                        diagnostic_suffix(&diagnostic)
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let diagnostic = self.diagnostic_text();
                    self.terminate();
                    bail!(
                        "SSH bridge closed its response stream{}",
                        diagnostic_suffix(&diagnostic)
                    );
                }
            }
        }
    }

    fn diagnostic_text(&self) -> String {
        self.stderr
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
            .unwrap_or_default()
    }

    /// Stops SSH and returns what it reported on stderr, giving its reader a
    /// moment to drain the pipe so a failing connection's explanation is not lost.
    fn settled_diagnostics(&mut self) -> String {
        self.terminate();
        let deadline = Instant::now() + THREAD_JOIN_GRACE;
        while self
            .threads
            .last()
            .is_some_and(|reader| !reader.is_finished())
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        self.diagnostic_text()
    }

    fn terminate(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        drop(self.writes.take());
    }
}

impl Drop for RemoteTransport {
    fn drop(&mut self) {
        self.terminate();
        // A helper still blocked on a pipe held open by something SSH left behind
        // is detached rather than allowed to stall the caller.
        let deadline = Instant::now() + THREAD_JOIN_GRACE;
        for thread in self.threads.drain(..) {
            while !thread.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

/// Uploads one regular file. Local targets return its canonical local path.
///
/// SSH targets use an exclusive private directory under the remote account's
/// home, which is removed again if the transfer fails; upload directories older
/// than seven days are swept on each upload. A transfer may run for 30 s plus the
/// time the file needs at 256 KiB/s, up to 30 minutes. `cancelled` abandons the
/// transfer with [`Cancelled`].
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
struct SshTools<'a> {
    ssh: &'a OsStr,
    sftp: &'a OsStr,
}

/// How long a transfer of `bytes` may run: a base allowance plus the time the
/// file needs at the slowest rate worth waiting for, so a large file on a slow
/// link is not cut off by the limit meant for a stalled one.
fn transfer_timeout(bytes: u64) -> Duration {
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

fn upload_ssh(
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
fn prepared_upload_directory(output: &[u8]) -> Result<&str> {
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

fn connect_local_socket(path: &Path) -> Result<Option<UnixStream>> {
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

fn local_server_executable() -> Result<PathBuf> {
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

fn encode_request(request: &Request) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(request).context("serializing spooky request")?;
    if bytes.len() + 1 > MAX_MESSAGE_BYTES {
        bail!("serialized spooky request exceeds the {MAX_MESSAGE_BYTES}-byte protocol limit");
    }
    bytes.push(b'\n');
    Ok(bytes)
}

fn read_frame(reader: &mut impl BufRead) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().context("reading protocol frame")?;
        if available.is_empty() {
            bail!("server closed the connection before completing a response frame");
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if bytes.len() + consumed > MAX_MESSAGE_BYTES {
            bail!("server response exceeds the {MAX_MESSAGE_BYTES}-byte protocol limit");
        }
        bytes.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            bytes.truncate(bytes.len() - 1);
            return Ok(bytes);
        }
    }
}

fn decode_response(frame: &[u8]) -> Result<Response> {
    serde_json::from_slice(frame).context("invalid JSON response")
}

fn validate_destination(destination: &str) -> Result<()> {
    if destination.is_empty() || destination.starts_with('-') {
        bail!("SSH destination must be a non-empty host or user@host and must not begin with '-'");
    }
    if destination.chars().any(char::is_whitespace) || destination.chars().any(char::is_control) {
        bail!("SSH destination must not contain whitespace or control characters");
    }
    if !destination
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-@[]:%".contains(&byte))
    {
        bail!("SSH destination contains characters outside the supported host/user/IPv6 syntax");
    }
    let mut parts = destination.split('@');
    if let Some(user) = parts.next().filter(|_| destination.contains('@')) {
        let host = parts.next().unwrap_or_default();
        if user.is_empty() || host.is_empty() || parts.next().is_some() {
            bail!("SSH destination must use a non-empty `user@host` when it contains '@'");
        }
    }
    Ok(())
}

fn check_pong(response: Response) -> Result<()> {
    match response {
        Response::Pong { version } if version == PROTOCOL_VERSION => Ok(()),
        Response::Pong { version } => Err(Unrecoverable(format!(
            "server protocol version {version} is incompatible with client version \
             {PROTOCOL_VERSION}. Update both ends and restart the server, which ends its shells."
        ))
        .into()),
        Response::Error(message) => bail!("server rejected protocol handshake: {message}"),
        _ => bail!("server returned an unexpected response to protocol handshake"),
    }
}

/// The error for a failed SSH handshake: what the remote side reported and what
/// to do about it, marked [`Unrecoverable`] when retrying cannot help.
fn ssh_failure(destination: &str, error: &anyhow::Error, diagnostics: &str) -> anyhow::Error {
    let detail = diagnostics
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let detail = if detail.is_empty() {
        format!("{error:#}")
    } else {
        detail
    };
    let evidence = detail.to_lowercase();
    let (advice, unrecoverable) = if evidence.contains("permission denied") {
        (
            "Spooky connects without prompts: authorize a key for this account (for example \
             with ssh-copy-id) or load it into your SSH agent."
                .to_owned(),
            true,
        )
    } else if evidence.contains("host key verification failed")
        || evidence.contains("remote host identification has changed")
    {
        (
            format!("Check the host key with a normal `ssh {destination}` first."),
            true,
        )
    } else if evidence.contains("could not resolve hostname") {
        ("Check the host name or address.".to_owned(), true)
    } else if evidence.contains("command not found") || evidence.contains(": not found") {
        (
            "Install spooky-server on the remote account and put it on the PATH that \
             non-interactive SSH uses."
                .to_owned(),
            true,
        )
    } else {
        (
            format!(
                "Confirm that `ssh {destination}` works and that spooky-server is installed there."
            ),
            false,
        )
    };
    let message = format!("{}. {advice}", detail.trim_end_matches('.'));
    if unrecoverable {
        Unrecoverable(message).into()
    } else {
        anyhow!(message)
    }
}

fn ssh_command_with(program: &OsStr, destination: &str) -> Command {
    let mut command = Command::new(program);
    command
        .arg("-T")
        .arg("-oBatchMode=yes")
        .arg("-oStrictHostKeyChecking=yes")
        .arg(format!("-oConnectTimeout={SSH_CONNECT_TIMEOUT_SECS}"))
        .arg("-oServerAliveInterval=15")
        .arg("-oServerAliveCountMax=2")
        .arg("-oClearAllForwardings=yes")
        .arg(destination);
    command
}

fn run_ssh_command(
    program: &OsStr,
    destination: &str,
    remote_command: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>> {
    let mut command = ssh_command_with(program, destination);
    command
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_captured(command, timeout, "SSH command", cancelled)
}

fn run_sftp_batch(
    program: &OsStr,
    destination: &str,
    batch: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let mut command = Command::new(program);
    command
        .arg("-b")
        .arg("-")
        .arg("-oBatchMode=yes")
        .arg("-oStrictHostKeyChecking=yes")
        .arg(format!("-oConnectTimeout={SSH_CONNECT_TIMEOUT_SECS}"))
        .arg("-oServerAliveInterval=15")
        .arg("-oServerAliveCountMax=2")
        .arg("-oClearAllForwardings=yes")
        .arg(destination)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| {
        format!("starting sftp to {destination}; verify OpenSSH is installed and the host key has been authorized")
    })?;
    let mut stdin = child.stdin.take().context("opening sftp batch input")?;
    if let Err(error) = stdin
        .write_all(batch.as_bytes())
        .and_then(|()| stdin.flush())
    {
        drop(stdin);
        terminate_child(&mut child);
        return Err(error).context("writing sftp upload batch");
    }
    drop(stdin);
    wait_captured_child(&mut child, timeout, "sftp upload", cancelled)
}

fn run_captured(
    mut command: Command,
    timeout: Duration,
    operation: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>> {
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {operation}"))?;
    let stdout = child.stdout.take().context("opening command stdout")?;
    let stderr = child.stderr.take().context("opening command stderr")?;
    let stdout_result = Arc::new(Mutex::new(Vec::new()));
    let stderr_result = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = drain_stderr(stdout, Arc::clone(&stdout_result))?;
    let stderr_thread = drain_stderr(stderr, Arc::clone(&stderr_result))?;
    let status = wait_for_exit(&mut child, timeout, operation, cancelled)?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    let stdout = take_capture(&stdout_result);
    let stderr = String::from_utf8_lossy(&take_capture(&stderr_result))
        .trim()
        .to_owned();
    if !status.success() {
        let message = if stderr.is_empty() {
            format!("{operation} exited with {status}")
        } else {
            format!("{operation} exited with {status}: {stderr}")
        };
        bail!("{message}");
    }
    Ok(stdout)
}

fn wait_captured_child(
    child: &mut Child,
    timeout: Duration,
    operation: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    let stdout = child.stdout.take().context("opening sftp stdout")?;
    let stderr = child.stderr.take().context("opening sftp stderr")?;
    let stdout_result = Arc::new(Mutex::new(Vec::new()));
    let stderr_result = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = drain_stderr(stdout, Arc::clone(&stdout_result))?;
    let stderr_thread = drain_stderr(stderr, Arc::clone(&stderr_result))?;
    let status = wait_for_exit(child, timeout, operation, cancelled)?;
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    let stdout = String::from_utf8_lossy(&take_capture(&stdout_result))
        .trim()
        .to_owned();
    let stderr = String::from_utf8_lossy(&take_capture(&stderr_result))
        .trim()
        .to_owned();
    if !status.success() {
        let diagnostic = if stderr.is_empty() { stdout } else { stderr };
        if diagnostic.is_empty() {
            bail!("{operation} exited with {status}");
        }
        bail!("{operation} exited with {status}: {diagnostic}");
    }
    Ok(())
}

/// Waits for `child`, killing it when `timeout` passes or `cancelled` turns true.
fn wait_for_exit(
    child: &mut Child,
    timeout: Duration,
    operation: &str,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                terminate_child(child);
                return Err(error).with_context(|| format!("waiting for {operation}"));
            }
        }
        if cancelled() {
            terminate_child(child);
            return Err(Cancelled.into());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{operation} timed out after {} seconds", timeout.as_secs());
        }
        thread::sleep(Duration::from_millis(25));
    }
}

fn drain_stderr(
    reader: impl Read + Send + 'static,
    capture: Arc<Mutex<Vec<u8>>>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("spooky-ssh-stderr".into())
        .spawn(move || drain_bounded(reader, capture))
}

fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
}

fn drain_bounded(mut reader: impl Read, capture: Arc<Mutex<Vec<u8>>>) {
    let mut buffer = [0_u8; 4096];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(count) => {
                if let Ok(mut captured) = capture.lock() {
                    let remaining = MAX_SSH_DIAGNOSTIC_BYTES.saturating_sub(captured.len());
                    captured.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
        }
    }
}

fn remote_writer(mut stdin: impl Write, receiver: mpsc::Receiver<WriteCommand>) {
    while let Ok(command) = receiver.recv() {
        let result = stdin
            .write_all(&command.bytes)
            .map_err(|error| error.to_string());
        let failed = result.is_err();
        let _ = command.completed.send(result);
        if failed {
            return;
        }
    }
}

fn remote_reader(stdout: impl Read, sender: mpsc::Sender<std::result::Result<Vec<u8>, String>>) {
    let mut reader = BufReader::new(stdout);
    loop {
        match read_frame(&mut reader) {
            Ok(frame) => {
                if sender.send(Ok(frame)).is_err() {
                    return;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(format!("{error:#}")));
                return;
            }
        }
    }
}

fn single_line<'a>(line: &'a str, label: &str) -> Result<&'a str> {
    if line.is_empty() || line.chars().any(char::is_control) {
        bail!("{label} was not exactly one non-empty line");
    }
    Ok(line)
}

/// Quotes `text` as one POSIX shell word.
fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn validate_remote_absolute_path(path: &str, label: &str) -> Result<()> {
    if !path.starts_with('/')
        || path.chars().any(char::is_control)
        || path.split('/').any(|part| part == "." || part == "..")
    {
        bail!("{label} was not a safe absolute path");
    }
    Ok(())
}

fn sftp_quote(path: &str) -> Result<String> {
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

fn take_capture(capture: &Arc<Mutex<Vec<u8>>>) -> Vec<u8> {
    capture
        .lock()
        .map(|mut bytes| std::mem::take(&mut *bytes))
        .unwrap_or_default()
}

fn diagnostic_suffix(diagnostic: &str) -> String {
    if diagnostic.is_empty() {
        String::new()
    } else {
        format!("; SSH: {diagnostic}")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Client, SshTools, is_unrecoverable, posix_quote, prepared_upload_directory, sftp_quote,
        transfer_timeout, upload_ssh, validate_destination, validate_remote_absolute_path,
        was_cancelled,
    };
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
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make it executable");
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
        assert!(
            validate_remote_absolute_path("/home/user/.spooky-upload.abc123/file", "path").is_ok()
        );
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
}
