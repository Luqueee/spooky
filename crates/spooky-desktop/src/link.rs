use spooky_core::{
    Request, Response, Snapshot,
    client::{Client, Target, upload},
};
use std::{
    path::PathBuf,
    sync::{
        OnceLock,
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
#[derive(Clone, Debug)]
pub(crate) enum Event {
    Snapshot(u64, Snapshot),
    Status(u64, String),
    Failed(u64, String),
    Uploaded(u64, u64, String),
    Hotkey(u32),
}
static EVENTS: OnceLock<Mutex<tokio::sync::mpsc::Receiver<Event>>> = OnceLock::new();
pub(crate) async fn next_event() -> Option<Event> {
    EVENTS.get()?.lock().await.recv().await
}
pub(crate) struct Link {
    pub tx: SyncSender<Command>,
}
impl Link {
    pub fn start() -> Self {
        let (tx, commands) = mpsc::sync_channel(256);
        let (events, rx) = tokio::sync::mpsc::channel(16);
        let _ = EVENTS.set(Mutex::new(rx));
        let hotkey_events = events.clone();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |event: global_hotkey::GlobalHotKeyEvent| {
                if event.state == global_hotkey::HotKeyState::Pressed {
                    let _ = hotkey_events.try_send(Event::Hotkey(event.id));
                }
            },
        ));
        let (targets, changes) = watch::channel((0, Target::Local));
        let snapshot_events = events.clone();
        let snapshots = thread::spawn(move || snapshots(changes, snapshot_events));
        thread::spawn(move || {
            let mut generation = 0;
            let mut target = Target::Local;
            let mut client: Option<Client> = None;
            // The command connection never waits for terminal output or snapshots.
            let mut next_connect = Instant::now();
            loop {
                let command = if client.is_some() {
                    commands
                        .recv()
                        .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                } else {
                    commands.recv_timeout(next_connect.saturating_duration_since(Instant::now()))
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
                        next_connect = Instant::now();
                        targets.send_replace((generation, target.clone()));
                        snapshots.thread().unpark();
                    }
                    Ok(Command::Request(g, request)) if g == generation => {
                        if let Some(c) = client.as_mut() {
                            match c.request(&request) {
                                Ok(Response::Error(e)) => {
                                    let _ = events.try_send(Event::Failed(generation, e));
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = events.try_send(Event::Status(
                                        generation,
                                        format!("Disconnected: {e}"),
                                    ));
                                    client = None;
                                    next_connect = Instant::now() + Duration::from_secs(1);
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
                        match upload(&target, &path) {
                            Ok(path) => {
                                let _ = events.try_send(Event::Uploaded(g, pane, path));
                            }
                            Err(e) => {
                                let _ = events
                                    .try_send(Event::Status(g, format!("Upload failed: {e}")));
                            }
                        }
                    }
                    Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                if client.is_none() && Instant::now() >= next_connect {
                    match Client::connect(&target) {
                        Ok(c) => {
                            client = Some(c);
                            let _ = events.try_send(Event::Status(generation, "Connected".into()));
                        }
                        Err(e) => {
                            let _ = events
                                .try_send(Event::Status(generation, format!("Connecting: {e}")));
                            next_connect = Instant::now() + Duration::from_secs(2);
                        }
                    }
                }
            }
            drop(targets);
            snapshots.thread().unpark();
        });
        Self { tx }
    }
}

// A separate connection can sleep in the server until a PTY/state change. Input
// remains independent even when the terminal deliberately produces no echo.
fn snapshots(
    mut targets: watch::Receiver<(u64, Target)>,
    events: tokio::sync::mpsc::Sender<Event>,
) {
    loop {
        if targets.has_changed().is_err() || events.is_closed() {
            return;
        }
        let (generation, target) = targets.borrow_and_update().clone();
        let result = stream_snapshots(&target, generation, &targets, &events);
        if let Err(error) = result {
            let _ = events.try_send(Event::Status(
                generation,
                format!("Display disconnected: {error:#}"),
            ));
            // Target changes unpark this thread; no polling while connected.
            thread::park_timeout(Duration::from_secs(1));
        }
    }
}

fn stream_snapshots(
    target: &Target,
    generation: u64,
    targets: &watch::Receiver<(u64, Target)>,
    events: &tokio::sync::mpsc::Sender<Event>,
) -> anyhow::Result<()> {
    let mut client = Client::connect(target)?;
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
