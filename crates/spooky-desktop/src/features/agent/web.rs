//! The messages between the agent view in a web page and the application.
//!
//! The page is Spooky's own bundle, but what it draws came from a model, so it is not
//! trusted to be well behaved: every message it posts is parsed into a closed set of
//! actions, bounded, and mapped to the same [`Message`]s the native view sends. Nothing the
//! page says reaches the server or the agent any other way.

use super::{Attachment, Message};
use crate::{commands::Command, shortcuts};
use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};
use serde::Deserialize;
use spooky_core::{Agent, DialogAnswer, MAX_AGENT_PROMPT_BYTES};

/// Largest message the page may post, in bytes: a prompt, or text to copy.
pub(crate) const MAX_ACTION_BYTES: usize = MAX_AGENT_PROMPT_BYTES + 4 * 1024;
/// Longest link the page may report, in bytes.
const MAX_LINK_BYTES: usize = 8 * 1024;
/// Longest key or code name of a forwarded key, in bytes.
const MAX_KEY_BYTES: usize = 32;
/// Longest model name or thinking level the page may name, in bytes.
const MAX_NAME_BYTES: usize = 512;

/// What the page asks for, as it posts it.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Action {
    Ready,
    Send {
        pane: u64,
        text: String,
    },
    Abort {
        pane: u64,
    },
    SetModel {
        pane: u64,
        model: String,
    },
    SetThinking {
        pane: u64,
        level: String,
    },
    Answer {
        pane: u64,
        dialog: String,
        answer: DialogAnswer,
    },
    Usage {
        pane: u64,
    },
    Unattach {
        pane: u64,
        named: String,
    },
    Copy {
        text: String,
    },
    Paste,
    Link {
        url: String,
        open: bool,
    },
    Key {
        key: String,
        code: String,
        shift: bool,
        alt: bool,
        ctrl: bool,
        meta: bool,
    },
}

/// What a page message means for the application.
#[derive(Debug)]
pub(crate) enum Event {
    /// The page has loaded and wants the current state.
    Ready,
    /// A message for the agent of a pane, which must be the pane on show.
    Agent { pane: u64, message: Message },
    /// Text to put on the clipboard.
    Copy(String),
    /// The page wants the clipboard's text.
    Paste,
    /// A link the user clicked, and whether they held the open modifier.
    Link { url: String, open: bool },
    /// A shortcut the page does not handle: the application's command for it.
    Command(Command),
    /// A forwarded key that is no shortcut; nothing to do.
    Ignored,
}

/// Reads one message posted by the page.
///
/// # Errors
///
/// Returns why the message is refused: too large, malformed, unknown, or out of bounds.
pub(crate) fn event(text: &str) -> Result<Event, String> {
    if text.len() > MAX_ACTION_BYTES {
        return Err("The agent view sent a message that is too large".into());
    }
    let action: Action = serde_json::from_str(text).map_err(|error| {
        format!("The agent view sent a message that is not understood: {error}")
    })?;
    match action {
        Action::Ready => Ok(Event::Ready),
        Action::Send { pane, text } => {
            if text.len() > MAX_AGENT_PROMPT_BYTES {
                return Err("The message is too long to send".into());
            }
            Ok(agent(pane, Message::Prompt(pane, text)))
        }
        Action::Abort { pane } => Ok(agent(pane, Message::Abort(pane))),
        Action::SetModel { pane, model } => {
            named_ok(&model)?;
            Ok(agent(pane, Message::SetModel(pane, model)))
        }
        Action::SetThinking { pane, level } => {
            named_ok(&level)?;
            Ok(agent(pane, Message::SetThinking(pane, level)))
        }
        Action::Answer {
            pane,
            dialog,
            answer,
        } => {
            named_ok(&dialog)?;
            if matches!(&answer, DialogAnswer::Value(v) if v.len() > MAX_AGENT_PROMPT_BYTES) {
                return Err("The answer is too long to send".into());
            }
            Ok(agent(pane, Message::Answer(pane, dialog, answer)))
        }
        Action::Usage { pane } => Ok(agent(pane, Message::RefreshUsage(pane))),
        Action::Unattach { pane, named } => {
            named_ok(&named)?;
            Ok(agent(pane, Message::Unattach(pane, named)))
        }
        Action::Copy { text } => Ok(Event::Copy(text)),
        Action::Paste => Ok(Event::Paste),
        Action::Link { url, open } => {
            if url.len() > MAX_LINK_BYTES {
                return Err("The link is too long".into());
            }
            Ok(Event::Link { url, open })
        }
        Action::Key {
            key,
            code,
            shift,
            alt,
            ctrl,
            meta,
        } => forwarded(&key, &code, shift, alt, ctrl, meta),
    }
}

fn agent(pane: u64, message: Message) -> Event {
    Event::Agent { pane, message }
}

fn named_ok(name: &str) -> Result<(), String> {
    if name.len() > MAX_NAME_BYTES {
        Err("A name sent by the agent view is too long".into())
    } else {
        Ok(())
    }
}

