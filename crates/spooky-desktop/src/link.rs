use spooky_core::{
    Request, Response, Snapshot,
    client::{Client, Target, is_unrecoverable, upload, was_cancelled},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, watch};

pub(crate) enum Command {
    Request(u64, Request),
    Upload(u64, u64, PathBuf),
    Target(u64, String),
}
/// One of the two server connections: ordered commands, or the snapshot stream.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Channel {
    Commands,
    Display,
}
#[derive(Clone, Debug)]
pub(crate) enum Event {
    Snapshot(u64, Snapshot),
    Status(u64, String),
    Failed(u64, String),
    Uploaded(u64, u64, String),
    Connection(u64, Channel, bool),
    /// The command connection could not be opened: why, and whether it will be retried.
    Unreachable(u64, String, bool),
    /// Outcome of a search: `(current, total)`, or `None` when nothing matched.
    Found(u64, Option<(u32, u32)>),
    /// Text a program stored in the clipboard with OSC 52.
    Clipboard(u64, String),
}
static EVENTS: OnceLock<Mutex<tokio::sync::mpsc::Receiver<Event>>> = OnceLock::new();
pub(crate) async fn next_event() -> Option<Event> {
    EVENTS.get()?.lock().await.recv().await
}
/// Presses of the global shortcut. They have a channel of their own: sharing the
/// bounded one with snapshots, a press that arrived while output was flooding it was
/// dropped, and the shortcut seemed not to answer. A person cannot press fast enough
/// for this to grow.
static HOTKEYS: OnceLock<Mutex<tokio::sync::mpsc::UnboundedReceiver<u32>>> = OnceLock::new();
pub(crate) async fn next_hotkey() -> Option<u32> {
    HOTKEYS.get()?.lock().await.recv().await
}

/// Waits between automatic reconnection attempts: doubling from 2 s up to 30 s,
/// so a remote that stays down is not hammered.
#[derive(Default)]
struct Backoff {
    failures: u32,
}

impl Backoff {
    const FIRST: Duration = Duration::from_secs(2);
    const MAX: Duration = Duration::from_secs(30);

    fn next(&mut self) -> Duration {
        let delay = Self::FIRST
            .saturating_mul(1 << self.failures.min(8))
            .min(Self::MAX);
        self.failures = self.failures.saturating_add(1);
        delay
    }

    fn reset(&mut self) {
        self.failures = 0;
    }
}

pub(crate) struct Link {
    pub tx: SyncSender<Command>,
    /// The newest target generation requested. Connection attempts for an older
    /// generation are abandoned instead of being waited out.
    latest: Arc<AtomicU64>,
}
impl Link {
    /// Points both connections at `host` (empty: this computer) as `generation`.
    /// Returns false, changing nothing, when the command queue is full.
    pub fn retarget(&self, generation: u64, host: &str) -> bool {
        let queued = self
            .tx
            .try_send(Command::Target(generation, host.into()))
            .is_ok();
        if queued {
            self.latest.store(generation, Ordering::Release);
        }
        queued
    }

