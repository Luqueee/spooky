//! Background work for updating the application: asking for newer releases, preparing
//! one and replacing the installation. Nothing here touches the screen; every result is
//! an [`Event`] the application applies.
//!
//! A prepared release waits in a slot until the person agrees to restart, so nothing about
//! the installation changes before that, and dropping it (closing the dialog) removes the
//! files that were unpacked.

use crate::features::updates::{Event, Ready};
use spooky_core::{
    PROTOCOL_VERSION,
    client::was_cancelled,
    update::{
        self, Available, CURRENT, Channel, Install, Prepared, Source, apply, local_sessions,
        prepare, relaunch, stop_local_server,
    },
};
use std::{
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use tokio::sync::{Mutex as AsyncMutex, mpsc};

type Events = (
    mpsc::UnboundedSender<Event>,
    AsyncMutex<mpsc::UnboundedReceiver<Event>>,
);
static EVENTS: LazyLock<Events> = LazyLock::new(|| {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, AsyncMutex::new(rx))
});

/// The next thing an update thread reported.
pub(crate) async fn next_event() -> Option<Event> {
    EVENTS.1.lock().await.recv().await
}

fn send(event: Event) {
    let _ = EVENTS.0.send(event);
}

fn spawn(name: &str, work: impl FnOnce() + Send + 'static, failed: impl FnOnce(String)) {
    if let Err(error) = thread::Builder::new().name(name.into()).spawn(work) {
        failed(format!("Cannot start the update: {error}"));
    }
}

/// The release being prepared, if any, and the one waiting for the person's agreement.
pub(crate) struct Updater {
    prepared: Arc<Mutex<Option<Prepared>>>,
    cancel: Arc<AtomicBool>,
}

impl Default for Updater {
    fn default() -> Self {
        Self {
            prepared: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Updater {
    /// Asks which release, if any, is newer than this one.
    pub(crate) fn check(&self, channel: Channel) {
        spawn(
            "spooky-update-check",
            move || {
                let result = update::check(&Source::official(), channel, CURRENT, &|| false)
                    .map_err(|error| format!("{error:#}"));
                send(Event::Checked(channel, result));
            },
            move |reason| send(Event::Checked(channel, Err(reason))),
        );
    }

    /// Downloads and checks `available`, then waits for [`Updater::apply`]. Only call this
    /// after the person asked for the update.
    pub(crate) fn prepare(&mut self, available: &Available, install: Install) {
        self.discard();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Arc::clone(&cancel);
        let slot = Arc::clone(&self.prepared);
        let version = available.version.clone();
        spawn(
            "spooky-update-prepare",
            move || {
                let cancelled = || cancel.load(Ordering::Acquire);
                let result = prepare(
                    &Source::official(),
                    &version,
                    &install,
                    &mut |progress| send(Event::Progress(progress)),
                    &cancelled,
                );
                let prepared = match result {
                    Ok(prepared) => prepared,
                    Err(error) if was_cancelled(&error) => return,
                    Err(error) => {
                        send(Event::Prepared(Err(format!("{error:#}"))));
                        return;
                    }
                };
                // A new server that speaks another protocol cannot work with the one
                // running: say how many shells stopping it ends, before anything changes.
                let restart_server = prepared.protocol() != PROTOCOL_VERSION;
                let sessions = if restart_server {
                    match local_sessions() {
                        Ok(sessions) => sessions,
                        Err(error) => {
                            send(Event::Prepared(Err(format!(
                                "Cannot ask the running server how many shells it holds: {error:#}"
                            ))));
                            return;
                        }
                    }
                } else {
                    None
                };
                if cancelled() {
                    return;
                }
                if let Ok(mut slot) = slot.lock() {
                    *slot = Some(prepared);
                    send(Event::Prepared(Ok(Ready {
                        restart_server: sessions.is_some(),
                        sessions: sessions.unwrap_or(0),
                    })));
                }
            },
            |reason| send(Event::Prepared(Err(reason))),
        );
    }

    /// Abandons a download in progress and removes a prepared release.
    pub(crate) fn discard(&self) {
        self.cancel.store(true, Ordering::Release);
        if let Ok(mut slot) = self.prepared.lock() {
            slot.take();
        }
    }

    /// Replaces the installation with the prepared release and starts it, stopping the
    /// local server first when `stop_server` says its protocol no longer fits. Only call
    /// this after the person agreed: stopping the server ends its shells and agents.
    pub(crate) fn apply(&self, stop_server: bool) {
        let slot = Arc::clone(&self.prepared);
        spawn(
            "spooky-update-apply",
            move || {
                let result = (|| -> anyhow::Result<()> {
                    let prepared = slot
                        .lock()
                        .ok()
                        .and_then(|mut slot| slot.take())
                        .ok_or_else(|| anyhow::anyhow!("there is no prepared update"))?;
                    let applied = apply(prepared)?;
                    if stop_server {
                        stop_local_server().map_err(|error| {
                            error.context(
                                "The new version is installed, but the old server did not stop. \
                                 End the spooky-server process and open Spooky again",
                            )
                        })?;
                    }
                    relaunch(&applied)
                })()
                .map_err(|error| format!("{error:#}"));
                send(Event::Applied(result));
            },
            |reason| send(Event::Applied(Err(reason))),
        );
    }
}
