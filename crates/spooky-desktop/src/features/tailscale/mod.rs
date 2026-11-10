//! The Tailscale device picker: asks the installed Tailscale client for devices and
//! prepares an SSH destination from the one the user picks. It never connects; that
//! stays with the Connect action.

mod discovery;
mod view;

pub(crate) use discovery::Network;
pub(crate) use view::view;

use discovery::{Discovery, discover};
use iced::Task;

/// What the rest of the application tells the picker, or the picker tells itself.
#[derive(Clone, Debug)]
pub(crate) enum Message {
    /// Show the picker, reading the devices if they are not being read already.
    Open,
    /// Hide the picker.
    Close,
    /// Read the devices again.
    Refresh,
    /// The client answered.
    Loaded(Result<Network, String>),
    /// The search text changed.
    Query(String),
    /// The SSH user text changed.
    User(String),
    /// The user picked the device with this host.
    Select(String),
}

/// What a message decided that the application must act on.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// An SSH destination was prepared (`user@host` or `host`).
    Chosen(String),
    /// The choice was refused; the text says why.
    Refused(String),
}

/// The picker's state.
#[derive(Default)]
pub(crate) struct Picker {
    open: bool,
    discovery: Discovery,
    query: String,
    user: String,
}

/// Whether `user` may be put in front of the host: empty, or letters, digits, dot,
/// underscore and hyphen without a leading hyphen (which `ssh` would read as an option).
fn user_is_valid(user: &str) -> bool {
    user.is_empty()
        || (!user.starts_with('-')
            && user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
}

impl Picker {
    /// Whether the picker is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Hides the picker.
    pub(crate) fn close(&mut self) {
        self.open = false;
    }

    /// Applies a message; a follow-up task and a decision for the application, if any.
    pub(crate) fn update(&mut self, message: Message) -> (Task<Message>, Option<Outcome>) {
        match message {
            Message::Open => {
                self.open = true;
                if !matches!(self.discovery, Discovery::Loading) {
                    return (Task::done(Message::Refresh), None);
                }
            }
            Message::Close => self.open = false,
            Message::Refresh => {
                if !matches!(self.discovery, Discovery::Loading) {
                    self.discovery = Discovery::Loading;
                    return (Task::perform(discover(), Message::Loaded), None);
                }
            }
            Message::Loaded(result) => {
                self.discovery = match result {
                    Ok(network) => Discovery::Ready(network),
                    Err(error) => Discovery::Failed(error),
                };
            }
            Message::Query(query) => self.query = query,
            Message::User(user) => self.user = user,
            Message::Select(host) => return (Task::none(), self.select(host)),
        }
        (Task::none(), None)
    }

    /// The destination for `host`, if it is an online device the client listed.
    fn select(&mut self, host: String) -> Option<Outcome> {
        let listed = matches!(&self.discovery, Discovery::Ready(network)
            if network.peers.iter().any(|peer| peer.host == host && peer.online));
        if !listed {
            return None;
        }
        let user = self.user.trim();
        if !user_is_valid(user) {
            return Some(Outcome::Refused(
                "SSH user must contain only letters, numbers, dot, underscore or hyphen.".into(),
            ));
        }
        let destination = if user.is_empty() {
            host
        } else {
            format!("{user}@{host}")
        };
        self.open = false;
        Some(Outcome::Chosen(destination))
    }
}

#[cfg(test)]
mod tests {
    use super::{discovery::Peer, *};

    fn picker_with(user: &str) -> Picker {
        let peer = |host: &str, online| Peer {
            name: host.into(),
            host: host.into(),
            online,
            os: "linux".into(),
        };
        Picker {
            open: true,
            discovery: Discovery::Ready(Network {
                peers: vec![peer("100.64.0.1", true), peer("100.64.0.2", false)],
            }),
            query: String::new(),
            user: user.into(),
        }
    }

    #[test]
    fn picking_an_online_device_prepares_the_destination_and_closes_the_picker() {
        let mut picker = picker_with("me");
        let (_, outcome) = picker.update(Message::Select("100.64.0.1".into()));
        assert_eq!(outcome, Some(Outcome::Chosen("me@100.64.0.1".into())));
        assert!(!picker.is_open());
        let mut picker = picker_with("");
        let (_, outcome) = picker.update(Message::Select("100.64.0.1".into()));
        assert_eq!(outcome, Some(Outcome::Chosen("100.64.0.1".into())));
    }

    #[test]
    fn only_devices_the_client_listed_as_online_can_be_picked() {
        let mut picker = picker_with("");
        for host in ["100.64.0.2", "100.64.0.9", "evil.example"] {
            let (_, outcome) = picker.update(Message::Select(host.into()));
            assert_eq!(outcome, None, "{host}");
            assert!(picker.is_open());
        }
    }

    #[test]
    fn a_user_that_ssh_could_read_as_an_option_is_refused_and_the_picker_stays() {
        for user in ["-oProxyCommand=x", "a b", "me@evil", "a/b"] {
            let mut picker = picker_with(user);
            let (_, outcome) = picker.update(Message::Select("100.64.0.1".into()));
            assert!(matches!(outcome, Some(Outcome::Refused(_))), "{user:?}");
            assert!(picker.is_open());
        }
    }

    #[test]
    fn reading_devices_moves_through_loading_to_the_answer() {
        let mut picker = Picker::default();
        // The tasks a message returns belong to the runtime; the state is what is checked.
        let _ = picker.update(Message::Refresh);
        assert!(matches!(picker.discovery, Discovery::Loading));
        let _ = picker.update(Message::Loaded(Err("not running".into())));
        assert!(matches!(&picker.discovery, Discovery::Failed(error) if error == "not running"));
        let _ = picker.update(Message::Loaded(Ok(Network { peers: Vec::new() })));
        assert!(matches!(picker.discovery, Discovery::Ready(_)));
    }
}