/// The application command for a chord the page forwarded; anything that is not a
/// shortcut is dropped, so the page cannot type into the application.
fn forwarded(
    key: &str,
    code: &str,
    shift: bool,
    alt: bool,
    ctrl: bool,
    meta: bool,
) -> Result<Event, String> {
    if key.len() > MAX_KEY_BYTES || code.len() > MAX_KEY_BYTES {
        return Err("A key name sent by the agent view is too long".into());
    }
    let mut mods = Modifiers::empty();
    mods.set(Modifiers::SHIFT, shift);
    mods.set(Modifiers::ALT, alt);
    mods.set(Modifiers::CTRL, ctrl);
    mods.set(Modifiers::LOGO, meta);
    let key = match key {
        "Tab" => Key::Named(Named::Tab),
        "ArrowUp" => Key::Named(Named::ArrowUp),
        "ArrowDown" => Key::Named(Named::ArrowDown),
        "PageUp" => Key::Named(Named::PageUp),
        "PageDown" => Key::Named(Named::PageDown),
        text if text.chars().count() == 1 => Key::Character(text.to_lowercase().as_str().into()),
        _ => return Ok(Event::Ignored),
    };
    Ok(shortcuts::shortcut(&key, physical(code), mods).map_or(Event::Ignored, Event::Command))
}

/// The physical keys shortcuts are matched by, from their `KeyboardEvent.code`.
fn physical(code: &str) -> Physical {
    const DIGITS: [Code; 9] = [
        Code::Digit1,
        Code::Digit2,
        Code::Digit3,
        Code::Digit4,
        Code::Digit5,
        Code::Digit6,
        Code::Digit7,
        Code::Digit8,
        Code::Digit9,
    ];
    let found = match code {
        "BracketLeft" => Some(Code::BracketLeft),
        "BracketRight" => Some(Code::BracketRight),
        "Comma" => Some(Code::Comma),
        other => other
            .strip_prefix("Digit")
            .and_then(|digit| digit.parse::<usize>().ok())
            .and_then(|digit| digit.checked_sub(1))
            .and_then(|index| DIGITS.get(index).copied()),
    };
    found.map_or(
        Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
        Physical::Code,
    )
}

/// An attached file as the page draws it.
#[derive(serde::Serialize)]
struct File<'a> {
    named: &'a str,
    name: &'a str,
    /// The small picture as a `data:` URI, when the file is one.
    preview: Option<&'a str>,
}

/// What the page is given: the agent of the pane on show, what the next message carries,
/// and the platform it runs on.
#[derive(serde::Serialize)]
struct View<'a> {
    pane: u64,
    agent: &'a Agent,
    attachments: Vec<File<'a>>,
    platform: &'static str,
}

