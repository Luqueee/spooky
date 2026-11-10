use super::errors::{Cancelled, Unrecoverable};
use super::framing::read_frame;
use super::{
    CANCEL_POLL, IO_TIMEOUT, MAX_SSH_DIAGNOSTIC_BYTES, SSH_CONNECT_TIMEOUT_SECS, THREAD_JOIN_GRACE,
};
use anyhow::{Context, Result, anyhow, bail};
use std::ffi::OsStr;
use std::io::{self, BufReader, Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub(super) struct RemoteTransport {
    pub(super) child: Child,
    pub(super) writes: Option<mpsc::Sender<WriteCommand>>,
    pub(super) responses: mpsc::Receiver<std::result::Result<Vec<u8>, String>>,
    pub(super) stderr: Arc<Mutex<Vec<u8>>>,
    pub(super) threads: Vec<JoinHandle<()>>,
}

pub(super) struct WriteCommand {
    pub(super) bytes: Vec<u8>,
    pub(super) completed: mpsc::Sender<std::result::Result<(), String>>,
}

impl RemoteTransport {
    pub(super) fn write_frame(&mut self, bytes: Vec<u8>) -> Result<()> {
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

    pub(super) fn read_frame(&mut self) -> Result<Vec<u8>> {
        self.read_frame_with(&|| false)
    }

    /// Waits for one response frame, polling `cancelled` so a hung remote side can
    /// be abandoned: the SSH child is killed and [`Cancelled`] returned.
    pub(super) fn read_frame_with(&mut self, cancelled: &dyn Fn() -> bool) -> Result<Vec<u8>> {
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
    pub(super) fn settled_diagnostics(&mut self) -> String {
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

pub(super) fn validate_destination(destination: &str) -> Result<()> {
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

/// The error for a failed SSH handshake: what the remote side reported and what
/// to do about it, marked [`Unrecoverable`] when retrying cannot help.
pub(super) fn ssh_failure(
    destination: &str,
    error: &anyhow::Error,
    diagnostics: &str,
) -> anyhow::Error {
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

pub(super) fn ssh_command_with(program: &OsStr, destination: &str) -> Command {
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

pub(super) fn run_ssh_command(
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

pub(super) fn run_sftp_batch(
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

pub(super) fn drain_stderr(
    reader: impl Read + Send + 'static,
    capture: Arc<Mutex<Vec<u8>>>,
) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("spooky-ssh-stderr".into())
        .spawn(move || drain_bounded(reader, capture))
}

pub(super) fn terminate_child(child: &mut Child) {
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

pub(super) fn remote_writer(mut stdin: impl Write, receiver: mpsc::Receiver<WriteCommand>) {
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

pub(super) fn remote_reader(
    stdout: impl Read,
    sender: mpsc::Sender<std::result::Result<Vec<u8>, String>>,
) {
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

pub(super) fn single_line<'a>(line: &'a str, label: &str) -> Result<&'a str> {
    if line.is_empty() || line.chars().any(char::is_control) {
        bail!("{label} was not exactly one non-empty line");
    }
    Ok(line)
}

/// Quotes `text` as one POSIX shell word.
pub(super) fn posix_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
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
