//! Agent panes: a coding agent (oh-my-pi, and other programs through their own
//! [`Backend`]) run by the server, so that the conversation outlives any client and is
//! the same for every client, local or over SSH.
//!
//! A [`Backend`] knows one program's command line and line protocol and translates its
//! frames into [`Event`]s. Everything else — the process, its bounded conversation, the
//! snapshot clients draw — is shared. Everything an agent sends is untrusted text: it is
//! stored and shown, never acted on, and only a user action answers a question it asks.

mod omp;
mod transcript;

pub(crate) use transcript::Transcript;

use crate::{pane::Changes, persist::SavedAgent, server::Saver};
use anyhow::{Context, Result, anyhow, bail, ensure};
use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::Value;
use spooky_core::{
    AgentDialog, AgentRole, Cursor, CursorShape, DialogAnswer, MAX_AGENT_PROMPT_BYTES, ModelRole,
    MouseEncoding, MouseTracking, Pane as PaneSnapshot, UsageProvider,
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::AtomicU64,
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

/// The longest frame read from an agent; a longer one is discarded.
const MAX_FRAME_BYTES: usize = 1024 * 1024;
/// Commands queued for the agent before sending fails instead of blocking.
const COMMAND_QUEUE: usize = 64;
/// How much of the agent's stderr is kept to explain an exit.
const STDERR_TAIL_BYTES: usize = 2048;

/// What happened in an agent, independent of the program that reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Event {
    /// A turn began.
    TurnStarted,
    /// The turn is over and the agent is idle.
    TurnEnded,
    /// The user's message, as the agent recorded it.
    User(String),
    /// A block of the agent's message began streaming (answer text or reasoning).
    BlockStarted(AgentRole),
    /// More text of the block being streamed.
    BlockDelta(String),
    /// The block is complete; this is its whole text (possibly empty).
    BlockDone { role: AgentRole, text: String },
    /// A tool call began.
    ToolStarted {
        call: String,
        name: String,
        intent: String,
        detail: String,
    },
    /// A tool call ended.
    ToolFinished {
        call: String,
        output: String,
        failed: bool,
    },
    /// A request failed.
    Failure(String),
    /// The agent asks the user something.
    Dialog(AgentDialog),
    /// The agent withdrew a question.
    DialogClosed(String),
    /// The model in use, `provider/id`.
    Model(String),
    /// The thinking level in use.
    Thinking(String),
    /// The models that can be chosen.
    Models(Vec<String>),
    /// The thinking levels that can be chosen.
    Levels(Vec<String>),
    /// Context window use, percent.
    Context(u8),
    /// What the providers report about the remaining usage.
    Usage(Vec<UsageProvider>),
}

/// What a backend wants done after a frame.
#[derive(Default)]
pub(crate) struct Reaction {
    /// Changes to the conversation.
    pub(crate) events: Vec<Event>,
    /// Lines to send back to the agent, for protocols that need replies.
    pub(crate) send: Vec<String>,
}

/// One agent program and its line protocol (JSON per line on stdio).
pub(crate) trait Backend: Send {
    /// The name clients request it by.
    fn name(&self) -> &'static str;
    /// The program and arguments; the server sets the directory and the pipes.
    fn command(&self) -> Command;
    /// Shown when the program cannot be started.
    fn install_hint(&self) -> &'static str;
    /// Lines sent right after it starts.
    fn start(&mut self) -> Vec<String>;
    /// Translates one frame.
    fn frame(&mut self, frame: &Value) -> Reaction;
    /// The line that sends a message; `busy` while a turn runs.
    fn prompt(&mut self, text: &str, busy: bool) -> String;
    /// The line that interrupts the turn.
    fn abort(&mut self) -> String;
    /// The line that selects a model the agent listed, if the program can.
    fn set_model(&mut self, _model: &str) -> Option<String> {
        None
    }
    /// The line that selects a thinking level the agent listed, if the program can.
    fn set_thinking(&mut self, _level: &str) -> Option<String> {
        None
    }
    /// The line that answers a dialog.
    fn answer(&mut self, _dialog: &str, _answer: &DialogAnswer) -> Option<String> {
        None
    }
    /// The program's own session, to resume after a restart.
    fn session(&self) -> Option<String> {
        None
    }
    /// The roles the program's own configuration gives its models.
    fn roles(&self) -> Vec<ModelRole> {
        Vec::new()
    }
    /// The line that asks the program for the usage its providers report; the answer
    /// comes back as [`Event::Usage`].
    fn usage(&mut self) -> Option<String> {
        None
    }
}

