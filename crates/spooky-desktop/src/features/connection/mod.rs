//! Which server the application talks to and how that connection is doing: the
//! target being edited and the one in use, the generation that tells current events
//! from stale ones, the two channels' state, and why a target cannot be reached.
//!
//! The threads themselves live in [`crate::link`]; this keeps what the interface shows
//! about them, so the rules can be tested without a server.

use crate::link::Channel;

/// Why the command connection cannot be opened, and whether it is being retried.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Unreachable {
    reason: String,
    retrying: bool,
}

/// What a channel changing state asks the application to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ChannelUpdate {
    /// A line for the status bar.
    pub status: Option<String>,
    /// The display connection came (back) up: a restarted server reuses pane ids, so
    /// what was sent or seen for the previous one says nothing about its panes.
    pub forget_panes: bool,
}

/// The connection to the server in use.
#[derive(Default)]
pub(crate) struct Connection {
    /// The target being edited: empty for this computer, else `user@host` or an alias.
    target: String,
    /// The target in use.
    connected: String,
    /// Events carry the generation they belong to; only the current one counts.
    generation: u64,
    commands_online: bool,
    display_online: bool,
    unreachable: Option<Unreachable>,
    /// A first workspace has been requested for this connection.
    initialized: bool,
}

impl Connection {
    /// The generation events must carry to be heard.
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    /// The target text in the editor.
    pub(crate) fn target(&self) -> &str {
        &self.target
    }

    pub(crate) fn set_target(&mut self, target: String) {
        self.target = target;
    }

    /// Edits the target back to this computer.
    pub(crate) fn use_local(&mut self) {
        self.target.clear();
    }

    /// Edits the target back to the one in use, to try it again.
    pub(crate) fn use_current(&mut self) {
        self.target = self.connected.clone();
    }

    /// The target in use: empty for this computer.
    pub(crate) fn connected_target(&self) -> &str {
        &self.connected
    }

    /// Whether the target in use is another machine.
    pub(crate) fn is_remote(&self) -> bool {
        !self.connected.is_empty()
    }

    /// Typing and live output need both the command and the display connection.
    pub(crate) fn online(&self) -> bool {
        self.commands_online && self.display_online
    }

    /// Why the target cannot be reached, and whether the attempt is being retried.
    pub(crate) fn unreachable(&self) -> Option<(&str, bool)> {
        self.unreachable
            .as_ref()
            .map(|unreachable| (unreachable.reason.as_str(), unreachable.retrying))
    }

    /// Switches to the target in the editor: a new generation, everything the old one
    /// reported forgotten. Returns the generation and target to hand to the link.
    pub(crate) fn reconnect(&mut self) -> (u64, String) {
        self.target = self.target.trim().to_owned();
        self.connected = self.target.clone();
        self.generation += 1;
        self.initialized = false;
        self.commands_online = false;
        self.display_online = false;
        self.unreachable = None;
        (self.generation, self.connected.clone())
    }

    /// A status line from the connection threads. While a target is unreachable the
    /// failure card owns the explanation, so the display thread's echo of it is dropped.
    pub(crate) fn status(&self, line: String) -> Option<String> {
        self.unreachable.is_none().then_some(line)
    }

    /// The command connection could not be opened; the status line to show.
    pub(crate) fn failed_to_connect(&mut self, reason: String, retrying: bool) -> String {
        self.unreachable = Some(Unreachable { reason, retrying });
        "Cannot connect".into()
    }

    /// One of the two channels went up or down.
    pub(crate) fn channel(&mut self, channel: Channel, online: bool) -> ChannelUpdate {
        let mut update = ChannelUpdate::default();
        match channel {
            Channel::Commands => {
                self.commands_online = online;
                if online {
                    self.unreachable = None;
                }
            }
            Channel::Display => {
                self.display_online = online;
                update.forget_panes = online;
            }
        }
        if online && self.online() {
            update.status = Some("Connected".into());
        }
        update
    }

    /// Whether the first snapshot of this connection is empty and so needs a first
    /// workspace. True once per connection.
    pub(crate) fn needs_first_workspace(&mut self, snapshot_is_empty: bool) -> bool {
        if snapshot_is_empty && !self.initialized {
            self.initialized = true;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnecting_starts_a_new_generation_and_forgets_the_old_state() {
        let mut connection = Connection::default();
        connection.set_target("  me@host ".into());
        let (generation, target) = connection.reconnect();
        assert_eq!((generation, target.as_str()), (1, "me@host"));
        assert!(connection.is_remote());
        assert_eq!(connection.target(), "me@host");
        connection.channel(Channel::Commands, true);
        connection.channel(Channel::Display, true);
        connection.failed_to_connect("no route".into(), true);
        assert!(connection.online());
        connection.needs_first_workspace(true);
        connection.use_local();
        let (generation, target) = connection.reconnect();
        assert_eq!((generation, target.as_str()), (2, ""));
        assert!(!connection.online() && !connection.is_remote());
        assert_eq!(connection.unreachable(), None);
        assert!(
            connection.needs_first_workspace(true),
            "a new connection may ask for its own first workspace"
        );
    }

    #[test]
    fn retrying_puts_the_target_in_use_back_in_the_editor() {
        let mut connection = Connection::default();
        connection.set_target("me@host".into());
        connection.reconnect();
        connection.use_local();
        assert_eq!(connection.target(), "");
        connection.use_current();
        assert_eq!(connection.target(), "me@host");
    }

    #[test]
    fn online_needs_both_channels_and_connected_is_said_once_when_both_are_up() {
        let mut connection = Connection::default();
        let first = connection.channel(Channel::Commands, true);
        assert_eq!(first.status, None, "the display is still down");
        assert!(!connection.online());
        let second = connection.channel(Channel::Display, true);
        assert_eq!(second.status.as_deref(), Some("Connected"));
        assert!(
            second.forget_panes,
            "a display that came up has new pane state"
        );
        let down = connection.channel(Channel::Display, false);
        assert_eq!(down, ChannelUpdate::default());
        assert!(!connection.online());
    }

    #[test]
    fn the_failure_card_owns_the_explanation_until_the_commands_channel_recovers() {
        let mut connection = Connection::default();
        assert_eq!(
            connection.status("Connecting".into()).as_deref(),
            Some("Connecting")
        );
        let line = connection.failed_to_connect("refused".into(), true);
        assert_eq!(line, "Cannot connect");
        assert_eq!(connection.unreachable(), Some(("refused", true)));
        assert_eq!(connection.status("echo".into()), None);
        // The display channel coming up does not clear it; the commands channel does.
        connection.channel(Channel::Display, true);
        assert!(connection.unreachable().is_some());
        connection.channel(Channel::Commands, true);
        assert_eq!(connection.unreachable(), None);
        assert!(connection.status("echo".into()).is_some());
    }

    #[test]
    fn only_the_first_empty_snapshot_of_a_connection_asks_for_a_workspace() {
        let mut connection = Connection::default();
        assert!(!connection.needs_first_workspace(false));
        assert!(connection.needs_first_workspace(true));
        assert!(!connection.needs_first_workspace(true));
    }
}
