//! Turning key presses, pastes and focus changes into the bytes a terminal program reads.
//!
//! Text is not handled here: what the keyboard types, composed by the input method (dead
//! keys, accents, CJK), arrives as text and goes to the program as it is. This module
//! encodes the presses that are not text, which are named keys and Ctrl and Alt chords.

use crate::keys::{Key, Modifiers, Named};

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
pub(crate) fn clipboard_action(key: &Key, mods: Modifiers) -> Option<ClipboardAction> {
    if !((mods.logo() && !mods.control()) || (mods.control() && mods.shift())) {
        return None;
    }
    match key {
        Key::Character(c) if c.eq_ignore_ascii_case("c") => Some(ClipboardAction::Copy),
        Key::Character(c) if c.eq_ignore_ascii_case("v") => Some(ClipboardAction::Paste),
        _ => None,
    }
}

/// What a press that is not text sends to the program: `None` when it is text, which the
/// input method delivers, or a chord the terminal has no encoding for.
///
/// `typed` is the text the press types; with Alt held on other platforms than macOS the
/// program reads it after ESC. `app` is the application cursor mode.
pub(crate) fn key_bytes(
    key: &Key,
    mods: Modifiers,
    typed: Option<&str>,
    app: bool,
) -> Option<Vec<u8>> {
    if mods.logo() {
        return None;
    }
    let modifier =
        1 + u8::from(mods.shift()) + 2 * u8::from(mods.alt()) + 4 * u8::from(mods.control());
    let mut bytes = match key {
        Key::Character(s) if mods.control() => {
            let c = s.chars().next()?.to_ascii_uppercase();
            match c {
                '@'..='_' => vec![(c as u8) & 0x1f],
                '?' => vec![0x7f],
                ' ' => vec![0],
                _ => return None,
            }
        }
        // Alt makes the program read ESC and then the character. On macOS Option composes
        // text instead, and the input method delivers that.
        Key::Character(s) if mods.alt() && !cfg!(target_os = "macos") => {
            typed.unwrap_or(s.as_str()).as_bytes().to_vec()
        }
        Key::Character(_) | Key::Unidentified => return None,
        Key::Named(named) => {
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
                        // A space with Alt held is ESC and a space, on macOS too: Option+Space
                        // composes nothing, so it is not left to the input method.
                        Named::Space if mods.alt() && typed.is_none_or(|text| text == " ") => {
                            b" ".to_vec()
                        }
                        Named::Space => return None,
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
    };
    if mods.alt()
        && (!cfg!(target_os = "macos") || mods.control())
        && matches!(key, Key::Character(_) | Key::Named(Named::Space))
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_left_to_the_input_method() {
        for key in [Key::Character("a".into()), Key::Named(Named::Space)] {
            for mods in [Modifiers::empty(), Modifiers::SHIFT] {
                assert_eq!(key_bytes(&key, mods, Some("a"), false), None, "{key:?}");
            }
        }
    }

    #[test]
    fn space_with_control_or_alt_is_not_text() {
        let space = Key::Named(Named::Space);
        assert_eq!(
            key_bytes(&space, Modifiers::CTRL, None, false),
            Some(b"\0".to_vec())
        );
        assert_eq!(
            key_bytes(&space, Modifiers::ALT | Modifiers::CTRL, None, false),
            Some(b"\x1b\0".to_vec())
        );
        let alt = key_bytes(&space, Modifiers::ALT, None, false);
        if cfg!(target_os = "macos") {
            assert_eq!(alt, Some(b" ".to_vec()));
        } else {
            assert_eq!(alt, Some(b"\x1b ".to_vec()));
        }
        assert_eq!(key_bytes(&space, Modifiers::LOGO, None, false), None);
    }

    #[test]
    fn control_letters_are_control_codes() {
        let c = Key::Character("c".into());
        assert_eq!(key_bytes(&c, Modifiers::CTRL, None, false), Some(vec![3]));
        assert_eq!(
            key_bytes(&Key::Character("?".into()), Modifiers::CTRL, None, false),
            Some(vec![0x7f])
        );
        assert_eq!(
            key_bytes(&c, Modifiers::CTRL | Modifiers::LOGO, None, false),
            None
        );
    }

    #[test]
    fn alt_makes_the_program_read_escape_first_except_where_option_composes() {
        let e = Key::Character("e".into());
        let bytes = key_bytes(&e, Modifiers::ALT, Some("e"), false);
        if cfg!(target_os = "macos") {
            assert_eq!(
                bytes, None,
                "Option composes text; the input method sends it"
            );
        } else {
            assert_eq!(bytes, Some(b"\x1be".to_vec()));
        }
    }

    #[test]
    fn named_keys_follow_the_application_cursor_mode_and_modifiers() {
        let up = Key::Named(Named::ArrowUp);
        assert_eq!(
            key_bytes(&up, Modifiers::empty(), None, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_bytes(&up, Modifiers::empty(), None, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(&up, Modifiers::SHIFT, None, true),
            Some(b"\x1b[1;2A".to_vec())
        );
        assert_eq!(
            key_bytes(&Key::Named(Named::Delete), Modifiers::CTRL, None, false),
            Some(b"\x1b[3;5~".to_vec())
        );
        assert_eq!(
            key_bytes(&Key::Named(Named::Tab), Modifiers::SHIFT, None, false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            key_bytes(&Key::Named(Named::F1), Modifiers::empty(), None, false),
            Some(b"\x1bOP".to_vec())
        );
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
