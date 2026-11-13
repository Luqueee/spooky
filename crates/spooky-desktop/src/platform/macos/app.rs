//! Process-level macOS integration.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWindowAnimationBehavior};

/// Shows or hides the Dock icon (and menu bar). Spooky runs as an accessory
/// application by default, as dropdown terminals usually do: no Dock icon, and
/// windows still take keyboard focus.
///
/// Must run on the main thread, where iced evaluates updates. Returns whether the
/// policy was applied.
pub(crate) fn set_dock_icon(show: bool) -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(main_thread).setActivationPolicy(if show {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    })
}

/// Makes every window appear and disappear at once. By default macOS scales a window
/// up a couple of percent as it is shown, which reads as the layout shifting.
///
/// Must run on the main thread. Returns whether it was applied.
pub(crate) fn show_windows_instantly() -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    for window in NSApplication::sharedApplication(main_thread).windows() {
        window.setAnimationBehavior(NSWindowAnimationBehavior::None);
    }
    true
}

/// Sets every window's opacity, from 0 (invisible) to 1 (opaque).
///
/// Must run on the main thread.
pub(crate) fn set_opacity(opacity: f32) {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    for window in NSApplication::sharedApplication(main_thread).windows() {
        window.setAlphaValue(f64::from(opacity));
    }
}

/// The width and height of the part of the window's display that windows may use:
/// without the menu bar and a visible Dock. A window as tall as the display would
/// hang below the screen.
///
/// Must run on the main thread. `None` when there is no window yet or it has no display.
pub(crate) fn usable_area() -> Option<(f32, f32)> {
    let main_thread = MainThreadMarker::new()?;
    let windows = NSApplication::sharedApplication(main_thread).windows();
    let area = windows.firstObject()?.screen()?.visibleFrame();
    Some((area.size.width as f32, area.size.height as f32))
}
