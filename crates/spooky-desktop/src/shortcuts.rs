//! Application shortcuts: the chords that drive workspaces, tabs and panes.
//!
//! macOS uses Cmd; other platforms use Ctrl+Shift so plain Ctrl combinations stay
//! terminal input. The letter chords that need a second variant (split rows,
//! close workspace, rename workspace) add Shift on macOS and Alt elsewhere;
//! workspace selection adds Alt on both. Digits and brackets are matched by
//! physical key so they work on any keyboard layout.

use iced::keyboard::{
    Key, Modifiers,
    key::{Code, Named, Physical},
};
use spooky_core::Axis;

/// What a chord asks the application to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shortcut {
    NewTab,
    NewWorkspace,
    Split(Axis),
    ClosePane,
    CloseWorkspace,
    /// A zero-based tab position; position 8 means the last tab.
    Tab(usize),
    NextTab,
    PreviousTab,
    /// A zero-based workspace position; position 8 means the last workspace.
    Workspace(usize),
    NextWorkspace,
    PreviousWorkspace,
    RenameTab,
    RenameWorkspace,
    ToggleSidebar,
    /// Open the scrollback search, or focus it when it is open.
    Find,
    FindNext,
    FindPrevious,
    Quit,
}

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
pub(crate) fn shortcut(key: &Key, physical: Physical, mods: Modifiers) -> Option<Shortcut> {
    let macos = cfg!(target_os = "macos");
    // Ctrl+Tab cycles tabs everywhere.
    if matches!(key, Key::Named(Named::Tab)) && mods.control() && !mods.logo() && !mods.alt() {
        return Some(if mods.shift() {
            Shortcut::PreviousTab
        } else {
            Shortcut::NextTab
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
            Some(Shortcut::Workspace(position))
        } else if tab_digit {
            Some(Shortcut::Tab(position))
        } else {
            None
        };
    }
    match physical {
        Physical::Code(Code::BracketRight) if macos && mods.shift() && !alt => {
            return Some(Shortcut::NextTab);
        }
        Physical::Code(Code::BracketLeft) if macos && mods.shift() && !alt => {
            return Some(Shortcut::PreviousTab);
        }
        _ => {}
    }
    match key {
        Key::Named(Named::PageDown) if !macos && !alt => return Some(Shortcut::NextTab),
        Key::Named(Named::PageUp) if !macos && !alt => return Some(Shortcut::PreviousTab),
        Key::Named(Named::ArrowDown) if alt => return Some(Shortcut::NextWorkspace),
        Key::Named(Named::ArrowUp) if alt => return Some(Shortcut::PreviousWorkspace),
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
        ("t", false) => Some(Shortcut::NewTab),
        ("n", false) => Some(Shortcut::NewWorkspace),
        ("d", false) => Some(Shortcut::Split(Axis::Vertical)),
        ("d", true) => Some(Shortcut::Split(Axis::Horizontal)),
        ("w", false) => Some(Shortcut::ClosePane),
        ("w", true) => Some(Shortcut::CloseWorkspace),
        ("r", false) => Some(Shortcut::RenameTab),
        ("r", true) => Some(Shortcut::RenameWorkspace),
        ("b", false) => Some(Shortcut::ToggleSidebar),
        ("f", false) => Some(Shortcut::Find),
        ("g", false) => Some(Shortcut::FindNext),
        ("g", true) => Some(Shortcut::FindPrevious),
        ("q", false) => Some(Shortcut::Quit),
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
    use super::{Shortcut, cycle, nth, shortcut};
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

    fn letter(text: &str, mods: Modifiers) -> Option<Shortcut> {
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
            ("t", PRIMARY, Shortcut::NewTab),
            ("n", PRIMARY, Shortcut::NewWorkspace),
            ("d", PRIMARY, Shortcut::Split(Axis::Vertical)),
            ("d", with_second, Shortcut::Split(Axis::Horizontal)),
            ("w", PRIMARY, Shortcut::ClosePane),
            ("w", with_second, Shortcut::CloseWorkspace),
            ("r", PRIMARY, Shortcut::RenameTab),
            ("r", with_second, Shortcut::RenameWorkspace),
            ("b", PRIMARY, Shortcut::ToggleSidebar),
            ("q", PRIMARY, Shortcut::Quit),
            ("f", PRIMARY, Shortcut::Find),
            ("g", PRIMARY, Shortcut::FindNext),
            ("g", with_second, Shortcut::FindPrevious),
        ];
        for (text, mods, expected) in cases {
            assert_eq!(letter(text, mods), Some(expected), "{text} {mods:?}");
        }
        // The shifted letter reaches the app as an uppercase character.
        assert_eq!(letter("T", PRIMARY), Some(Shortcut::NewTab));
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
        assert_eq!(shortcut(&key, one, PRIMARY), Some(Shortcut::Tab(0)));
        assert_eq!(shortcut(&key, nine, PRIMARY), Some(Shortcut::Tab(8)));
        assert_eq!(
            shortcut(&key, one, PRIMARY.union(Modifiers::ALT)),
            Some(Shortcut::Workspace(0))
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
        assert_eq!(next_tab, Some(Shortcut::NextTab));
        let down = shortcut(
            &Key::Named(Named::ArrowDown),
            Physical::Code(Code::ArrowDown),
            PRIMARY.union(Modifiers::ALT),
        );
        assert_eq!(down, Some(Shortcut::NextWorkspace));
        for (mods, expected) in [
            (Modifiers::CTRL, Shortcut::NextTab),
            (Modifiers::CTRL | Modifiers::SHIFT, Shortcut::PreviousTab),
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
}
