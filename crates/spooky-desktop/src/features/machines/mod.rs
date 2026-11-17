//! The machines Spooky knows: this computer and the SSH destinations the person added.
//!
//! Only one machine is shown at a time, but the others stay connected through light
//! background probes (see [`crate::probe`]) that report how each one is doing and what
//! workspaces it holds. A machine that fails, or that lacks the server, never takes the
//! machine in use away: this module keeps their states and the installation dialog, and
//! tells the rest of the application what changed through plain data.

use spooky_core::{
    Snapshot,
    client::{Outcome, Problem, Progress},
};

/// The machine in use is the first one: this computer, whose destination is empty.
pub(crate) const LOCAL: u64 = 0;

/// What the sidebar shows of a workspace on a machine that is not in use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WorkspaceLine {
    pub id: u64,
    pub name: String,
    pub cwd: String,
    pub tabs: usize,
}

/// What a probe reports of a machine's server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub workspaces: Vec<WorkspaceLine>,
}

impl Summary {
    pub(crate) fn of(snapshot: &Snapshot) -> Self {
        Self {
            workspaces: snapshot
                .workspaces
                .iter()
                .map(|workspace| WorkspaceLine {
                    id: workspace.id,
                    name: workspace.name.clone(),
                    cwd: workspace.cwd.clone(),
                    tabs: workspace.tabs.len(),
                })
                .collect(),
        }
    }
}

/// Why a machine cannot be used, and whether something can be done about it here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Failure {
    pub reason: String,
    /// What the machine lacks, when installing or updating the server would fix it.
    pub problem: Option<Problem>,
    /// Whether the probe is trying again by itself.
    pub retrying: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum State {
    Connecting,
    Ready(Summary),
    Failed(Failure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Machine {
    pub id: u64,
    /// `user@host` or an SSH alias; empty for this computer.
    pub destination: String,
    pub state: State,
    /// Whether its workspaces are listed in the sidebar.
    pub open: bool,
}

impl Machine {
    pub(crate) fn is_local(&self) -> bool {
        self.destination.is_empty()
    }

    /// The name the sidebar shows.
    pub(crate) fn label(&self) -> &str {
        if self.is_local() {
            "This computer"
        } else {
            &self.destination
        }
    }
}

/// What a probe sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Probe {
    Connecting,
    Summary(Summary),
    Failed(Failure),
}

/// What the installation thread sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Install {
    Progress(Progress),
    Finished(Result<Outcome, String>),
    Restarted(Result<(), String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    Probe(u64, Probe),
    Install(u64, Install),
}

/// What the installation has done so far, for the progress panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Run {
    pub steps: Vec<String>,
    pub downloaded: u64,
    pub sent: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Asking whether to install or update the server on the machine.
    Ask(Problem),
    Running(Run),
    /// Installed, but an old server is still running there: restarting it ends its
    /// shells and agents, so it is asked about separately.
    Restart {
        running_protocol: u32,
    },
    Restarting,
    Done,
    Failed(String),
}

/// The installation dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dialog {
    pub machine: u64,
    pub stage: Stage,
}

/// What the application must do after an event.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Effects {
    /// A machine is ready (again): its probe has nothing more to learn about a failure.
    pub reconnect_probe: Option<u64>,
}

pub(crate) struct Machines {
    list: Vec<Machine>,
    next_id: u64,
    dialog: Option<Dialog>,
}

impl Machines {
    /// This computer first, then `destinations`, skipping duplicates and empty ones.
    pub(crate) fn new(destinations: &[String]) -> Self {
        let mut machines = Self {
            list: vec![Machine {
                id: LOCAL,
                destination: String::new(),
                state: State::Connecting,
                open: true,
            }],
            next_id: 1,
            dialog: None,
        };
        for destination in destinations {
            machines.add(destination);
        }
        machines
    }

    pub(crate) fn list(&self) -> &[Machine] {
        &self.list
    }

    pub(crate) fn get(&self, id: u64) -> Option<&Machine> {
        self.list.iter().find(|machine| machine.id == id)
    }

    fn get_mut(&mut self, id: u64) -> Option<&mut Machine> {
        self.list.iter_mut().find(|machine| machine.id == id)
    }

    pub(crate) fn by_destination(&self, destination: &str) -> Option<&Machine> {
        self.list
            .iter()
            .find(|machine| machine.destination == destination)
    }

