//! Application shortcuts: the chords that map to a [`Command`].
//!
//! macOS uses Cmd; other platforms use Ctrl+Shift so plain Ctrl combinations stay
//! terminal input. The letter chords that need a second variant (split rows,
//! close workspace, rename workspace) add Shift on macOS and Alt elsewhere;
//! workspace selection adds Alt on both. Digits and brackets are matched by
//! physical key so they work on any keyboard layout.

use crate::commands::Command;
use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};
use spooky_core::Axis;

/// Whether the platform's primary chord modifiers are held: Cmd on macOS,
/// Ctrl+Shift elsewhere.
pub(crate) fn primary(mods: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        mods.logo() && !mods.control()
    } else {
        mods.control() && mods.shift() && !mods.logo()
    }
}

/// The shortcut for a key press, if it is one. Digits and brackets are read from
/// the physical key.
pub(crate) fn shortcut(key: &Key, physical: Physical, mods: Modifiers) -> Option<Command> {
    let macos = cfg!(target_os = "macos");
    // Ctrl+Tab cycles tabs everywhere.
    if matches!(key, Key::Named(Named::Tab)) && mods.control() && !mods.logo() && !mods.alt() {
        return Some(if mods.shift() {
            Command::PreviousTab
        } else {
            Command::NextTab
        });
    }
    if !primary(mods) {
        return None;
    }
    let alt = mods.alt();
    // Cmd+Shift+digit belongs to macOS screenshots; Cmd+Option+letter is left to the system.
    let tab_digit = !(macos && mods.shift()) && !alt;
    let workspace_digit = alt && !(macos && mods.shift());

    if let Some(position) = digit(physical) {
        return if workspace_digit {
            Some(Command::Workspace(position))
        } else if tab_digit {
            Some(Command::Tab(position))
        } else {
            None
        };
    }
    match physical {
        Physical::Code(Code::BracketRight) if macos && mods.shift() && !alt => {
            return Some(Command::NextTab);
        }
        Physical::Code(Code::BracketLeft) if macos && mods.shift() && !alt => {
            return Some(Command::PreviousTab);
        }
        Physical::Code(Code::Comma) if !alt && !(macos && mods.shift()) => {
            return Some(Command::Settings);
        }
        _ => {}
    }
    match key {
        Key::Named(Named::PageDown) if !macos && !alt => return Some(Command::NextTab),
        Key::Named(Named::PageUp) if !macos && !alt => return Some(Command::PreviousTab),
        Key::Named(Named::ArrowDown) if alt => return Some(Command::NextWorkspace),
        Key::Named(Named::ArrowUp) if alt => return Some(Command::PreviousWorkspace),
        _ => {}
    }
    let Key::Character(text) = key else {
        return None;
    };
    if macos && alt {
        return None;
    }
    let second = if macos { mods.shift() } else { alt };
    match (text.to_ascii_lowercase().as_str(), second) {
        ("t", false) => Some(Command::NewTab),
        ("n", false) => Some(Command::NewWorkspace),
        ("d", false) => Some(Command::Split(Axis::Vertical)),
        ("d", true) => Some(Command::Split(Axis::Horizontal)),
        ("w", false) => Some(Command::ClosePane),
        ("w", true) => Some(Command::CloseWorkspace),
        ("r", false) => Some(Command::RenameTab),
        ("r", true) => Some(Command::RenameWorkspace),
        ("b", false) => Some(Command::ToggleSidebar),
        ("f", false) => Some(Command::Find),
        ("g", false) => Some(Command::FindNext),
        ("g", true) => Some(Command::FindPrevious),
        ("q", false) => Some(Command::Quit),
        // Cmd+Shift+P on macOS, Ctrl+Shift+P elsewhere.
        ("p", shift_or_alt) if shift_or_alt == macos => Some(Command::Palette),
        _ => None,
    }
}

/// The zero-based position a digit key selects.
fn digit(physical: Physical) -> Option<usize> {
    let Physical::Code(code) = physical else {
        return None;
    };
    Some(match code {
        Code::Digit1 => 0,
        Code::Digit2 => 1,
        Code::Digit3 => 2,
        Code::Digit4 => 3,
        Code::Digit5 => 4,
        Code::Digit6 => 5,
        Code::Digit7 => 6,
        Code::Digit8 => 7,
        Code::Digit9 => 8,
        _ => return None,
    })
}

