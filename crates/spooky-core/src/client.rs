use crate::{MAX_MESSAGE_BYTES, PROTOCOL_VERSION, Request, Response};
use anyhow::{Context, Result, anyhow, bail};
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
        match target {
            Target::Local => Self::connect_local(),
            Target::Ssh(destination) => Self::connect_ssh(destination),
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

    fn connect_local() -> Result<Self> {
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

    fn connect_ssh(destination: &str) -> Result<Self> {
        validate_destination(destination)?;
        let mut command = ssh_command(destination);
        command
            .arg("spooky-server bridge")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().with_context(|| {
            format!("starting SSH to {destination}; install OpenSSH and authorize the host key with a standard `ssh {destination}` connection first")
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
        if let Err(error) = client.verify_protocol() {
            let diagnostics = match &client.transport {
                Transport::Remote(remote) => remote.diagnostic_text(),
                Transport::Local(_) => String::new(),
            };
            let mut error =
                anyhow!("SSH transport to {destination} failed its Spooky protocol handshake: {error:#}")
                    .context("confirm the host key with standard OpenSSH and verify spooky-server is installed on the remote account");
            if !diagnostics.is_empty() {
                error = error.context(format!("SSH diagnostics: {diagnostics}"));
            }
            return Err(error);
        }
        Ok(client)
    }

    fn verify_protocol(&mut self) -> Result<()> {
        let response = self.request(&Request::Ping)?;
        match response {
            Response::Pong { version } if version == PROTOCOL_VERSION => Ok(()),
            Response::Pong { version } => bail!(
                "server protocol version {version} is incompatible with client version {PROTOCOL_VERSION}"
            ),
            Response::Error(message) => bail!("server rejected protocol handshake: {message}"),
            _ => bail!("server returned an unexpected response to protocol handshake"),
        }
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
        match self.responses.recv_timeout(IO_TIMEOUT) {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(error)) => {
                let diagnostic = self.diagnostic_text();
                self.terminate();
                bail!(
                    "reading response from SSH bridge failed: {error}{}",
                    diagnostic_suffix(&diagnostic)
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
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

    fn diagnostic_text(&self) -> String {
        self.stderr
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
            .unwrap_or_default()
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
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// Uploads one regular file. Local targets return its canonical local path;
/// SSH targets use an exclusive private directory under the remote account's home.
pub fn upload(target: &Target, path: &Path) -> Result<String> {
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
        Target::Ssh(destination) => upload_ssh(destination, &canonical),
    }
}

fn upload_ssh(destination: &str, local_path: &Path) -> Result<String> {
    validate_destination(destination)?;
    let home_output = run_ssh_command(destination, "printf '%s\\n' \"$HOME\"")
        .with_context(|| format!("resolving remote home directory for {destination}"))?;
    let home = one_output_line(&home_output, "remote home directory")?;
    validate_remote_absolute_path(home, "remote home directory")?;

    let directory_output = run_ssh_command(
        destination,
        "umask 077; mktemp -d \"$HOME/.spooky-upload.XXXXXX\"",
    )
    .with_context(|| format!("creating a private upload directory on {destination}"))?;
    let directory = one_output_line(&directory_output, "remote upload directory")?;
    validate_remote_absolute_path(directory, "remote upload directory")?;
    let expected_prefix = format!("{}/.spooky-upload.", home.trim_end_matches('/'));
    let suffix = directory
        .strip_prefix(&expected_prefix)
        .context("remote upload directory was not created inside the reported remote home")?;
    if suffix.is_empty() || !suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        bail!("remote upload directory has an unexpected name");
    }

    let extension = local_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    let remote_path = match extension {
        Some(extension) => format!("{directory}/attachment.{extension}"),
        None => format!("{directory}/attachment"),
    };
    let local_text = local_path
        .to_str()
        .context("upload source path is not valid UTF-8, which OpenSSH sftp batch mode requires")?;
    if local_text.chars().any(char::is_control) {
        bail!(
            "upload source path contains control characters and cannot be represented safely in an sftp batch"
        );
    }
    let batch = format!(
        "put {} {}\n",
        sftp_quote(local_text)?,
        sftp_quote(&remote_path)?
    );
    run_sftp_batch(destination, &batch)
        .with_context(|| format!("uploading {} to {destination}", local_path.display()))?;
    Ok(remote_path)
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

fn ssh_command(destination: &str) -> Command {
    let mut command = Command::new("ssh");
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

fn run_ssh_command(destination: &str, remote_command: &str) -> Result<Vec<u8>> {
    let mut command = ssh_command(destination);
    command
        .arg(remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    run_captured(command, SSH_COMMAND_TIMEOUT, "SSH command")
}

fn run_sftp_batch(destination: &str, batch: &str) -> Result<()> {
    let mut command = Command::new("sftp");
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
    wait_captured_child(&mut child, SSH_COMMAND_TIMEOUT, "sftp upload")
}

fn run_captured(mut command: Command, timeout: Duration, operation: &str) -> Result<Vec<u8>> {
    let mut child = command
        .spawn()
        .with_context(|| format!("starting {operation}"))?;
    let stdout = child.stdout.take().context("opening command stdout")?;
    let stderr = child.stderr.take().context("opening command stderr")?;
    let stdout_result = Arc::new(Mutex::new(Vec::new()));
    let stderr_result = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = drain_stderr(stdout, Arc::clone(&stdout_result))?;
    let stderr_thread = drain_stderr(stderr, Arc::clone(&stderr_result))?;
    let status = wait_for_exit(&mut child, timeout, operation)?;
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

fn wait_captured_child(child: &mut Child, timeout: Duration, operation: &str) -> Result<()> {
    let stdout = child.stdout.take().context("opening sftp stdout")?;
    let stderr = child.stderr.take().context("opening sftp stderr")?;
    let stdout_result = Arc::new(Mutex::new(Vec::new()));
    let stderr_result = Arc::new(Mutex::new(Vec::new()));
    let stdout_thread = drain_stderr(stdout, Arc::clone(&stdout_result))?;
    let stderr_thread = drain_stderr(stderr, Arc::clone(&stderr_result))?;
    let status = wait_for_exit(child, timeout, operation)?;
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

fn wait_for_exit(child: &mut Child, timeout: Duration, operation: &str) -> Result<ExitStatus> {
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

fn one_output_line<'a>(output: &'a [u8], label: &str) -> Result<&'a str> {
    let text = std::str::from_utf8(output).with_context(|| format!("{label} was not UTF-8"))?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.is_empty()
        || line.contains('\n')
        || line.contains('\r')
        || line.chars().any(char::is_control)
    {
        bail!("{label} was not exactly one non-empty line");
    }
    Ok(line)
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
    use super::{sftp_quote, validate_destination, validate_remote_absolute_path};

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
}
