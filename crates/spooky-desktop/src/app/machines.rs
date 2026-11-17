//! Machines besides the one in use: adding and removing them, switching to one, and
//! installing the server on one that lacks it. The machine in use keeps the connection
//! the rest of the application works with; the others are watched by probes.

use super::{App, Message};
use crate::runtime::Task;
use crate::{features::machines::State, probe};

impl App {
    pub(super) fn machines_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Machine(event) => {
                let effects = self.machines.apply(event);
                if let Some(id) = effects.reconnect_probe {
                    self.reprobe(id);
                }
            }
            Message::AddMachine => {
                let destination = self.connection.target().trim().to_owned();
                if destination.is_empty() {
                    self.status = "Type a host or user@host to add".into();
                    return Task::none();
                }
                if let Err(error) = spooky_core::client::check_destination(&destination) {
                    self.status = format!("{error:#}");
                    return Task::none();
                }
                self.tailscale.close();
                self.connection.use_local();
                self.connection_open = false;
                if let Some(id) = self.machines.add(&destination) {
                    self.save_machines();
                    self.sync_probes();
                    self.status = format!("Connecting to {destination} in the background");
                    let _ = id;
                }
            }
            Message::ActivateMachine(id) => return self.activate_machine(id),
            Message::ToggleMachine(id) => self.machines.toggle(id),
            Message::RemoveMachine(id) => {
                if self.machines.get(id).is_some_and(|machine| {
                    machine.destination != self.connection.connected_target()
                }) {
                    self.probes.stop(id);
                    self.machines.remove(id);
                    self.save_machines();
                }
            }
            Message::RetryMachine(id) => self.reprobe(id),
            Message::InstallAsk(id) => {
                self.machines.ask_to_install(id);
            }
            Message::InstallGo => {
                if let Some(id) = self.machines.start_install()
                    && let Some(destination) = self.destination(id)
                {
                    self.installation = Some(probe::start_install(id, destination));
                }
            }
            Message::InstallCancel => {
                if let Some(installation) = &self.installation {
                    installation.cancel();
                }
            }
            Message::RestartGo => {
                if let Some(id) = self.machines.start_restart()
                    && let Some(destination) = self.destination(id)
                {
                    probe::start_restart(id, destination);
                }
            }
            Message::InstallClose => {
                if let Some(installation) = self.installation.take() {
                    installation.cancel();
                }
                self.machines.close_dialog();
            }
            _ => {}
        }
        Task::none()
    }

    fn destination(&self, id: u64) -> Option<String> {
        self.machines
            .get(id)
            .map(|machine| machine.destination.clone())
    }

    /// Switches to a machine that answers; one that does not is explained, and when
    /// installing the server there would fix it, offered.
    fn activate_machine(&mut self, id: u64) -> Task<Message> {
        let Some(machine) = self.machines.get(id) else {
            return Task::none();
        };
        if machine.destination == self.connection.connected_target() {
            return Task::none();
        }
        let destination = machine.destination.clone();
        match &machine.state {
            State::Ready(_) => {
                self.connection.set_target(destination);
                Task::done(Message::Connect)
            }
            State::Failed(failure) if failure.problem.is_some() => {
                self.machines.ask_to_install(id);
                Task::none()
            }
            State::Failed(failure) => {
                self.status = format!("{}: {}", machine.label(), failure.reason);
                self.reprobe(id);
                Task::none()
            }
            State::Connecting => {
                self.status = format!("Still connecting to {}", machine.label());
                Task::none()
            }
        }
    }

    /// Tries a machine again now.
    fn reprobe(&mut self, id: u64) {
        let Some(destination) = self.destination(id) else {
            return;
        };
        if destination == self.connection.connected_target() {
            return;
        }
        self.machines.connecting(id);
        self.probes.poke(id, &destination);
    }

    /// Every machine but the one in use is watched; the one in use has the real
    /// connection.
    pub(super) fn sync_probes(&mut self) {
        let active = self.connection.connected_target().to_owned();
        let machines: Vec<(u64, String)> = self
            .machines
            .list()
            .iter()
            .map(|machine| (machine.id, machine.destination.clone()))
            .collect();
        for (id, destination) in machines {
            if destination == active {
                self.probes.stop(id);
            } else {
                self.probes.start(id, &destination);
            }
        }
    }

    fn save_machines(&mut self) {
        self.config.machines = self.machines.destinations();
        if let Err(error) = self.config.save() {
            self.status = format!("The machines were not saved: {error:#}");
        }
    }
}
