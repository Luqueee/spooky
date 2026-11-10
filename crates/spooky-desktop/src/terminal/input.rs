//! Turning key presses, pastes and focus changes into the bytes a terminal program reads.

use iced::keyboard;

/// Whether a text field's new content is its old content plus the one letter of
/// a search chord (Cmd+F or Cmd+G). The field inserts that letter even while the
/// chord modifier is held, so such an edit is the chord and not typing.
pub(crate) fn is_chord_echo(old: &str, new: &str) -> bool {
    new.strip_prefix(old).is_some_and(|extra| {
        let mut chars = extra.chars();
        matches!(
            (chars.next(), chars.next()),
            (Some(c), None) if matches!(c.to_ascii_lowercase(), 'f' | 'g')
        )
    })
}

const FOCUS_IN: &[u8] = b"\x1b[I";
const FOCUS_OUT: &[u8] = b"\x1b[O";

/// What to send when the keyboard focus moves from pane `old` to pane `new`:
/// focus-out and focus-in reports, only to programs that asked for them.
pub(crate) fn focus_reports(
    old: Option<u64>,
    new: Option<u64>,
    reporting: impl Fn(u64) -> bool,
) -> Vec<(u64, &'static [u8])> {
    let mut reports = Vec::new();
    if old == new {
        return reports;
    }
    if let Some(id) = old
        && reporting(id)
    {
        reports.push((id, FOCUS_OUT));
    }
    if let Some(id) = new
        && reporting(id)
    {
        reports.push((id, FOCUS_IN));
    }
    reports
}

/// Bytes sent for pasted text. Bracketed paste drops ESC and ETX: pasted text
/// must not end the bracket early (running the rest as typed input) or interrupt
/// the foreground program.
pub(crate) fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let text = text.replace(['\x1b', '\x03'], "");
    format!("\x1b[200~{text}\x1b[201~").into_bytes()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ClipboardAction {
    Copy,
    Paste,
}

/// Cmd+C/V and Ctrl+Shift+C/V. Plain Ctrl+C and Ctrl+V remain terminal input.
pub(crate) fn clipboard_action(
    key: &keyboard::Key,
    mods: keyboard::Modifiers,
) -> Option<ClipboardAction> {
    if !((mods.logo() && !mods.control()) || (mods.control() && mods.shift())) {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character(c) if c.eq_ignore_ascii_case("c") => Some(ClipboardAction::Copy),
        keyboard::Key::Character(c) if c.eq_ignore_ascii_case("v") => Some(ClipboardAction::Paste),
        _ => None,
    }
}

