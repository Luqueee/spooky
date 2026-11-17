//! Updating the application: what is known about newer releases, and the dialog that
//! offers one and shows how it goes.
//!
//! The work itself (asking for the release list, downloading, replacing the files) runs
//! on background threads (see [`crate::updater`]); this module keeps their results as
//! plain data and decides what the person is asked, so it needs no window to test. Nothing
//! about the installation changes before the person confirms, and a server that holds
//! shells is never stopped without saying how many it ends.

use spooky_core::{
    client::Progress,
    update::{Available, Channel, Support},
};

/// What a prepared update needs before it is applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Ready {
    /// The new server speaks another protocol than the running one, so the server that
    /// is running must be stopped for the new application to work.
    pub restart_server: bool,
    /// Shells and agents that stopping it ends.
    pub sessions: u32,
}

/// What a background thread reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// The check for a newer release ended.
    Checked(Channel, Result<Option<Available>, String>),
    /// The download of the release is under way.
    Progress(Progress),
    /// The release is downloaded, checked and unpacked, or why it could not be.
    Prepared(Result<Ready, String>),
    /// The installation was replaced and the new application started, or why not.
    Applied(Result<(), String>),
}

/// What is known about newer releases.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Nothing asked yet (or checking is off).
    Idle,
    Checking,
    UpToDate,
    Available(Available),
    Failed(String),
}

/// What the update has done so far, for the progress panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Run {
    pub steps: Vec<String>,
    pub downloaded: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Asking whether to update.
    Ask,
    Running(Run),
    /// Downloaded and checked: the application restarts when the person agrees.
    Ready(Ready),
    Applying,
    Failed(String),
}

/// The update dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dialog {
    pub available: Available,
    pub stage: Stage,
}

/// What the application must do after an event.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Effects {
    /// The new application is running: this one exits.
    pub exit: bool,
}

pub(crate) struct Updates {
    status: Status,
    /// The channel the running check, if any, asks about.
    checking: Channel,
    support: Support,
    dialog: Option<Dialog>,
}

impl Updates {
    pub(crate) fn new(support: Support) -> Self {
        Self {
            status: Status::Idle,
            checking: Channel::default(),
            support,
            dialog: None,
        }
    }

    pub(crate) fn status(&self) -> &Status {
        &self.status
    }

    /// Whether this installation can replace itself, or why not.
    pub(crate) fn support(&self) -> &Support {
        &self.support
    }

    pub(crate) fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// The newer release known, if any.
    pub(crate) fn available(&self) -> Option<&Available> {
        match &self.status {
            Status::Available(available) => Some(available),
            _ => None,
        }
    }

    /// Marks a check of `channel` as running; `false` when one of it already is. A check
    /// of another channel that is still running is forgotten: its answer is ignored.
    pub(crate) fn start_check(&mut self, channel: Channel) -> bool {
        if self.status == Status::Checking && self.checking == channel {
            return false;
        }
        self.status = Status::Checking;
        self.checking = channel;
        true
    }

    /// Opens the dialog for the release known, unless it is already open.
    pub(crate) fn ask(&mut self) {
        if self.dialog.is_none()
            && let Some(available) = self.available().cloned()
        {
            self.dialog = Some(Dialog {
                available,
                stage: Stage::Ask,
            });
        }
    }

    /// Starts downloading, from the question or after a failure. Returns the release to
    /// prepare; nothing starts when this installation cannot replace itself.
    pub(crate) fn start(&mut self) -> Option<Available> {
        if !matches!(self.support, Support::Update(_)) {
            return None;
        }
        let dialog = self.dialog.as_mut()?;
        if !matches!(dialog.stage, Stage::Ask | Stage::Failed(_)) {
            return None;
        }
        dialog.stage = Stage::Running(Run::default());
        Some(dialog.available.clone())
    }

    /// The person agreed to restart. Whether the running server must be stopped first.
    pub(crate) fn confirm(&mut self) -> Option<bool> {
        let dialog = self.dialog.as_mut()?;
        let Stage::Ready(ready) = &dialog.stage else {
            return None;
        };
        let stop_server = ready.restart_server;
        dialog.stage = Stage::Applying;
        Some(stop_server)
    }

    /// Whether closing the dialog now would abandon work that is running or prepared.
    pub(crate) fn busy(&self) -> bool {
        self.dialog
            .as_ref()
            .is_some_and(|dialog| matches!(dialog.stage, Stage::Running(_) | Stage::Ready(_)))
    }