    pub fn start() -> Self {
        let (tx, commands) = mpsc::sync_channel(256);
        let (events, rx) = tokio::sync::mpsc::channel(16);
        let _ = EVENTS.set(Mutex::new(rx));
        let (hotkey_events, hotkey_rx) = tokio::sync::mpsc::unbounded_channel();
        let _ = HOTKEYS.set(Mutex::new(hotkey_rx));
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.state == global_hotkey::HotKeyState::Pressed {
                    let _ = hotkey_events.send(event.id);
                }
            },
        ));
        let latest = Arc::new(AtomicU64::new(0));
        let (targets, changes) = watch::channel((0, Target::Local));
        let snapshot_events = events.clone();
        let snapshot_latest = Arc::clone(&latest);
        let command_latest = Arc::clone(&latest);
        let snapshots =
            thread::spawn(move || snapshots(changes, snapshot_events, &snapshot_latest));
        thread::spawn(move || {
            let mut generation = 0;
            let mut target = Target::Local;
            let mut client: Option<Client> = None;
            // The command connection never waits for terminal output or snapshots.
            // `None` means connecting cannot succeed until the user picks a target again.
            let mut next_connect = Some(Instant::now());
            let mut backoff = Backoff::default();
            let uploading = Arc::new(AtomicBool::new(false));
            loop {
                let command = match (client.is_some(), next_connect) {
                    (false, Some(at)) => {
                        commands.recv_timeout(at.saturating_duration_since(Instant::now()))
                    }
                    _ => commands
                        .recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                };
                match command {
                    Ok(Command::Target(g, host)) => {
                        generation = g;
                        target = if host.trim().is_empty() {
                            Target::Local
                        } else {
                            Target::Ssh(host)
                        };
                        client = None;
                        next_connect = Some(Instant::now());
                        backoff.reset();
                        targets.send_replace((generation, target.clone()));
                        snapshots.thread().unpark();
                    }
                    Ok(Command::Request(g, request)) if g == generation => {
                        if let Some(c) = client.as_mut() {
                            match c.request(&request) {
                                // Replies the UI acts on are never dropped: a refusal
                                // must clear whatever the UI is waiting for.
                                Ok(Response::Error(e)) => {
                                    let _ = events.blocking_send(Event::Failed(generation, e));
                                }
                                Ok(Response::Found { current, total }) => {
                                    let _ = events.blocking_send(Event::Found(
                                        generation,
                                        Some((current, total)),
                                    ));
                                }
                                Ok(Response::NotFound) => {
                                    let _ = events.blocking_send(Event::Found(generation, None));
                                }
                                Ok(Response::Clipboard { text, .. }) => {
                                    let _ =
                                        events.blocking_send(Event::Clipboard(generation, text));
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = events.try_send(Event::Status(
                                        generation,
                                        format!("Disconnected: {e}"),
                                    ));
                                    client = None;
                                    let _ = events.blocking_send(Event::Connection(
                                        generation,
                                        Channel::Commands,
                                        false,
                                    ));
                                    next_connect = Some(Instant::now() + Duration::from_secs(1));
                                }
                            }
                        } else {
                            let _ = events.try_send(Event::Status(
                                generation,
                                "Not connected; command not sent".into(),
                            ));
                        }
                    }
                    Ok(Command::Upload(g, pane, path)) if g == generation => {
                        if uploading.swap(true, Ordering::AcqRel) {
                            let _ = events
                                .try_send(Event::Status(g, "An upload is already running".into()));
                        } else {
                            // A transfer can take minutes; it never holds up input or snapshots.
                            spawn_upload(Upload {
                                target: target.clone(),
                                generation: g,
                                pane,
                                path,
                                latest: Arc::clone(&command_latest),
                                busy: BusyGuard(Arc::clone(&uploading)),
                                events: events.clone(),
                            });
                        }
                    }
                    Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if client.is_none() && next_connect.is_some_and(|at| Instant::now() >= at) {
                    let superseded = || command_latest.load(Ordering::Acquire) != generation;
                    match Client::connect_cancellable(&target, &superseded) {
                        Ok(c) => {
                            client = Some(c);
                            backoff.reset();
                            let _ = events.try_send(Event::Status(generation, "Connected".into()));
                            let _ = events.blocking_send(Event::Connection(
                                generation,
                                Channel::Commands,
                                true,
                            ));
                        }
                        // A newer target is already queued; the next read takes it.
                        Err(e) if was_cancelled(&e) => next_connect = Some(Instant::now()),
                        Err(e) => {
                            let retry = !is_unrecoverable(&e);
                            next_connect = retry.then(|| Instant::now() + backoff.next());
                            let _ = events.blocking_send(Event::Unreachable(
                                generation,
                                format!("{e:#}"),
                                retry,
                            ));
                        }
                    }
                }
            }
            drop(targets);
            snapshots.thread().unpark();
        });
        Self { tx, latest }
    }
}

/// Clears the "an upload is running" flag however the upload thread ends.
struct BusyGuard(Arc<AtomicBool>);

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

struct Upload {
    target: Target,
    generation: u64,
    pane: u64,
    path: PathBuf,
    latest: Arc<AtomicU64>,
    busy: BusyGuard,
    events: tokio::sync::mpsc::Sender<Event>,
}