    /// Adds a machine, or returns the one that already has this destination.
    pub(crate) fn add(&mut self, destination: &str) -> Option<u64> {
        let destination = destination.trim();
        if destination.is_empty() {
            return None;
        }
        if let Some(existing) = self.by_destination(destination) {
            return Some(existing.id);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.list.push(Machine {
            id,
            destination: destination.to_owned(),
            state: State::Connecting,
            open: true,
        });
        Some(id)
    }

    /// Forgets a machine. This computer cannot be removed.
    pub(crate) fn remove(&mut self, id: u64) -> bool {
        if id == LOCAL {
            return false;
        }
        let before = self.list.len();
        self.list.retain(|machine| machine.id != id);
        if self
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.machine == id)
        {
            self.dialog = None;
        }
        self.list.len() != before
    }

    /// The destinations to save: the added machines, in order.
    pub(crate) fn destinations(&self) -> Vec<String> {
        self.list
            .iter()
            .filter(|machine| !machine.is_local())
            .map(|machine| machine.destination.clone())
            .collect()
    }

    pub(crate) fn toggle(&mut self, id: u64) {
        if let Some(machine) = self.get_mut(id) {
            machine.open = !machine.open;
        }
    }

    pub(crate) fn connecting(&mut self, id: u64) {
        if let Some(machine) = self.get_mut(id) {
            machine.state = State::Connecting;
        }
    }

