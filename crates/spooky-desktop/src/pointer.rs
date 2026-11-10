//! Mouse reports and wheel-as-arrow input for terminal applications. While an
//! application asks for mouse events, the pointer is theirs: the terminal sends
//! these bytes instead of selecting text or scrolling its own history.

use iced::keyboard::Modifiers;
use spooky_core::{MouseEncoding, MouseTracking};

/// A button or wheel notch an application can be told about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Button {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl Button {
    /// The reportable button for an iced mouse button; others are never reported.
    pub(crate) fn from_mouse(button: iced::mouse::Button) -> Option<Self> {
        match button {
            iced::mouse::Button::Left => Some(Self::Left),
            iced::mouse::Button::Middle => Some(Self::Middle),
            iced::mouse::Button::Right => Some(Self::Right),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Press,
    Release,
    Motion,
}

/// Bytes telling the application about one event at zero-based `col`/`row`.
///
/// `button` is the pressed or released button, or the button held during motion
/// (`None` for motion with nothing held). Returns `None` when the application's
/// tracking mode does not ask for the event, or the encoding cannot express the
/// position.
pub(crate) fn report(
    tracking: MouseTracking,
    encoding: MouseEncoding,
    action: Action,
    button: Option<Button>,
    modifiers: Modifiers,
    col: u16,
    row: u16,
) -> Option<Vec<u8>> {
    let wheel = matches!(button, Some(Button::WheelUp | Button::WheelDown));
    let wanted = match (tracking, action) {
        (MouseTracking::Off, _) => false,
        (_, Action::Motion) => {
            tracking == MouseTracking::Motion
                || (tracking == MouseTracking::Drag && button.is_some())
        }
        (_, Action::Release) => !wheel,
        (_, Action::Press) => true,
    };
    if !wanted {
        return None;
    }
    let sgr = encoding == MouseEncoding::Sgr;
    let mut code: u32 = match (action, button) {
        // Legacy encodings do not say which button was released.
        (Action::Release, _) if !sgr => 3,
        (_, Some(Button::Left)) => 0,
        (_, Some(Button::Middle)) => 1,
        (_, Some(Button::Right)) => 2,
        (_, Some(Button::WheelUp)) => 64,
        (_, Some(Button::WheelDown)) => 65,
        (_, None) => 3,
    };
    if action == Action::Motion {
        code += 32;
    }
    if modifiers.shift() {
        code += 4;
    }
    if modifiers.alt() {
        code += 8;
    }
    if modifiers.control() {
        code += 16;
    }
    let (x, y) = (u32::from(col) + 1, u32::from(row) + 1);
    match encoding {
        MouseEncoding::Sgr => {
            let end = if action == Action::Release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{x};{y}{end}").into_bytes())
        }
        MouseEncoding::Normal => {
            let mut bytes = vec![0x1b, b'[', b'M'];
            for value in [code, x, y] {
                bytes.push(u8::try_from(value + 32).ok()?);
            }
            Some(bytes)
        }
        MouseEncoding::Utf8 => {
            let mut text = String::from("\x1b[M");
            for value in [code, x, y] {
                let value = value + 32;
                if value > 2047 {
                    return None;
                }
                text.push(char::from_u32(value)?);
            }
            Some(text.into_bytes())
        }
    }
}

/// `count` arrow-key presses for wheel movement over an application that left
/// the alternate screen's alternate-scroll mode on (less, man, vim).
pub(crate) fn wheel_arrows(up: bool, application_cursor: bool, count: u32) -> Vec<u8> {
    let arrow = [
        0x1b,
        if application_cursor { b'O' } else { b'[' },
        if up { b'A' } else { b'B' },
    ];
    arrow.repeat(count as usize)
}

#[cfg(test)]
mod tests {
    use super::{Action, Button, report, wheel_arrows};
    use iced::keyboard::Modifiers;
    use spooky_core::{MouseEncoding, MouseTracking};

    fn sgr(action: Action, button: Option<Button>, modifiers: Modifiers) -> Option<Vec<u8>> {
        report(
            MouseTracking::Motion,
            MouseEncoding::Sgr,
            action,
            button,
            modifiers,
            0,
            0,
        )
    }

    #[test]
    fn sgr_reports_say_which_button_was_released() {
        assert_eq!(
            sgr(Action::Press, Some(Button::Left), Modifiers::empty()),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        assert_eq!(
            sgr(Action::Release, Some(Button::Left), Modifiers::empty()),
            Some(b"\x1b[<0;1;1m".to_vec())
        );
        assert_eq!(
            sgr(Action::Press, Some(Button::Right), Modifiers::empty()),
            Some(b"\x1b[<2;1;1M".to_vec())
        );
    }

    #[test]
    fn modifiers_motion_and_wheel_add_to_the_button_code() {
        assert_eq!(
            sgr(
                Action::Press,
                Some(Button::Left),
                Modifiers::CTRL | Modifiers::ALT
            ),
            Some(b"\x1b[<24;1;1M".to_vec())
        );
        assert_eq!(
            sgr(Action::Motion, Some(Button::Left), Modifiers::empty()),
            Some(b"\x1b[<32;1;1M".to_vec())
        );
        assert_eq!(
            sgr(Action::Motion, None, Modifiers::empty()),
            Some(b"\x1b[<35;1;1M".to_vec())
        );
        assert_eq!(
            sgr(Action::Press, Some(Button::WheelUp), Modifiers::SHIFT),
            Some(b"\x1b[<68;1;1M".to_vec())
        );
        assert_eq!(
            sgr(Action::Press, Some(Button::WheelDown), Modifiers::empty()),
            Some(b"\x1b[<65;1;1M".to_vec())
        );
        assert_eq!(
            sgr(Action::Release, Some(Button::WheelUp), Modifiers::empty()),
            None,
            "a wheel notch has no release"
        );
    }

    #[test]
    fn tracking_mode_decides_which_events_are_reported() {
        let event = |tracking, action, button| {
            report(
                tracking,
                MouseEncoding::Sgr,
                action,
                button,
                Modifiers::empty(),
                4,
                2,
            )
            .is_some()
        };
        let left = Some(Button::Left);
        assert!(!event(MouseTracking::Off, Action::Press, left));
        assert!(event(MouseTracking::Clicks, Action::Press, left));
        assert!(event(MouseTracking::Clicks, Action::Release, left));
        assert!(!event(MouseTracking::Clicks, Action::Motion, left));
        assert!(event(MouseTracking::Drag, Action::Motion, left));
        assert!(!event(MouseTracking::Drag, Action::Motion, None));
        assert!(event(MouseTracking::Motion, Action::Motion, None));
    }

    #[test]
    fn legacy_encodings_use_offset_bytes_and_refuse_unreachable_columns() {
        let legacy = |encoding, action, col| {
            report(
                MouseTracking::Clicks,
                encoding,
                action,
                Some(Button::Left),
                Modifiers::empty(),
                col,
                0,
            )
        };
        assert_eq!(
            legacy(MouseEncoding::Normal, Action::Press, 0),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(
            legacy(MouseEncoding::Normal, Action::Release, 0),
            Some(vec![0x1b, b'[', b'M', 35, 33, 33]),
            "legacy releases carry button 3"
        );
        assert_eq!(
            legacy(MouseEncoding::Normal, Action::Press, 222).map(|bytes| bytes[4]),
            Some(255)
        );
        assert_eq!(legacy(MouseEncoding::Normal, Action::Press, 223), None);
        assert_eq!(
            legacy(MouseEncoding::Utf8, Action::Press, 300),
            Some("\x1b[M \u{14d}!".as_bytes().to_vec()),
            "column 301 + 32 is the two-byte U+014D"
        );
    }

    #[test]
    fn wheel_becomes_arrow_keys_in_the_applications_cursor_mode() {
        assert_eq!(wheel_arrows(true, false, 2), b"\x1b[A\x1b[A");
        assert_eq!(wheel_arrows(false, true, 1), b"\x1bOB");
        assert!(wheel_arrows(true, false, 0).is_empty());
    }
}