pub(crate) fn terminal_text(
    key: &keyboard::Key,
    physical: keyboard::key::Physical,
    modifiers: keyboard::Modifiers,
    text: Option<String>,
) -> Option<String> {
    #[cfg(target_os = "macos")]
    if modifiers.alt()
        && !modifiers.control()
        && !modifiers.logo()
        && matches!(key, keyboard::Key::Character(_))
        && text.as_deref().is_none_or(str::is_empty)
    {
        return crate::platform::macos::keyboard::option_text(physical, modifiers);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (key, physical, modifiers);
    text
}

pub(crate) fn key_bytes(
    key: &keyboard::Key,
    mods: keyboard::Modifiers,
    typed: Option<&str>,
    app: bool,
) -> Option<Vec<u8>> {
    use keyboard::key::Named;
    if mods.logo() {
        return None;
    }
    let modifier =
        1 + u8::from(mods.shift()) + 2 * u8::from(mods.alt()) + 4 * u8::from(mods.control());
    let mut bytes = match key {
        keyboard::Key::Character(s) if mods.control() => {
            let c = s.chars().next()?.to_ascii_uppercase();
            match c {
                '@'..='_' => vec![(c as u8) & 0x1f],
                '?' => vec![0x7f],
                ' ' => vec![0],
                _ => return None,
            }
        }
        keyboard::Key::Character(s) => {
            // Dead keys without terminal text must not leak their unmodified base key.
            let text = if cfg!(target_os = "macos") {
                typed.filter(|text| !text.is_empty())?
            } else {
                typed.unwrap_or(s.as_str())
            };
            text.as_bytes().to_vec()
        }
        keyboard::Key::Named(named) => {
            let arrow = match named {
                Named::ArrowUp => Some('A'),
                Named::ArrowDown => Some('B'),
                Named::ArrowRight => Some('C'),
                Named::ArrowLeft => Some('D'),
                Named::Home => Some('H'),
                Named::End => Some('F'),
                _ => None,
            };
            if let Some(c) = arrow {
                if modifier > 1 {
                    format!("\x1b[1;{modifier}{c}").into_bytes()
                } else if app {
                    format!("\x1bO{c}").into_bytes()
                } else {
                    format!("\x1b[{c}").into_bytes()
                }
            } else {
                let code = match named {
                    Named::Insert => Some(2),
                    Named::Delete => Some(3),
                    Named::PageUp => Some(5),
                    Named::PageDown => Some(6),
                    Named::F5 => Some(15),
                    Named::F6 => Some(17),
                    Named::F7 => Some(18),
                    Named::F8 => Some(19),
                    Named::F9 => Some(20),
                    Named::F10 => Some(21),
                    Named::F11 => Some(23),
                    Named::F12 => Some(24),
                    _ => None,
                };
                if let Some(n) = code {
                    if modifier > 1 {
                        format!("\x1b[{n};{modifier}~").into_bytes()
                    } else {
                        format!("\x1b[{n}~").into_bytes()
                    }
                } else {
                    match named {
                        Named::Space if mods.control() => vec![0],
                        Named::Space => typed.unwrap_or(" ").as_bytes().to_vec(),
                        Named::Enter => b"\r".to_vec(),
                        Named::Backspace => vec![0x7f],
                        Named::Tab if mods.shift() => b"\x1b[Z".to_vec(),
                        Named::Tab => b"\t".to_vec(),
                        Named::Escape => vec![0x1b],
                        Named::F1 => b"\x1bOP".to_vec(),
                        Named::F2 => b"\x1bOQ".to_vec(),
                        Named::F3 => b"\x1bOR".to_vec(),
                        Named::F4 => b"\x1bOS".to_vec(),
                        _ => return None,
                    }
                }
            }
        }
        _ => return None,
    };
    if mods.alt()
        && (!cfg!(target_os = "macos") || mods.control())
        && matches!(
            key,
            keyboard::Key::Character(_) | keyboard::Key::Named(Named::Space)
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{Key, Modifiers, key::Named};

    #[test]
    fn space_reaches_the_shell_with_terminal_modifiers() {
        let cases: &[(Modifiers, Option<&[u8]>)] = &[
            (Modifiers::empty(), Some(b" ")),
            (Modifiers::SHIFT, Some(b" ")),
            (Modifiers::CTRL, Some(b"\0")),
            #[cfg(target_os = "macos")]
            (Modifiers::ALT, Some(b" ")),
            #[cfg(not(target_os = "macos"))]
            (Modifiers::ALT, Some(b"\x1b ")),
            (Modifiers::ALT | Modifiers::CTRL, Some(b"\x1b\0")),
            (Modifiers::LOGO, None),
        ];
        for &(modifiers, expected) in cases {
            for text in [None, Some(" ")] {
                assert_eq!(
                    key_bytes(&Key::Named(Named::Space), modifiers, text, false).as_deref(),
                    expected,
                    "space with {modifiers:?}, text {text:?}"
                );
            }
        }
    }

    #[test]
    fn composed_text_reaches_the_shell_unchanged() {
        assert_eq!(
            key_bytes(
                &Key::Named(Named::Space),
                Modifiers::empty(),
                Some("~"),
                false
            ),
            Some(b"~".to_vec())
        );
        assert_eq!(
            key_bytes(
                &Key::Character("n".into()),
                Modifiers::empty(),
                Some("ñ"),
                false
            ),
            Some("ñ".as_bytes().to_vec())
        );
        #[cfg(target_os = "macos")]
        {
            for base in ["ñ", "n", "e"] {
                assert_eq!(
                    key_bytes(&Key::Character(base.into()), Modifiers::ALT, None, false),
                    None,
                    "an uncommitted dead key must not reach the shell"
                );
            }
            assert_eq!(
                key_bytes(&Key::Named(Named::Space), Modifiers::ALT, Some("~"), false),
                Some(b"~".to_vec())
            );
            assert_eq!(
                key_bytes(
                    &Key::Character("2".into()),
                    Modifiers::ALT,
                    Some("@"),
                    false
                ),
                Some(b"@".to_vec())
            );
        }
    }

    #[test]
    fn focus_is_reported_only_to_programs_that_asked_for_it() {
        let asks = |id| id != 3;
        let (out, focus_in) = (&b"\x1b[O"[..], &b"\x1b[I"[..]);
        assert_eq!(
            focus_reports(Some(1), Some(2), asks),
            [(1, out), (2, focus_in)]
        );
        assert_eq!(focus_reports(None, Some(1), asks), [(1, focus_in)]);
        assert_eq!(focus_reports(Some(1), None, asks), [(1, out)]);
        assert_eq!(focus_reports(Some(1), Some(3), asks), [(1, out)]);
        assert!(focus_reports(Some(1), Some(1), asks).is_empty());
    }

    #[test]
    fn the_letter_a_search_chord_leaves_in_a_text_field_is_not_typing() {
        assert!(is_chord_echo("hit-", "hit-g"));
        assert!(is_chord_echo("hit-", "hit-F"));
        assert!(is_chord_echo("", "g"));
        assert!(!is_chord_echo("hit-", "hit-x"), "other letters are typing");
        assert!(!is_chord_echo("hit-", "hit-gg"), "pasting is not a chord");
        assert!(!is_chord_echo("hit-", "hit"), "deleting is not a chord");
        assert!(!is_chord_echo("a", "ba"), "only a trailing letter counts");
    }

    #[test]
    fn bracketed_paste_cannot_be_ended_or_interrupted_by_pasted_text() {
        assert_eq!(
            paste_bytes("a\x1b[201~rm -rf ~\n\x03", true),
            b"\x1b[200~a[201~rm -rf ~\n\x1b[201~"
        );
        assert_eq!(paste_bytes("a\x1b[b\n", false), b"a\x1b[b\n");
    }

    #[test]
    fn clipboard_chords_leave_plain_control_keys_for_the_terminal() {
        let c = Key::Character("c".into());
        let v = Key::Character("v".into());
        let cases = [
            (&c, Modifiers::LOGO, Some(ClipboardAction::Copy)),
            (
                &c,
                Modifiers::LOGO | Modifiers::SHIFT,
                Some(ClipboardAction::Copy),
            ),
            (
                &c,
                Modifiers::CTRL | Modifiers::SHIFT,
                Some(ClipboardAction::Copy),
            ),
            (&v, Modifiers::LOGO, Some(ClipboardAction::Paste)),
            (
                &v,
                Modifiers::CTRL | Modifiers::SHIFT,
                Some(ClipboardAction::Paste),
            ),
            (&c, Modifiers::CTRL, None),
            (&v, Modifiers::CTRL, None),
            (&c, Modifiers::empty(), None),
        ];
        for (key, modifiers, expected) in cases {
            assert_eq!(
                clipboard_action(key, modifiers),
                expected,
                "{key:?} with {modifiers:?}"
            );
        }
    }
}
