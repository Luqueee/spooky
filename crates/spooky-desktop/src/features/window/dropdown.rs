//! Where the dropdown window goes, and what it does when it loses the focus.

use crate::config::OnBlur;
use iced::{Point, Size, window};

/// Where a dropdown window of `window` size goes on a display of `monitor` size:
/// horizontally centred against the top edge. `None` when the display is unknown,
/// leaving placement to the system.
pub(crate) fn dropdown_origin(monitor: Option<Size>, window: Size) -> Option<Point> {
    let monitor = monitor?;
    Some(Point::new(
        ((monitor.width - window.width) / 2.0).max(0.0),
        0.0,
    ))
}
/// [`dropdown_origin`] as the window's starting position, so it is created where it
/// stays instead of appearing elsewhere and moving. Before the display is known it
/// falls back to the top-left corner.
pub(crate) fn initial_position(window: Size, monitor: Size) -> Point {
    dropdown_origin(Some(monitor), window).unwrap_or(Point::ORIGIN)
}
/// Whether losing the keyboard focus should hide the window: only when asked to, while
/// the window is shown, no native dialog took the focus, and the global shortcut can
/// bring it back.
pub(crate) fn hides_on_blur(
    on_blur: OnBlur,
    hidden: bool,
    dialog_open: bool,
    has_shortcut: bool,
) -> bool {
    on_blur == OnBlur::Hide && !hidden && !dialog_open && has_shortcut
}
/// The part of the display `monitor` that a window may cover. On macOS that leaves out
/// the menu bar and a visible Dock, so a "full height" window does not hang below the
/// screen; elsewhere the whole display is used.
pub(crate) fn usable_area(monitor: Size) -> Size {
    #[cfg(target_os = "macos")]
    if let Some((width, height)) = crate::platform::macos::app::usable_area() {
        return Size::new(width.min(monitor.width), height.min(monitor.height));
    }
    monitor
}
/// The window level a focus-loss behaviour calls for.
pub(crate) fn level_for(on_blur: OnBlur) -> window::Level {
    if on_blur == OnBlur::Top {
        window::Level::AlwaysOnTop
    } else {
        window::Level::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dropdown_is_centred_against_the_top_and_never_starts_off_screen() {
        let window = Size::new(1440.0, 900.0);
        assert_eq!(
            dropdown_origin(Some(Size::new(1920.0, 1080.0)), window),
            Some(Point::new(240.0, 0.0))
        );
        assert_eq!(
            dropdown_origin(Some(Size::new(1280.0, 800.0)), window),
            Some(Point::new(0.0, 0.0)),
            "a window wider than its display starts at the left edge"
        );
        assert_eq!(dropdown_origin(None, window), None);
    }

    #[test]
    fn losing_focus_hides_only_a_shown_window_that_can_come_back() {
        use crate::config::OnBlur;
        // (on_blur, hidden, dialog_open, has_shortcut)
        assert!(hides_on_blur(OnBlur::Hide, false, false, true));
        assert!(!hides_on_blur(OnBlur::None, false, false, true));
        assert!(!hides_on_blur(OnBlur::Top, false, false, true));
        assert!(
            !hides_on_blur(OnBlur::Hide, true, false, true),
            "already hidden"
        );
        assert!(
            !hides_on_blur(OnBlur::Hide, false, true, true),
            "a file dialog took the focus, not another app"
        );
        assert!(
            !hides_on_blur(OnBlur::Hide, false, false, false),
            "without the global shortcut a hidden window is lost"
        );
    }
}
