//! GPUI keystrokes as the application's key vocabulary.

use crate::keys::{Code, Key, Modifiers, Named, Physical, Press};
use gpui::Keystroke;

/// The application's key press for a GPUI keystroke.
///
/// GPUI reports the character a key makes with no modifier held, so the layout-independent
/// position is only known for keys whose character identifies them (letters, digits and the
/// bracket and comma keys on a US layout).
pub(crate) fn press(keystroke: &Keystroke) -> Press {
    let modifiers = modifiers(&keystroke.modifiers);
    let key = key(&keystroke.key);
    let physical = code(&keystroke.key).map_or(Physical::Unidentified, Physical::Code);
    // A held Ctrl or Cmd makes a chord, not text, whatever the layout would have typed.
    let text = if modifiers.control || modifiers.logo {
        None
    } else {
        keystroke.key_char.clone()
    };
    Press {
        key,
        physical,
        modifiers,
        text,
    }
}

/// The modifier keys held.
pub(crate) fn modifiers(modifiers: &gpui::Modifiers) -> Modifiers {
    Modifiers {
        shift: modifiers.shift,
        control: modifiers.control,
        alt: modifiers.alt,
        logo: modifiers.platform,
    }
}

fn key(name: &str) -> Key {
    let named = match name {
        "up" => Named::ArrowUp,
        "down" => Named::ArrowDown,
        "left" => Named::ArrowLeft,
        "right" => Named::ArrowRight,
        "home" => Named::Home,
        "end" => Named::End,
        "pageup" => Named::PageUp,
        "pagedown" => Named::PageDown,
        "insert" => Named::Insert,
        "delete" => Named::Delete,
        "backspace" => Named::Backspace,
        "enter" => Named::Enter,
        "tab" => Named::Tab,
        "space" => Named::Space,
        "escape" => Named::Escape,
        "f1" => Named::F1,
        "f2" => Named::F2,
        "f3" => Named::F3,
        "f4" => Named::F4,
        "f5" => Named::F5,
        "f6" => Named::F6,
        "f7" => Named::F7,
        "f8" => Named::F8,
        "f9" => Named::F9,
        "f10" => Named::F10,
        "f11" => Named::F11,
        "f12" => Named::F12,
        _ => {
            return if name.chars().count() == 1 {
                Key::Character(name.to_owned())
            } else {
                Key::Unidentified
            };
        }
    };
    Key::Named(named)
}

fn code(name: &str) -> Option<Code> {
    Some(match name {
        "a" => Code::KeyA,
        "b" => Code::KeyB,
        "c" => Code::KeyC,
        "d" => Code::KeyD,
        "e" => Code::KeyE,
        "f" => Code::KeyF,
        "g" => Code::KeyG,
        "h" => Code::KeyH,
        "i" => Code::KeyI,
        "j" => Code::KeyJ,
        "k" => Code::KeyK,
        "l" => Code::KeyL,
        "m" => Code::KeyM,
        "n" => Code::KeyN,
        "o" => Code::KeyO,
        "p" => Code::KeyP,
        "q" => Code::KeyQ,
        "r" => Code::KeyR,
        "s" => Code::KeyS,
        "t" => Code::KeyT,
        "u" => Code::KeyU,
        "v" => Code::KeyV,
        "w" => Code::KeyW,
        "x" => Code::KeyX,
        "y" => Code::KeyY,
        "z" => Code::KeyZ,
        "0" => Code::Digit0,
        "1" => Code::Digit1,
        "2" => Code::Digit2,
        "3" => Code::Digit3,
        "4" => Code::Digit4,
        "5" => Code::Digit5,
        "6" => Code::Digit6,
        "7" => Code::Digit7,
        "8" => Code::Digit8,
        "9" => Code::Digit9,
        "[" => Code::BracketLeft,
        "]" => Code::BracketRight,
        "," => Code::Comma,
        "space" => Code::Space,
        "enter" => Code::Enter,
        "tab" => Code::Tab,
        "escape" => Code::Escape,
        "up" => Code::ArrowUp,
        "down" => Code::ArrowDown,
        "left" => Code::ArrowLeft,
        "right" => Code::ArrowRight,
        "f1" => Code::F1,
        "f2" => Code::F2,
        "f3" => Code::F3,
        "f4" => Code::F4,
        "f5" => Code::F5,
        "f6" => Code::F6,
        "f7" => Code::F7,
        "f8" => Code::F8,
        "f9" => Code::F9,
        "f10" => Code::F10,
        "f11" => Code::F11,
        "f12" => Code::F12,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_carry_modifiers_and_position_but_no_text() {
        let press = press(&Keystroke::parse("cmd-shift-k").expect("a valid keystroke"));
        assert_eq!(press.key, Key::Character("k".into()));
        assert_eq!(press.physical, Physical::Code(Code::KeyK));
        assert!(press.modifiers.logo && press.modifiers.shift && !press.modifiers.control);
        assert_eq!(press.text, None);
    }

    #[test]
    fn named_keys_map_to_their_names() {
        let press = press(&Keystroke::parse("ctrl-left").expect("a valid keystroke"));
        assert_eq!(press.key, Key::Named(Named::ArrowLeft));
        assert!(press.modifiers.control);
    }
}