/// The global-shortcut spelling of a key that may be bound system-wide.
fn hotkey_key_name(code: Code) -> Option<String> {
    let fixed = match code {
        Code::Space => "Space",
        Code::Enter => "Enter",
        Code::Tab => "Tab",
        Code::ArrowUp => "Up",
        Code::ArrowDown => "Down",
        Code::ArrowLeft => "Left",
        Code::ArrowRight => "Right",
        _ => "",
    };
    if !fixed.is_empty() {
        return Some(fixed.to_owned());
    }
    // Letters, digits and function keys are named like their `Code` variants.
    let name = format!("{code:?}");
    let short = |prefix: &str| {
        name.strip_prefix(prefix)
            .filter(|rest| rest.len() == 1 && rest.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(str::to_owned)
    };
    short("Key").or_else(|| short("Digit")).or_else(|| {
        let number = name.strip_prefix('F')?;
        (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit())).then_some(name)
    })
}

/// The spelling of a key press recorded as a new global shortcut (`"F10"`,
/// `"Ctrl+Shift+Space"`). `None` while only modifiers are held, and the reason when
/// the press cannot be a system-wide shortcut: a plain letter would take that key from
/// every other application.
pub(crate) fn hotkey_text(
    key: &Key,
    physical: Physical,
    mods: Modifiers,
) -> Option<Result<String, &'static str>> {
    if matches!(
        key,
        Key::Named(
            Named::Control
                | Named::Shift
                | Named::Alt
                | Named::AltGraph
                | Named::Super
                | Named::Meta
                | Named::Hyper
                | Named::CapsLock
                | Named::Fn
                | Named::FnLock
        )
    ) {
        return None;
    }
    let Some(name) = (match physical {
        Physical::Code(code) => hotkey_key_name(code),
        Physical::Unidentified(_) => None,
    }) else {
        return Some(Err("That key cannot be a global shortcut"));
    };
    let function_key = name.len() > 1 && name.starts_with('F') && name[1..].parse::<u8>().is_ok();
    if !function_key && !(mods.control() || mods.alt() || mods.logo()) {
        return Some(Err(
            "Hold Ctrl, Alt or Cmd with the key, or use a function key",
        ));
    }
    let mut text = String::new();
    for (held, label) in [
        (mods.control(), "Ctrl"),
        (mods.alt(), "Alt"),
        (mods.shift(), "Shift"),
        (mods.logo(), "Cmd"),
    ] {
        if held {
            text.push_str(label);
            text.push('+');
        }
    }
    text.push_str(&name);
    Some(Ok(text))
}

/// The item at `position`, where position 8 (the 9 key) means the last item, as in browsers.
pub(crate) fn nth(ids: &[u64], position: usize) -> Option<u64> {
    if position == 8 {
        ids.last().copied()
    } else {
        ids.get(position).copied()
    }
}

/// The item after (or before) `current`, wrapping at the ends. Without a current
/// item the first (or last) is chosen.
pub(crate) fn cycle(ids: &[u64], current: Option<u64>, forward: bool) -> Option<u64> {
    let at = current.and_then(|id| ids.iter().position(|candidate| *candidate == id));
    let next = match (at, forward) {
        (Some(index), true) => (index + 1) % ids.len(),
        (Some(index), false) => (index + ids.len() - 1) % ids.len(),
        (None, true) => 0,
        (None, false) => ids.len().checked_sub(1)?,
    };
    ids.get(next).copied()
}

#[cfg(test)]
mod tests {
    use super::{Command, cycle, hotkey_text, nth, shortcut};
    use iced::keyboard::{
        Key, Modifiers,
        key::{Code, Named, Physical},
    };
    use spooky_core::Axis;

    /// The platform's primary chord: Cmd, or Ctrl+Shift.
    const PRIMARY: Modifiers = if cfg!(target_os = "macos") {
        Modifiers::LOGO
    } else {
        Modifiers::CTRL.union(Modifiers::SHIFT)
    };
    /// What the second letter variant adds: Shift on macOS, Alt elsewhere.
    const SECOND: Modifiers = if cfg!(target_os = "macos") {
        Modifiers::SHIFT
    } else {
        Modifiers::ALT
    };