/// The backend a client asked for, with the arguments the user gave. `session` resumes
/// a saved conversation.
pub(crate) fn backend(
    name: &str,
    args: &[String],
    session: Option<&str>,
) -> Result<Box<dyn Backend>> {
    match name {
        "omp" => Ok(Box::new(omp::Omp::new(args, session)?)),
        other => bail!("unknown agent backend {other:?}; available: omp"),
    }
}

/// A running agent process the server holds for one pane.
pub(crate) struct Agent {
    id: u64,
    name: &'static str,
    cwd: PathBuf,
    backend: Arc<Mutex<Box<dyn Backend>>>,
    transcript: Arc<Mutex<Transcript>>,
    commands: SyncSender<String>,
    changes: Arc<Changes>,
    child: Mutex<Option<Child>>,
    pub(crate) revision: Arc<AtomicU64>,
}

impl Agent {
    /// Starts `backend` in `cwd`.
    pub(crate) fn spawn(
        id: u64,
        mut backend: Box<dyn Backend>,
        cwd: &Path,
        changes: Arc<Changes>,
        saver: Arc<Saver>,
    ) -> Result<Arc<Self>> {
        let name = backend.name();
        let mut command = backend.command();
        let mut child = command
            .current_dir(cwd)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "starting {}: {}",
                    command.get_program().to_string_lossy(),
                    backend.install_hint()
                )
            })?;
        let (Some(mut stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            bail!("{name} started without its pipes");
        };
        let (commands, queued) = mpsc::sync_channel::<String>(COMMAND_QUEUE);
        for line in backend.start() {
            commands
                .try_send(line)
                .context("queueing the agent's start commands")?;
        }
        thread::spawn(move || {
            while let Ok(line) = queued.recv() {
                if writeln!(stdin, "{line}")
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
        let tail = Arc::clone(&stderr_tail);
        thread::spawn(move || {
            let mut reader = stderr;
            let mut chunk = [0_u8; 1024];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                if let Ok(mut held) = tail.lock() {
                    held.extend_from_slice(&chunk[..n]);
                    let excess = held.len().saturating_sub(STDERR_TAIL_BYTES);
                    held.drain(..excess);
                }
            }
        });
        let revision = Arc::new(AtomicU64::new(0));
        let mut shown = Transcript::default();
        shown.set_roles(backend.roles());
        let transcript = Arc::new(Mutex::new(shown));
        let backend = Arc::new(Mutex::new(backend));
        let agent = Arc::new(Self {
            id,
            name,
            cwd: cwd.to_path_buf(),
            backend: Arc::clone(&backend),
            transcript: Arc::clone(&transcript),
            commands: commands.clone(),
            changes: Arc::clone(&changes),
            child: Mutex::new(Some(child)),
            revision: Arc::clone(&revision),
        });
        let reading = Arc::downgrade(&agent);
        thread::spawn(move || {
            let mut sink = Sink {
                backend: &backend,
                transcript: &transcript,
                commands: &commands,
                changes: &changes,
                saver: &saver,
                saved_session: None,
                revision: &revision,
            };
            read_frames(stdout, &mut sink);
            let reason = reading.upgrade().map_or_else(
                || "closed".to_owned(),
                |agent| agent.exit_reason(&stderr_tail),
            );
            if let Ok(mut held) = transcript.lock() {
                held.end(reason);
            }
            changes.advance(&revision);
        });
        Ok(agent)
    }

    /// Sends a message. While a turn runs it is queued after it.
    pub(crate) fn prompt(&self, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            bail!("the message is empty");
        }
        if text.len() > MAX_AGENT_PROMPT_BYTES {
            bail!("the message is too long");
        }
        let (busy, ended) = {
            let held = self.lock()?;
            (held.is_busy(), held.has_ended())
        };
        if ended {
            bail!("the agent has ended");
        }
        let line = self.with_backend(|backend| backend.prompt(text, busy))?;
        self.send(line)
    }

    /// Asks the program what its providers report about the remaining usage.
    pub(crate) fn usage(&self) -> Result<()> {
        ensure!(
            !self.running()?.is_busy(),
            "the agent is working; usage can be read when it is idle"
        );
        let line = self
            .with_backend(|backend| backend.usage())?
            .context("this agent does not report usage")?;
        self.send(line)
    }

    /// Interrupts the current turn.
    pub(crate) fn abort(&self) -> Result<()> {
        drop(self.running()?);
        let line = self.with_backend(|backend| backend.abort())?;
        self.send(line)
    }

    /// Selects one of the models the agent listed.
    pub(crate) fn set_model(&self, model: &str) -> Result<()> {
        if !self.running()?.offers_model(model) {
            bail!("the agent does not offer that model");
        }
        let line = self
            .with_backend(|backend| backend.set_model(model))?
            .context("this agent cannot change its model")?;
        self.send(line)
    }

    /// Selects one of the thinking levels the agent listed.
    pub(crate) fn set_thinking(&self, level: &str) -> Result<()> {
        if !self.running()?.offers_level(level) {
            bail!("the agent does not offer that thinking level");
        }
        let line = self
            .with_backend(|backend| backend.set_thinking(level))?
            .context("this agent cannot change its thinking level")?;
        self.send(line)
    }

    /// Answers a dialog the agent is waiting on.
    pub(crate) fn answer(&self, dialog: &str, answer: &DialogAnswer) -> Result<()> {
        self.lock()?
            .take_dialog(dialog, answer)
            .map_err(|reason| anyhow!(reason))?;
        self.changes.advance(&self.revision);
        let line = self
            .with_backend(|backend| backend.answer(dialog, answer))?
            .context("this agent cannot be answered")?;
        self.send(line)
    }

    /// Whether the agent process is still running.
    pub(crate) fn is_alive(&self) -> bool {
        self.transcript.lock().is_ok_and(|held| !held.has_ended())
    }

    /// What is needed to start this agent again after a server restart.
    pub(crate) fn saved(&self) -> Option<SavedAgent> {
        let session = self.backend.lock().ok()?.session();
        Some(SavedAgent {
            pane: self.id,
            backend: self.name.to_owned(),
            cwd: self.cwd.to_string_lossy().into_owned(),
            session,
            covers: false,
        })
    }

    /// The pane snapshot clients draw.
    pub(crate) fn snapshot(&self) -> Result<PaneSnapshot> {
        let held = self.lock()?;
        Ok(PaneSnapshot {
            id: self.id,
            title: "Agent".into(),
            cols: 0,
            rows: 0,
            lines: Vec::new(),
            cursor: Cursor {
                row: 0,
                col: 0,
                visible: false,
                shape: CursorShape::Beam,
                blinking: false,
            },
            exited: held.has_ended(),
            bracketed_paste: false,
            application_cursor: false,
            mouse_tracking: MouseTracking::Off,
            mouse_encoding: MouseEncoding::Normal,
            alternate_scroll: false,
            history: 0,
            scrolled: 0,
            focus_reporting: false,
            clipboard_seq: 0,
            highlights: Vec::new(),
            agent: Some(held.snapshot(self.name)),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Transcript>> {
        self.transcript
            .lock()
            .map_err(|_| anyhow!("agent state poisoned"))
    }

    /// The conversation, if the agent has not ended.
    fn running(&self) -> Result<MutexGuard<'_, Transcript>> {
        let held = self.lock()?;
        if held.has_ended() {
            bail!("the agent has ended");
        }
        Ok(held)
    }

    fn with_backend<T>(&self, f: impl FnOnce(&mut dyn Backend) -> T) -> Result<T> {
        let mut backend = self
            .backend
            .lock()
            .map_err(|_| anyhow!("agent backend poisoned"))?;
        Ok(f(backend.as_mut()))
    }

    fn send(&self, line: String) -> Result<()> {
        self.commands
            .try_send(line)
            .map_err(|error| anyhow!("the agent is not accepting input: {error}"))
    }

    /// Why the process stopped, from its exit status and the end of its stderr.
    fn exit_reason(&self, stderr_tail: &Mutex<Vec<u8>>) -> String {
        let status = self.child.lock().ok().and_then(|mut child| {
            let child = child.as_mut()?;
            let deadline = Instant::now() + Duration::from_millis(500);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => return Some(status.to_string()),
                    Ok(None) if Instant::now() < deadline => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    _ => return None,
                }
            }
        });
        let tail = stderr_tail
            .lock()
            .map(|held| String::from_utf8_lossy(&held).trim().to_owned())
            .unwrap_or_default();
        let name = self.name;
        match (status, tail.is_empty()) {
            (Some(status), true) => format!("{name} stopped ({status})"),
            (Some(status), false) => format!("{name} stopped ({status}): {tail}"),
            (None, true) => format!("{name} stopped"),
            (None, false) => format!("{name} stopped: {tail}"),
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        let Some(mut child) = self.child.lock().ok().and_then(|mut held| held.take()) else {
            return;
        };
        // Agents run tools as children: end the whole group, then make sure of it.
        let group = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
        if let Some(group) = group {
            let _ = kill_process_group(group, Signal::TERM);
        }
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
                let _ = kill_process_group(group, Signal::KILL);
            }
            let _ = child.wait();
        });
    }
}

