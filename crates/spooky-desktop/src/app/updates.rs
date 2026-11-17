//! Looking for newer releases of the application and updating it: starting the
//! background work and carrying out what it reports. Nothing is downloaded until the
//! person asks, and nothing is replaced until they agree to restart.

use super::{App, Message};
use crate::runtime::Task;
use spooky_core::update::Support;

impl App {
    pub(super) fn updates_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Update(event) => {
                if self.updates.apply(event).exit {
                    // The new application was started; this one makes way for it. The
                    // server and its shells are not part of this process.
                    return Task::exit();
                }
            }
            Message::UpdateCheck => self.updates_check(),
            Message::UpdateAsk => self.updates.ask(),
            Message::UpdateGo => {
                if let Some(available) = self.updates.start()
                    && let Support::Update(install) = self.updates.support().clone()
                {
                    self.updater.prepare(&available, install);
                }
            }
            Message::UpdateRestart => {
                if let Some(stop_server) = self.updates.confirm() {
                    self.updater.apply(stop_server);
                }
            }
            Message::UpdateClose => {
                if self.updates.busy() {
                    self.updater.discard();
                }
                self.updates.close();
            }
            _ => {}
        }
        Task::none()
    }

    /// Asks which release of the configured channel is newer than this one.
    pub(super) fn updates_check(&mut self) {
        let channel = self.config.update_channel;
        if self.updates.start_check(channel) {
            self.updater.check(channel);
        }
    }
}