/// Transfers one file on a thread of its own and reports the outcome. Choosing
/// another target abandons the transfer silently.
fn spawn_upload(job: Upload) {
    let events = job.events.clone();
    let generation = job.generation;
    let spawned = thread::Builder::new()
        .name("spooky-upload".into())
        .spawn(move || {
            let Upload {
                target,
                generation,
                pane,
                path,
                latest,
                busy,
                events,
            } = job;
            let superseded = || latest.load(Ordering::Acquire) != generation;
            let event = match upload(&target, &path, &superseded) {
                Ok(path) => Event::Uploaded(generation, pane, path),
                Err(error) if was_cancelled(&error) => return,
                Err(error) => Event::Status(generation, format!("Upload failed: {error:#}")),
            };
            // Free the slot before reporting, so the next upload may start at once.
            drop(busy);
            let _ = events.blocking_send(event);
        });
    if let Err(error) = spawned {
        let _ = events.try_send(Event::Status(
            generation,
            format!("Upload failed: cannot start the transfer thread: {error}"),
        ));
    }
}

// A separate connection can sleep in the server until a PTY/state change. Input
// remains independent even when the terminal deliberately produces no echo.
fn snapshots(
    mut targets: watch::Receiver<(u64, Target)>,
    events: tokio::sync::mpsc::Sender<Event>,
    latest: &AtomicU64,
) {
    let mut backoff = Backoff::default();
    loop {
        if targets.has_changed().is_err() || events.is_closed() {
            return;
        }
        let (generation, target) = targets.borrow_and_update().clone();
        match stream_snapshots(&target, generation, &targets, &events, latest) {
            Ok(()) => backoff.reset(),
            // The target changed while connecting; the loop takes the new one.
            Err(error) if was_cancelled(&error) => {}
            Err(error) => {
                let _ = events.try_send(Event::Status(
                    generation,
                    format!("Display disconnected: {error:#}"),
                ));
                let _ =
                    events.blocking_send(Event::Connection(generation, Channel::Display, false));
                if is_unrecoverable(&error) {
                    // Retrying cannot help; sleep until the target changes.
                    while matches!(targets.has_changed(), Ok(false)) && !events.is_closed() {
                        thread::park_timeout(Duration::from_secs(5));
                    }
                } else {
                    // Target changes unpark this thread; no polling while connected.
                    thread::park_timeout(backoff.next());
                }
            }
        }
    }
}

fn stream_snapshots(
    target: &Target,
    generation: u64,
    targets: &watch::Receiver<(u64, Target)>,
    events: &tokio::sync::mpsc::Sender<Event>,
    latest: &AtomicU64,
) -> anyhow::Result<()> {
    let superseded = || {
        latest.load(Ordering::Acquire) != generation || !matches!(targets.has_changed(), Ok(false))
    };
    let mut client = Client::connect_cancellable(target, &superseded)?;
    events.blocking_send(Event::Connection(generation, Channel::Display, true))?;
    let mut revision = None;
    while matches!(targets.has_changed(), Ok(false)) && !events.is_closed() {
        match client.request(&Request::Snapshot { since: revision })? {
            Response::Snapshot(snapshot) => {
                let current = snapshot.revision;
                // Backpressure keeps ordered deltas intact, never blocks input.
                events.blocking_send(Event::Snapshot(generation, snapshot))?;
                revision = Some(current);
            }
            Response::Unchanged { revision: current } => revision = Some(current),
            Response::Error(error) => anyhow::bail!("{error}"),
            _ => anyhow::bail!("unexpected snapshot response"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Backoff;
    use std::time::Duration;

    #[test]
    fn retries_back_off_to_a_ceiling_and_start_over_after_a_success() {
        let mut backoff = Backoff::default();
        let waits: Vec<u64> = (0..7).map(|_| backoff.next().as_secs()).collect();
        assert_eq!(waits, [2, 4, 8, 16, 30, 30, 30]);
        backoff.reset();
        assert_eq!(backoff.next(), Duration::from_secs(2));
    }
}