    pub(crate) fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref()
    }

    /// Opens the dialog for a machine whose failure says what it lacks.
    pub(crate) fn ask_to_install(&mut self, id: u64) -> bool {
        let problem = self.get(id).and_then(|machine| match &machine.state {
            State::Failed(failure) => failure.problem,
            _ => None,
        });
        match problem {
            Some(problem) => {
                self.dialog = Some(Dialog {
                    machine: id,
                    stage: Stage::Ask(problem),
                });
                true
            }
            None => false,
        }
    }

    /// The person agreed: the installation starts. Returns the machine to install on.
    pub(crate) fn start_install(&mut self) -> Option<u64> {
        let dialog = self.dialog.as_mut()?;
        if !matches!(dialog.stage, Stage::Ask(_) | Stage::Failed(_)) {
            return None;
        }
        dialog.stage = Stage::Running(Run::default());
        Some(dialog.machine)
    }

    /// The person agreed to restart the old server.
    pub(crate) fn start_restart(&mut self) -> Option<u64> {
        let dialog = self.dialog.as_mut()?;
        if !matches!(dialog.stage, Stage::Restart { .. }) {
            return None;
        }
        dialog.stage = Stage::Restarting;
        Some(dialog.machine)
    }

    pub(crate) fn close_dialog(&mut self) {
        self.dialog = None;
    }

    /// Applies what a thread reported.
    pub(crate) fn apply(&mut self, event: Event) -> Effects {
        let mut effects = Effects::default();
        match event {
            Event::Probe(id, probe) => {
                if let Some(machine) = self.get_mut(id) {
                    machine.state = match probe {
                        Probe::Connecting => State::Connecting,
                        Probe::Summary(summary) => State::Ready(summary),
                        Probe::Failed(failure) => State::Failed(failure),
                    };
                }
            }
            Event::Install(id, install) => {
                let Some(dialog) = self.dialog.as_mut().filter(|dialog| dialog.machine == id)
                else {
                    return effects;
                };
                match (install, &mut dialog.stage) {
                    (Install::Progress(progress), Stage::Running(run)) => match progress {
                        Progress::Step(step) => run.steps.push(step),
                        Progress::Downloaded(bytes) => run.downloaded = bytes,
                        Progress::Uploaded { sent, total } => run.sent = Some((sent, total)),
                    },
                    (Install::Finished(Ok(Outcome::Ready)), Stage::Running(_)) => {
                        dialog.stage = Stage::Done;
                        effects.reconnect_probe = Some(id);
                    }
                    (
                        Install::Finished(Ok(Outcome::NeedsRestart { running_protocol })),
                        Stage::Running(_),
                    ) => dialog.stage = Stage::Restart { running_protocol },
                    (Install::Finished(Err(message)), Stage::Running(_)) => {
                        dialog.stage = Stage::Failed(message);
                    }
                    (Install::Restarted(Ok(())), Stage::Restarting) => {
                        dialog.stage = Stage::Done;
                        effects.reconnect_probe = Some(id);
                    }
                    (Install::Restarted(Err(message)), Stage::Restarting) => {
                        dialog.stage = Stage::Failed(message);
                    }
                    _ => {}
                }
            }
        }
        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(problem: Option<Problem>) -> Event {
        Event::Probe(
            1,
            Probe::Failed(Failure {
                reason: "no server".into(),
                problem,
                retrying: false,
            }),
        )
    }

    #[test]
    fn this_computer_is_first_and_cannot_be_removed_or_duplicated() {
        let mut machines = Machines::new(&["devlabs".into(), " devlabs ".into(), "".into()]);
        assert_eq!(machines.list().len(), 2);
        assert!(machines.list()[0].is_local());
        assert_eq!(machines.list()[0].label(), "This computer");
        assert_eq!(
            machines.add("devlabs"),
            Some(1),
            "the same destination is the same machine"
        );
        assert_eq!(machines.add("  "), None);
        assert!(!machines.remove(LOCAL));
        assert!(machines.remove(1));
        assert_eq!(machines.destinations(), Vec::<String>::new());
    }

    #[test]
    fn a_failing_machine_changes_only_its_own_state() {
        let mut machines = Machines::new(&["devlabs".into()]);
        machines.apply(Event::Probe(LOCAL, Probe::Summary(Summary::default())));
        machines.apply(failed(None));
        assert!(matches!(
            machines.get(LOCAL).map(|m| &m.state),
            Some(State::Ready(_))
        ));
        assert!(matches!(
            machines.get(1).map(|m| &m.state),
            Some(State::Failed(_))
        ));
    }

    #[test]
    fn the_dialog_is_offered_only_for_what_installing_can_fix() {
        let mut machines = Machines::new(&["devlabs".into()]);
        machines.apply(failed(None));
        assert!(
            !machines.ask_to_install(1),
            "an unreachable host is not fixed by installing"
        );
        machines.apply(failed(Some(Problem::ServerMissing)));
        assert!(machines.ask_to_install(1));
        assert_eq!(
            machines.dialog().map(|d| d.stage.clone()),
            Some(Stage::Ask(Problem::ServerMissing))
        );
    }

    #[test]
    fn installing_walks_through_progress_to_done_and_reconnects_the_probe() {
        let mut machines = Machines::new(&["devlabs".into()]);
        machines.apply(failed(Some(Problem::ServerMissing)));
        machines.ask_to_install(1);
        assert_eq!(machines.start_install(), Some(1));
        assert_eq!(machines.start_install(), None, "already running");
        machines.apply(Event::Install(
            1,
            Install::Progress(Progress::Step("Checking".into())),
        ));
        machines.apply(Event::Install(
            1,
            Install::Progress(Progress::Downloaded(10)),
        ));
        machines.apply(Event::Install(
            1,
            Install::Progress(Progress::Uploaded { sent: 5, total: 9 }),
        ));
        let Some(Dialog {
            stage: Stage::Running(run),
            ..
        }) = machines.dialog().cloned()
        else {
            panic!("running");
        };
        assert_eq!(
            (run.steps.len(), run.downloaded, run.sent),
            (1, 10, Some((5, 9)))
        );
        let effects = machines.apply(Event::Install(1, Install::Finished(Ok(Outcome::Ready))));
        assert_eq!(effects.reconnect_probe, Some(1));
        assert_eq!(
            machines.dialog().map(|d| d.stage.clone()),
            Some(Stage::Done)
        );
    }

    #[test]
    fn an_old_server_still_running_asks_before_it_is_restarted() {
        let mut machines = Machines::new(&["devlabs".into()]);
        machines.apply(failed(Some(Problem::Incompatible {
            server: 10,
            client: 18,
        })));
        machines.ask_to_install(1);
        machines.start_install();
        machines.apply(Event::Install(
            1,
            Install::Finished(Ok(Outcome::NeedsRestart {
                running_protocol: 10,
            })),
        ));
        assert_eq!(machines.start_restart(), Some(1));
        let effects = machines.apply(Event::Install(1, Install::Restarted(Ok(()))));
        assert_eq!(effects.reconnect_probe, Some(1));
        assert_eq!(machines.start_restart(), None);
    }

    #[test]
    fn a_failed_installation_can_be_tried_again_and_events_for_other_machines_are_ignored() {
        let mut machines = Machines::new(&["a".into(), "b".into()]);
        machines.apply(Event::Probe(
            1,
            Probe::Failed(Failure {
                reason: String::new(),
                problem: Some(Problem::ServerMissing),
                retrying: false,
            }),
        ));
        machines.ask_to_install(1);
        machines.start_install();
        machines.apply(Event::Install(2, Install::Finished(Err("other".into()))));
        assert!(matches!(
            machines.dialog().map(|d| &d.stage),
            Some(Stage::Running(_))
        ));
        machines.apply(Event::Install(1, Install::Finished(Err("no route".into()))));
        assert_eq!(
            machines.start_install(),
            Some(1),
            "a failed attempt may be retried"
        );
    }

    #[test]
    fn removing_a_machine_closes_its_dialog() {
        let mut machines = Machines::new(&["devlabs".into()]);
        machines.apply(failed(Some(Problem::ServerMissing)));
        machines.ask_to_install(1);
        machines.remove(1);
        assert!(machines.dialog().is_none());
    }
}
