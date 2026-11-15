//! Native behind-window material for the desktop's translucent background.
//!
//! All AppKit access happens on the main thread. The effect is a sibling below
//! winit's content view, so it never covers the renderer or intercepts input.

use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindowButton,
    NSWindowOrderingMode, NSWorkspace,
};
use std::cell::RefCell;

thread_local! {
    static MATERIAL: RefCell<Option<(Retained<NSVisualEffectView>, Retained<NSView>)>> = const { RefCell::new(None) };
}

/// Installs the material only when the application has exactly one Spooky window.
pub(crate) fn install() -> bool {
    MATERIAL.with(|slot| {
        if slot.borrow().is_some() {
            return true;
        }
        let Some(main_thread) = MainThreadMarker::new() else {
            return false;
        };
        let windows = NSApplication::sharedApplication(main_thread).windows();
        let mut matching = windows
            .into_iter()
            .filter(|window| window.title().to_string() == "Spooky");
        let Some(window) = matching.next() else {
            return false;
        };
        if matching.next().is_some() {
            return false;
        }
        let Some(content) = window.contentView() else {
            return false;
        };
        let Some(close_button) = window.standardWindowButton(NSWindowButton::CloseButton) else {
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
        *slot.borrow_mut() = Some((material, content));
        true
    })
}

/// The OS accessibility choice takes precedence over the configured opacity.
pub(crate) fn reduce_transparency() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceTransparency()
}

/// Enables the material only while its transparent backing is being used.
pub(crate) fn show(visible: bool) {
    MATERIAL.with(|slot| {
        if let Some((material, _)) = slot.borrow().as_ref() {
            material.setHidden(!visible);
        }
    });
}

/// Keeps the native blur aligned with the content during the dropdown slide.
pub(crate) fn slide(hidden_share: f32) {
    MATERIAL.with(|slot| {
        if let Some((material, content)) = slot.borrow().as_ref() {
            let mut frame = content.frame();
            frame.origin.y += frame.size.height * f64::from(hidden_share);
            material.setFrame(frame);
        }
    });
}
