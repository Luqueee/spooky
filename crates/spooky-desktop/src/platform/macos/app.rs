//! Process-level macOS integration.

use iced::{Rectangle, Size};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSScreen, NSWindowAnimationBehavior,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

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

/// The display under the pointer, in iced's global top-left coordinates, and its
/// usable size. AppKit's global origin is the primary screen's bottom-left.
/// Must run on the main thread.
pub(crate) fn pointer_display() -> Option<(Rectangle, Size)> {
    let main_thread = MainThreadMarker::new()?;
    let screens = NSScreen::screens(main_thread);
    let primary = screens.firstObject()?.frame();
    let primary_top = primary.origin.y + primary.size.height;
    let pointer = NSEvent::mouseLocation();
    for screen in screens {
        let frame = screen.frame();
        if pointer.x >= frame.origin.x
            && pointer.x < frame.origin.x + frame.size.width
            && pointer.y >= frame.origin.y
            && pointer.y < frame.origin.y + frame.size.height
        {
            let visible = screen.visibleFrame();
            return Some((
                Rectangle {
                    x: frame.origin.x as f32,
                    y: (primary_top - frame.origin.y - frame.size.height) as f32,
                    width: frame.size.width as f32,
                    height: frame.size.height as f32,
                },
                Size::new(visible.size.width as f32, visible.size.height as f32),
            ));
        }
    }
    None
}

/// Whether the uniquely identified dropdown is actually visible on this Space.
/// Must run on the main thread; an ambiguous or missing window is unknown.
pub(crate) fn dropdown_visible() -> Option<bool> {
    let main_thread = MainThreadMarker::new()?;
    let app = NSApplication::sharedApplication(main_thread);
    let mut windows = app
        .windows()
        .into_iter()
        .filter(|window| window.title().to_string() == "Spooky");
    let window = windows.next()?;
    if windows.next().is_some() {
        return None;
    }
    Some(!app.isHidden() && window.isVisible() && window.isOnActiveSpace())
}

/// Places the one Spooky window in AppKit points. Going through iced here would
/// convert with the old screen's scale before the backing-scale event arrives.
/// Returns false rather than moving another window if the target is ambiguous.
pub(crate) fn place_dropdown(origin: iced::Point, size: Size) -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let screens = NSScreen::screens(main_thread);
    let Some(primary) = screens.firstObject() else {
        return false;
    };
    let primary = primary.frame();
    let primary_top = primary.origin.y + primary.size.height;
    let windows = NSApplication::sharedApplication(main_thread).windows();
    let mut matches = windows
        .into_iter()
        .filter(|window| window.title().to_string() == "Spooky");
    let Some(window) = matches.next() else {
        return false;
    };
    if matches.next().is_some() {
        return false;
    }
    let frame = NSRect::new(
        NSPoint::new(
            f64::from(origin.x),
            primary_top - f64::from(origin.y) - f64::from(size.height),
        ),
        NSSize::new(f64::from(size.width), f64::from(size.height)),
    );
    window.setFrame_display(frame, true);
    true
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