/// Where frames go: the backend translates, the conversation changes, replies are sent.
struct Sink<'a> {
    backend: &'a Mutex<Box<dyn Backend>>,
    transcript: &'a Mutex<Transcript>,
    commands: &'a SyncSender<String>,
    changes: &'a Changes,
    saver: &'a Saver,
    /// The session last reported to the saver.
    saved_session: Option<String>,
    revision: &'a AtomicU64,
}

impl Sink<'_> {
    fn frame(&mut self, frame: &Value) {
        let Ok(reaction) = self.backend.lock().map(|mut backend| backend.frame(frame)) else {
            return;
        };
        for line in reaction.send {
            let _ = self.commands.try_send(line);
        }
        let changed = self.transcript.lock().is_ok_and(|mut held| {
            let mut changed = false;
            for event in reaction.events {
                changed |= held.apply(event);
            }
            changed
        });
        if changed {
            self.changes.advance(self.revision);
        }
        // Once the program says which session it is in, the saved state is stale.
        let session = self
            .backend
            .lock()
            .ok()
            .and_then(|backend| backend.session());
        if session != self.saved_session {
            self.saved_session = session;
            self.saver.mark();
        }
    }
}

/// Reads frames until the agent closes its output.
fn read_frames(stdout: impl Read, sink: &mut Sink<'_>) {
    let mut reader = BufReader::new(stdout);
    let mut line = Vec::new();
    loop {
        line.clear();
        match (&mut reader)
            .take(MAX_FRAME_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if !line.ends_with(b"\n") {
            // Longer than a frame may be: skip the rest of it.
            if !discard_line(&mut reader) {
                return;
            }
            continue;
        }
        if let Ok(frame) = serde_json::from_slice::<Value>(&line) {
            sink.frame(&frame);
        }
    }
}

/// Reads to the end of the current line. False when the stream ends first.
fn discard_line(reader: &mut impl BufRead) -> bool {
    let mut sink = Vec::new();
    loop {
        sink.clear();
        match (&mut *reader)
            .take(MAX_FRAME_BYTES as u64)
            .read_until(b'\n', &mut sink)
        {
            Ok(0) | Err(_) => return false,
            Ok(_) if sink.ends_with(b"\n") => return true,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn an_unknown_backend_is_refused_with_the_available_ones() {
        let error = backend("nope", &[], None)
            .err()
            .expect("unknown")
            .to_string();
        assert!(error.contains("omp"), "{error}");
    }

    #[test]
    fn an_oversize_frame_is_skipped_without_losing_the_next_one() {
        let mut data = vec![b'x'; MAX_FRAME_BYTES + 10];
        data.extend_from_slice(b"\n{\"type\":\"agent_start\"}\n");
        let backend: Mutex<Box<dyn Backend>> = Mutex::new(backend("omp", &[], None).expect("omp"));
        let transcript = Mutex::new(Transcript::default());
        let changes = Changes::default();
        let revision = AtomicU64::new(0);
        let (commands, _queued) = mpsc::sync_channel(1);
        let saver = Saver::default();
        let mut sink = Sink {
            backend: &backend,
            transcript: &transcript,
            commands: &commands,
            changes: &changes,
            saver: &saver,
            saved_session: None,
            revision: &revision,
        };
        read_frames(std::io::Cursor::new(data), &mut sink);
        assert!(transcript.lock().expect("lock").is_busy());
        assert_eq!(revision.load(Ordering::Acquire), 1);
    }
}
