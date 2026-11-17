//! Background work for machines that are not the one in use: a light probe that keeps
//! each machine's state and workspaces up to date, and the installation of the server on
//! a machine that lacks it. Nothing here touches the screen; every result is an
//! [`Event`] the application applies.
//!
//! A probe holds one connection of its own and asks for a snapshot every few seconds. It
//! backs off while a machine is down and stops retrying when only the person can fix it
//! (a missing key, a missing server) until it is told to try again.

use crate::features::machines::{Event, Failure, Install, Probe, Summary};
use spooky_core::{
    Request, Response,
    client::{
        Client, Release, Target, install, is_unrecoverable, remote_problem, restart_server,
        was_cancelled,
    },
};
use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, Thread},
    time::Duration,
};
use tokio::sync::{Mutex, mpsc};

type Channel = (
    mpsc::UnboundedSender<Event>,
    Mutex<mpsc::UnboundedReceiver<Event>>,
);
static EVENTS: LazyLock<Channel> = LazyLock::new(|| {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, Mutex::new(rx))
});

/// The next thing a probe or an installation reported.
pub(crate) async fn next_event() -> Option<Event> {
    EVENTS.1.lock().await.recv().await
}

fn send(event: Event) {
    let _ = EVENTS.0.send(event);
}

/// How often a machine that is up is asked for its workspaces.
const POLL: Duration = Duration::from_secs(3);
const FIRST_RETRY: Duration = Duration::from_secs(2);
const MAX_RETRY: Duration = Duration::from_secs(30);

struct Handle {
    stop: Arc<AtomicBool>,
    thread: Thread,
}

/// The probes that are running, by machine.
#[derive(Default)]
pub(crate) struct Probes {
    running: HashMap<u64, Handle>,
}

impl Probes {
    /// Starts watching a machine, unless it is already watched.
    pub(crate) fn start(&mut self, id: u64, destination: &str) {
        if self.running.contains_key(&id) {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let target = target_of(destination);
        match thread::Builder::new()
            .name("spooky-probe".into())
            .spawn(move || run(id, &target, &flag))
        {
            Ok(thread) => {
                self.running.insert(
                    id,
                    Handle {
                        stop,
                        thread: thread.thread().clone(),
                    },
                );
            }
            Err(error) => send(Event::Probe(
                id,
                Probe::Failed(Failure {
                    reason: format!("Cannot start a connection: {error}"),
                    problem: None,
                    retrying: false,
                }),
            )),
        }
    }

    /// Stops watching a machine, for instance because it is now the one in use.
    pub(crate) fn stop(&mut self, id: u64) {
        if let Some(handle) = self.running.remove(&id) {
            handle.stop.store(true, Ordering::Release);
            handle.thread.unpark();
        }
    }

    /// Tries a machine again now, whatever the probe was waiting for.
    pub(crate) fn poke(&mut self, id: u64, destination: &str) {
        match self.running.get(&id) {
            Some(handle) => handle.thread.unpark(),
            None => self.start(id, destination),
        }
    }
}

fn target_of(destination: &str) -> Target {
    if destination.is_empty() {
        Target::Local
    } else {
        Target::Ssh(destination.to_owned())
    }
}

fn run(id: u64, target: &Target, stop: &AtomicBool) {
    let stopped = || stop.load(Ordering::Acquire);
    let mut wait = FIRST_RETRY;
    while !stopped() {
        send(Event::Probe(id, Probe::Connecting));
        match Client::connect_cancellable(target, &stopped) {
            Ok(mut client) => {
                wait = FIRST_RETRY;
                while !stopped() {
                    match client.request(&Request::Snapshot { since: None }) {
                        Ok(Response::Snapshot(snapshot)) => {
                            send(Event::Probe(id, Probe::Summary(Summary::of(&snapshot))));
                        }
                        Ok(_) => {}
                        Err(error) => {
                            send(Event::Probe(
                                id,
                                Probe::Failed(Failure {
                                    reason: format!("{error:#}"),
                                    problem: None,
                                    retrying: true,
                                }),
                            ));
                            break;
                        }
                    }
                    thread::park_timeout(POLL);
                }
            }
            Err(error) if was_cancelled(&error) => return,
            Err(error) => {
                let fixed_by_the_person = is_unrecoverable(&error);
                send(Event::Probe(
                    id,
                    Probe::Failed(Failure {
                        reason: format!("{error:#}"),
                        problem: remote_problem(&error),
                        retrying: !fixed_by_the_person,
                    }),
                ));
                if fixed_by_the_person {
                    // Only a change on the machine, or a press of Retry, helps.
                    thread::park();
                } else {
                    thread::park_timeout(wait);
                    wait = (wait * 2).min(MAX_RETRY);
                }
            }
        }
    }
}

/// A running installation, which can be cancelled.
pub(crate) struct Installation {
    cancel: Arc<AtomicBool>,
}

impl Installation {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

/// Installs the server on `destination`, reporting progress. Only call this after the
/// person agreed.
pub(crate) fn start_install(id: u64, destination: String) -> Installation {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    let spawned = thread::Builder::new()
        .name("spooky-install".into())
        .spawn(move || {
            let result = install(
                &destination,
                &Release::official(),
                &mut |progress| send(Event::Install(id, Install::Progress(progress))),
                &|| flag.load(Ordering::Acquire),
            )
            .map_err(|error| format!("{error:#}"));
            send(Event::Install(id, Install::Finished(result)));
        });
    if let Err(error) = spawned {
        send(Event::Install(
            id,
            Install::Finished(Err(format!("Cannot start the installation: {error}"))),
        ));
    }
    Installation { cancel }
}

/// Ends the server running on `destination`. Only call this after the person agreed: it
/// ends that server's shells and agents.
pub(crate) fn start_restart(id: u64, destination: String) {
    let spawned = thread::Builder::new()
        .name("spooky-restart".into())
        .spawn(move || {
            let result =
                restart_server(&destination, &|| false).map_err(|error| format!("{error:#}"));
            send(Event::Install(id, Install::Restarted(result)));
        });
    if let Err(error) = spawned {
        send(Event::Install(
            id,
            Install::Restarted(Err(format!("Cannot start the restart: {error}"))),
        ));
    }
}