/// The JSON the page's `update` function takes.
pub(crate) fn state(pane: u64, agent: &Agent, attachments: &[Attachment]) -> String {
    let view = View {
        pane,
        agent,
        attachments: attachments
            .iter()
            .map(|file| File {
                named: &file.named,
                name: &file.name,
                preview: file.preview.as_ref().map(|p| p.data_uri.as_str()),
            })
            .collect(),
        platform: if cfg!(target_os = "macos") {
            "macos"
        } else {
            "other"
        },
    };
    // A conversation always serializes; the empty view is the page's own idle state.
    serde_json::to_string(&view).unwrap_or_else(|_| "{\"pane\":null,\"agent\":null}".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spooky_core::{AgentDialog, AgentMessage, AgentRole, DialogKind};

    fn is_refused(text: &str) -> bool {
        event(text).is_err()
    }

    #[test]
    fn a_prompt_becomes_the_native_message_for_its_pane() {
        let event = event(r#"{"type":"send","pane":4,"text":"hi"}"#).expect("a prompt");
        assert!(matches!(
            event,
            Event::Agent { pane: 4, message: Message::Prompt(4, ref text) } if text == "hi"
        ));
    }

    #[test]
    fn answers_carry_exactly_what_the_user_clicked() {
        let yes = event(r#"{"type":"answer","pane":1,"dialog":"d","answer":{"Confirm":true}}"#)
            .expect("an answer");
        assert!(matches!(
            yes,
            Event::Agent { pane: 1, message: Message::Answer(1, ref d, DialogAnswer::Confirm(true)) } if d == "d"
        ));
        let dismissed = event(r#"{"type":"answer","pane":1,"dialog":"d","answer":"Cancel"}"#)
            .expect("an answer");
        assert!(matches!(
            dismissed,
            Event::Agent {
                message: Message::Answer(_, _, DialogAnswer::Cancel),
                ..
            }
        ));
    }

    #[test]
    fn unknown_or_malformed_actions_are_refused() {
        assert!(is_refused("not json"));
        assert!(is_refused(r#"{"type":"shell","command":"ls"}"#));
        assert!(is_refused(r#"{"type":"abort","pane":1,"extra":true}"#));
        assert!(is_refused(r#"{"type":"abort","pane":-1}"#));
        assert!(is_refused(r#"{"type":"send","pane":1}"#));
    }

    #[test]
    fn oversized_messages_are_refused_whole() {
        let big = "x".repeat(MAX_ACTION_BYTES + 1);
        assert!(is_refused(&format!(r#"{{"type":"copy","text":"{big}"}}"#)));
        let long = "x".repeat(MAX_AGENT_PROMPT_BYTES + 1);
        assert!(is_refused(&format!(
            r#"{{"type":"send","pane":1,"text":"{long}"}}"#
        )));
        let name = "m".repeat(MAX_NAME_BYTES + 1);
        assert!(is_refused(&format!(
            r#"{{"type":"setModel","pane":1,"model":"{name}"}}"#
        )));
        let url = "u".repeat(MAX_LINK_BYTES + 1);
        assert!(is_refused(&format!(
            r#"{{"type":"link","url":"{url}","open":true}}"#
        )));
    }

    #[test]
    fn a_paste_request_reads_no_text_from_the_page() {
        // The text comes from the clipboard, never from the page.
        let event = event(r#"{"type":"paste","text":"x"}"#).expect("a paste");
        assert!(matches!(event, Event::Paste));
    }

    #[test]
    fn a_link_only_reports_whether_the_open_modifier_was_held() {
        let event =
            event(r#"{"type":"link","url":"https://a.test","open":false}"#).expect("a link");
        assert!(matches!(event, Event::Link { ref url, open: false } if url == "https://a.test"));
    }

    #[test]
    fn forwarded_keys_are_shortcuts_or_nothing() {
        let key = |key: &str, code: &str, shift: bool, meta: bool| {
            forwarded(key, code, shift, false, false, meta)
        };
        if cfg!(target_os = "macos") {
            assert!(matches!(
                key("t", "KeyT", false, true),
                Ok(Event::Command(Command::NewTab))
            ));
            assert!(matches!(
                key("P", "KeyP", true, true),
                Ok(Event::Command(Command::Palette))
            ));
            assert!(matches!(
                key("3", "Digit3", false, true),
                Ok(Event::Command(Command::Tab(2)))
            ));
        }
        // Plain typing never becomes a command.
        assert!(matches!(key("a", "KeyA", false, false), Ok(Event::Ignored)));
        assert!(matches!(
            key("Enter", "Enter", false, true),
            Ok(Event::Ignored)
        ));
        assert!(key(&"k".repeat(MAX_KEY_BYTES + 1), "KeyK", false, true).is_err());
    }

    #[test]
    fn dropping_an_attachment_names_only_the_file_the_page_clicked() {
        let event = event(r#"{"type":"unattach","pane":2,"named":"/tmp/a.png"}"#).expect("a drop");
        assert!(matches!(
            event,
            Event::Agent { pane: 2, message: Message::Unattach(2, ref named) } if named == "/tmp/a.png"
        ));
        assert!(is_refused(r#"{"type":"unattach","pane":2}"#));
    }

    #[test]
    fn the_page_is_given_what_the_next_message_carries() {
        let agent = Agent::default();
        let files = [
            Attachment {
                named: "'/tmp/a b.png'".into(),
                name: "a b.png".into(),
                preview: Some(crate::features::attachments::preview::Preview {
                    png: vec![1, 2],
                    data_uri: "data:image/png;base64,AQI=".into(),
                    width: 4,
                    height: 2,
                }),
            },
            Attachment {
                named: "/tmp/notes.md".into(),
                name: "notes.md".into(),
                preview: None,
            },
        ];
        let value: serde_json::Value =
            serde_json::from_str(&state(1, &agent, &files)).expect("the state is JSON");
        assert_eq!(value["attachments"][0]["named"], "'/tmp/a b.png'");
        assert_eq!(
            value["attachments"][0]["preview"],
            "data:image/png;base64,AQI="
        );
        assert_eq!(value["attachments"][1]["preview"], serde_json::Value::Null);
    }

    #[test]
    fn the_state_the_page_reads_keeps_the_wire_shape_of_the_agent() {
        let agent = Agent {
            backend: "omp".into(),
            messages: vec![AgentMessage {
                id: 1,
                role: AgentRole::User,
                text: "hi".into(),
                tool: None,
            }],
            dialogs: vec![AgentDialog {
                id: "d".into(),
                kind: DialogKind::Confirm,
                title: "Run?".into(),
                message: String::new(),
                options: Vec::new(),
            }],
            ..Agent::default()
        };
        let value: serde_json::Value =
            serde_json::from_str(&state(7, &agent, &[])).expect("the state is JSON");
        assert_eq!(value["pane"], 7);
        assert_eq!(value["agent"]["messages"][0]["role"], "User");
        assert_eq!(value["agent"]["dialogs"][0]["kind"], "Confirm");
        assert!(value["agent"]["messages"][0].get("tool").is_none());
    }
}
