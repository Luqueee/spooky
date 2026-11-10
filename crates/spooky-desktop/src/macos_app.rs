//! Process-level macOS integration.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSWindowAnimationBehavior};

/// Runs Spooky as an accessory application: no Dock icon and no menu bar, as
/// dropdown terminals usually do. Windows still take keyboard focus.
///
/// Must run on the main thread, where iced evaluates updates. Returns whether the
/// policy was applied.
pub(crate) fn hide_dock_icon() -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    NSApplication::sharedApplication(main_thread)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory)
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