    fn letter(text: &str, mods: Modifiers) -> Option<Command> {
        shortcut(
            &Key::Character(text.into()),
            Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified),
            mods,
        )
    }

    #[test]
    fn letter_chords_drive_tabs_workspaces_and_panes() {
        let with_second = PRIMARY.union(SECOND);
        let cases = [
            ("t", PRIMARY, Command::NewTab),
            ("n", PRIMARY, Command::NewWorkspace),
            ("d", PRIMARY, Command::Split(Axis::Vertical)),
            ("d", with_second, Command::Split(Axis::Horizontal)),
            ("w", PRIMARY, Command::ClosePane),
            ("w", with_second, Command::CloseWorkspace),
            ("r", PRIMARY, Command::RenameTab),
            ("r", with_second, Command::RenameWorkspace),
            ("b", PRIMARY, Command::ToggleSidebar),
            ("q", PRIMARY, Command::Quit),
            ("f", PRIMARY, Command::Find),
            ("g", PRIMARY, Command::FindNext),
            ("g", with_second, Command::FindPrevious),
        ];
        for (text, mods, expected) in cases {
            assert_eq!(letter(text, mods), Some(expected), "{text} {mods:?}");
        }
        // The shifted letter reaches the app as an uppercase character.
        assert_eq!(letter("T", PRIMARY), Some(Command::NewTab));
    }

    #[test]
    fn the_palette_chord_is_command_shift_p_or_ctrl_shift_p() {
        let chord = if cfg!(target_os = "macos") {
            Modifiers::LOGO | Modifiers::SHIFT
        } else {
            Modifiers::CTRL | Modifiers::SHIFT
        };
        assert_eq!(letter("p", chord), Some(Command::Palette));
        // Without its second modifier, and as plain terminal input, it is not ours.
        assert_eq!(letter("p", Modifiers::CTRL), None);
        if cfg!(target_os = "macos") {
            assert_eq!(letter("p", Modifiers::LOGO), None);
        } else {
            assert_eq!(letter("p", chord | Modifiers::ALT), None);
        }
    }

    #[test]
    fn terminal_control_keys_and_unbound_letters_are_left_alone() {
        for text in ["t", "w", "d", "c", "r"] {
            assert_eq!(
                letter(text, Modifiers::CTRL),
                None,
                "Ctrl+{text} is terminal input"
            );
            assert_eq!(letter(text, Modifiers::empty()), None);
            assert_eq!(letter(text, Modifiers::ALT), None);
        }
        assert_eq!(letter("x", PRIMARY), None);
        assert_eq!(
            letter("t", PRIMARY.union(SECOND)),
            None,
            "no second variant for new tab"
        );
    }

    #[test]
    fn digits_select_by_physical_key_whatever_the_layout_types() {
        let one = Physical::Code(Code::Digit1);
        let nine = Physical::Code(Code::Digit9);
        // A layout where the 1 key types "&".
        let key = Key::Character("&".into());
        assert_eq!(shortcut(&key, one, PRIMARY), Some(Command::Tab(0)));
        assert_eq!(shortcut(&key, nine, PRIMARY), Some(Command::Tab(8)));
        assert_eq!(
            shortcut(&key, one, PRIMARY.union(Modifiers::ALT)),
            Some(Command::Workspace(0))
        );
        assert_eq!(shortcut(&key, one, Modifiers::empty()), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn command_shift_digits_stay_with_the_system_screenshot_shortcuts() {
        let key = Key::Character("#".into());
        let mods = Modifiers::LOGO | Modifiers::SHIFT;
        assert_eq!(shortcut(&key, Physical::Code(Code::Digit3), mods), None);
    }

    #[test]
    fn tabs_and_workspaces_can_be_stepped_from_the_keyboard() {
        let next_tab = if cfg!(target_os = "macos") {
            shortcut(
                &Key::Character("}".into()),
                Physical::Code(Code::BracketRight),
                Modifiers::LOGO | Modifiers::SHIFT,
            )
        } else {
            shortcut(
                &Key::Named(Named::PageDown),
                Physical::Code(Code::PageDown),
                PRIMARY,
            )
        };
        assert_eq!(next_tab, Some(Command::NextTab));
        let down = shortcut(
            &Key::Named(Named::ArrowDown),
            Physical::Code(Code::ArrowDown),
            PRIMARY.union(Modifiers::ALT),
        );
        assert_eq!(down, Some(Command::NextWorkspace));
        for (mods, expected) in [
            (Modifiers::CTRL, Command::NextTab),
            (Modifiers::CTRL | Modifiers::SHIFT, Command::PreviousTab),
        ] {
            let key = Key::Named(Named::Tab);
            assert_eq!(
                shortcut(&key, Physical::Code(Code::Tab), mods),
                Some(expected)
            );
        }
    }

    #[test]
    fn stepping_wraps_and_the_ninth_position_is_the_last_item() {
        let ids = [10, 20, 30];
        assert_eq!(cycle(&ids, Some(30), true), Some(10));
        assert_eq!(cycle(&ids, Some(10), false), Some(30));
        assert_eq!(cycle(&ids, Some(20), true), Some(30));
        assert_eq!(cycle(&ids, None, true), Some(10));
        assert_eq!(cycle(&ids, None, false), Some(30));
        assert_eq!(
            cycle(&ids, Some(99), true),
            Some(10),
            "an unknown current item starts over"
        );
        assert_eq!(cycle(&[], Some(1), true), None);
        assert_eq!(nth(&ids, 1), Some(20));
        assert_eq!(nth(&ids, 5), None);
        assert_eq!(nth(&ids, 8), Some(30));
        assert_eq!(nth(&[], 8), None);
    }

    #[test]
    fn the_settings_chord_is_read_from_the_comma_key() {
        let comma = Physical::Code(Code::Comma);
        let key = Key::Character(",".into());
        assert_eq!(shortcut(&key, comma, PRIMARY), Some(Command::Settings));
        assert_eq!(shortcut(&key, comma, Modifiers::empty()), None);
        assert_eq!(shortcut(&key, comma, Modifiers::CTRL), None);
    }

    #[test]
    fn a_recorded_press_becomes_a_shortcut_the_system_accepts() {
        use global_hotkey::hotkey::HotKey;
        let press = |key: Key, code: Code, mods| hotkey_text(&key, Physical::Code(code), mods);
        let cases = [
            (Key::Named(Named::F10), Code::F10, Modifiers::empty(), "F10"),
            (
                Key::Named(Named::Space),
                Code::Space,
                Modifiers::CTRL | Modifiers::SHIFT,
                "Ctrl+Shift+Space",
            ),
            (
                Key::Character("k".into()),
                Code::KeyK,
                Modifiers::LOGO | Modifiers::SHIFT,
                "Shift+Cmd+K",
            ),
            (
                Key::Character("1".into()),
                Code::Digit1,
                Modifiers::ALT,
                "Alt+1",
            ),
            (
                Key::Named(Named::ArrowDown),
                Code::ArrowDown,
                Modifiers::CTRL | Modifiers::ALT,
                "Ctrl+Alt+Down",
            ),
        ];
        for (key, code, mods, expected) in cases {
            let text = press(key, code, mods)
                .expect("a key was pressed")
                .expect("usable");
            assert_eq!(text, expected);
            assert!(text.parse::<HotKey>().is_ok(), "{text} must parse");
        }
    }

    #[test]
    fn a_press_that_would_take_a_typing_key_from_every_app_is_refused() {
        let letter = |mods| {
            hotkey_text(
                &Key::Character("a".into()),
                Physical::Code(Code::KeyA),
                mods,
            )
        };
        assert!(
            matches!(letter(Modifiers::empty()), Some(Err(_))),
            "bare letter"
        );
        assert!(
            matches!(letter(Modifiers::SHIFT), Some(Err(_))),
            "shift and a letter"
        );
        // Holding only a modifier is not an answer yet.
        assert!(
            hotkey_text(
                &Key::Named(Named::Control),
                Physical::Code(Code::ControlLeft),
                Modifiers::CTRL
            )
            .is_none()
        );
        // A key with no stable identity cannot be registered.
        let unknown = Physical::Unidentified(iced::keyboard::key::NativeCode::Unidentified);
        assert!(matches!(
            hotkey_text(&Key::Character("a".into()), unknown, Modifiers::CTRL),
            Some(Err(_))
        ));
    }
}
