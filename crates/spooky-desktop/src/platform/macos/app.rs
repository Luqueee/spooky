//! Process-level macOS integration.

use crate::geometry::{Point, Rectangle, Size};
use objc2::{AnyThread, MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSEvent,
    NSFloatingWindowLevel, NSImage, NSImageView, NSNormalWindowLevel, NSScreen,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowAnimationBehavior, NSWindowButton, NSWindowOrderingMode, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use std::cell::RefCell;

/// Sets the running application's Dock artwork independently of the bundle icon cache.
pub(crate) fn set_dock_artwork() -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let Some(executable) = std::env::current_exe().ok() else {
        return false;
    };
    let Some(contents) = executable.parent().and_then(std::path::Path::parent) else {
        return false;
    };
    let icon_path = contents.join("Resources/SpookyLogo.png");
    let Some(icon_path) = icon_path.to_str() else {
        return false;
    };
    let icon_path = NSString::from_str(icon_path);
    let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &icon_path) else {
        return false;
    };
    let tile = NSApplication::sharedApplication(main_thread).dockTile();
    let view = NSImageView::imageViewWithImage(&image, main_thread);
    view.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), tile.size()));
    tile.setContentView(Some(&view));
    tile.display();
    true
}

/// Shows or hides the Dock icon (and menu bar). Spooky runs as an accessory
/// application by default, as dropdown terminals usually do: no Dock icon, and
/// windows still take keyboard focus.
///
/// Must run on the main thread. Returns whether the
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

/// The display under the pointer, in global top-left coordinates, and its
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

/// Places the one Spooky window in AppKit points, in one step, so the frame is never
/// converted with the scale of the display the window is leaving.
/// Returns false rather than moving another window if the target is ambiguous.
pub(crate) fn place_dropdown(origin: Point, size: Size) -> bool {
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

/// The one window titled Spooky; none when there is not exactly one.
fn spooky_window(main_thread: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let windows = NSApplication::sharedApplication(main_thread).windows();
    let mut matches = windows
        .into_iter()
        .filter(|window| window.title().to_string() == "Spooky");
    let window = matches.next()?;
    matches.next().is_none().then_some(window)
}

/// Puts the window on screen, or takes it off. Must run on the main thread.
pub(crate) fn set_visible(visible: bool) {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let Some(window) = spooky_window(main_thread) else {
        return;
    };
    if visible {
        window.makeKeyAndOrderFront(None);
    } else {
        window.orderOut(None);
    }
}

/// Brings the window back from the Dock. Must run on the main thread.
pub(crate) fn unminimize() {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    if let Some(window) = spooky_window(main_thread)
        && window.isMiniaturized()
    {
        window.deminiaturize(None);
    }
}

/// Whether the window is minimized to the Dock. Must run on the main thread.
pub(crate) fn is_minimized() -> Option<bool> {
    let main_thread = MainThreadMarker::new()?;
    Some(spooky_window(main_thread)?.isMiniaturized())
}

/// Moves the window's top left corner to `origin`, in global top-left points.
/// Must run on the main thread.
pub(crate) fn move_to(origin: Point) {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    let screens = NSScreen::screens(main_thread);
    let (Some(primary), Some(window)) = (screens.firstObject(), spooky_window(main_thread)) else {
        return;
    };
    let primary = primary.frame();
    let primary_top = primary.origin.y + primary.size.height;
    let height = window.frame().size.height;
    window.setFrameOrigin(NSPoint::new(
        f64::from(origin.x),
        primary_top - f64::from(origin.y) - height,
    ));
}

/// Floats the window above others, or stacks it normally. Must run on the main thread.
pub(crate) fn set_always_on_top(on_top: bool) {
    let Some(main_thread) = MainThreadMarker::new() else {
        return;
    };
    if let Some(window) = spooky_window(main_thread) {
        window.setLevel(if on_top {
            NSFloatingWindowLevel
        } else {
            NSNormalWindowLevel
        });
    }
}

/// Whether the system asks for solid backgrounds instead of transparency.
pub(crate) fn reduce_transparency() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}

thread_local! {
    /// The behind-window material, once installed under the window's content.
    static GLASS: RefCell<Option<Retained<NSVisualEffectView>>> = const { RefCell::new(None) };
}

/// Shows or hides a native behind-window material (a dark blur of what is behind the
/// window) under the content, for a translucent background. It is a sibling below the
/// content view, so it never covers what is drawn or takes input. Must run on the main
/// thread; returns whether the material exists, which it does not until the one window
/// does.
pub(crate) fn set_glass(on: bool) -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    GLASS.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let Some(window) = spooky_window(main_thread) else {
                return false;
            };
            let Some(content) = window.contentView() else {
                return false;
            };
            let Some(close_button) = window.standardWindowButton(NSWindowButton::CloseButton)
            else {
                return false;
            };
            let Some(parent) = content.ancestorSharedWithView(&close_button) else {
                return false;
            };
            if !parent.subviews().iter().any(|view| *view == *content) {
                return false;
            }
            let material = NSVisualEffectView::new(main_thread);
            material.setFrame(content.frame());
            material.setAutoresizingMask(
                NSAutoresizingMaskOptions::ViewWidthSizable
                    | NSAutoresizingMaskOptions::ViewHeightSizable,
            );
            material.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
            material.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
            material.setState(NSVisualEffectState::Active);
            material.setHidden(true);
            parent.addSubview_positioned_relativeTo(
                &material,
                NSWindowOrderingMode::Below,
                Some(&content),
            );
            *slot = Some(material);
        }
        if let Some(material) = slot.as_ref()
            && material.isHidden() == on
        {
            material.setHidden(!on);
        }
        true
    })
}