    /// Closes the dialog, but not while the files are being replaced.
    pub(crate) fn close(&mut self) {
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.stage == Stage::Applying)
        {
            return;
        }
        self.dialog = None;
    }

    pub(crate) fn apply(&mut self, event: Event) -> Effects {
        let mut effects = Effects::default();
        match event {
            Event::Checked(channel, result) => {
                if self.status == Status::Checking && channel == self.checking {
                    self.status = match result {
                        Ok(Some(available)) => Status::Available(available),
                        Ok(None) => Status::UpToDate,
                        Err(reason) => Status::Failed(reason),
                    };
                }
            }
            Event::Progress(progress) => {
                if let Some(Dialog {
                    stage: Stage::Running(run),
                    ..
                }) = &mut self.dialog
                {
                    match progress {
                        Progress::Step(step) => run.steps.push(step),
                        Progress::Downloaded(bytes) => run.downloaded = bytes,
                        Progress::Uploaded { .. } => {}
                    }
                }
            }
            Event::Prepared(result) => {
                if let Some(dialog) = &mut self.dialog
                    && matches!(dialog.stage, Stage::Running(_))
                {
                    dialog.stage = match result {
                        Ok(ready) => Stage::Ready(ready),
                        Err(reason) => Stage::Failed(reason),
                    };
                }
            }
            Event::Applied(result) => match result {
                Ok(()) => effects.exit = true,
                Err(reason) => {
                    if let Some(dialog) = &mut self.dialog {
                        dialog.stage = Stage::Failed(reason);
                    }
                }
            },
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spooky_core::update::Install;
    use std::path::PathBuf;

    fn updatable() -> Updates {
        Updates::new(Support::Update(Install::Binaries(PathBuf::from(
            "/opt/bin",
        ))))
    }

    fn found(updates: &mut Updates) {
        updates.start_check(Channel::Stable);
        updates.apply(Event::Checked(
            Channel::Stable,
            Ok(Some(Available {
                version: "0.2.0".into(),
                prerelease: false,
            })),
        ));
    }

    #[test]
    fn a_check_reports_a_newer_release_nothing_new_or_why_it_failed() {
        let mut updates = updatable();
        assert!(updates.start_check(Channel::Stable));
        assert!(!updates.start_check(Channel::Stable), "one check at a time");
        found(&mut updates);
        assert_eq!(
            updates.available().map(|a| a.version.as_str()),
            Some("0.2.0")
        );
        updates.start_check(Channel::Stable);
        updates.apply(Event::Checked(Channel::Stable, Ok(None)));
        assert_eq!(updates.status(), &Status::UpToDate);
        updates.start_check(Channel::Stable);
        updates.apply(Event::Checked(Channel::Stable, Err("offline".into())));
        assert_eq!(updates.status(), &Status::Failed("offline".into()));
        assert!(updates.available().is_none());
    }

    #[test]
    fn the_answer_for_a_channel_that_was_left_is_ignored() {
        let mut updates = updatable();
        updates.start_check(Channel::Stable);
        // The person switches channel while the first check is still running.
        assert!(
            updates.start_check(Channel::Dev),
            "the other channel is asked"
        );
        updates.apply(Event::Checked(
            Channel::Stable,
            Ok(Some(Available {
                version: "0.2.0".into(),
                prerelease: false,
            })),
        ));
        assert_eq!(updates.status(), &Status::Checking);
        updates.apply(Event::Checked(Channel::Dev, Ok(None)));
        assert_eq!(updates.status(), &Status::UpToDate);
    }

    #[test]
    fn nothing_downloads_until_the_person_asks_and_nothing_is_replaced_until_they_agree() {
        let mut updates = updatable();
        assert_eq!(updates.start(), None, "no dialog, nothing to start");
        found(&mut updates);
        updates.ask();
        assert_eq!(updates.dialog().map(|d| &d.stage), Some(&Stage::Ask));
        assert_eq!(updates.confirm(), None, "nothing prepared to apply");

        assert_eq!(updates.start().map(|a| a.version), Some("0.2.0".into()));
        assert_eq!(updates.start(), None, "already running");
        updates.apply(Event::Progress(Progress::Step("Downloading".into())));
        updates.apply(Event::Progress(Progress::Downloaded(1_000_000)));
        assert!(matches!(
            updates.dialog().map(|d| &d.stage),
            Some(Stage::Running(run)) if run.steps == ["Downloading"] && run.downloaded == 1_000_000
        ));
        assert!(updates.busy());

        updates.apply(Event::Prepared(Ok(Ready {
            restart_server: true,
            sessions: 3,
        })));
        assert!(updates.busy(), "prepared files are discarded on close");
        assert_eq!(updates.confirm(), Some(true), "the server must be stopped");
        assert_eq!(updates.dialog().map(|d| &d.stage), Some(&Stage::Applying));
        updates.close();
        assert!(updates.dialog().is_some(), "not while files are replaced");
        assert_eq!(updates.confirm(), None);

        let effects = updates.apply(Event::Applied(Ok(())));
        assert!(effects.exit);
    }

    #[test]
    fn a_failure_can_be_tried_again_from_the_start() {
        let mut updates = updatable();
        found(&mut updates);
        updates.ask();
        updates.start();
        updates.apply(Event::Prepared(Err("checksum".into())));
        assert_eq!(
            updates.dialog().map(|d| &d.stage),
            Some(&Stage::Failed("checksum".into()))
        );
        assert!(!updates.busy());
        assert!(updates.start().is_some());
        updates.apply(Event::Prepared(Ok(Ready {
            restart_server: false,
            sessions: 0,
        })));
        updates.confirm();
        let effects = updates.apply(Event::Applied(Err("disk full".into())));
        assert!(!effects.exit);
        assert_eq!(
            updates.dialog().map(|d| &d.stage),
            Some(&Stage::Failed("disk full".into()))
        );
        updates.close();
        assert!(updates.dialog().is_none());
    }

    #[test]
    fn an_installation_that_cannot_replace_itself_only_explains() {
        let mut updates = Updates::new(Support::Manual("installed by apt".into()));
        found(&mut updates);
        updates.ask();
        assert!(updates.dialog().is_some());
        assert_eq!(updates.start(), None);
    }

    #[test]
    fn a_late_result_after_the_dialog_closed_changes_nothing() {
        let mut updates = updatable();
        found(&mut updates);
        updates.ask();
        updates.start();
        updates.close();
        updates.apply(Event::Prepared(Ok(Ready {
            restart_server: false,
            sessions: 0,
        })));
        updates.apply(Event::Progress(Progress::Downloaded(5)));
        assert!(updates.dialog().is_none());
    }
}
