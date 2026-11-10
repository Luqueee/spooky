//! Layout-aware literal Option input without first-party native FFI.

use iced::keyboard::{Modifiers, key::Physical};
use iced_winit::winit::{keyboard::PhysicalKey, platform::scancode::PhysicalKeyExtScancode};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSTextInputContext};
use objc2_foundation::{NSPoint, NSString};
use std::{collections::HashMap, sync::LazyLock};

// Reverse winit's platform mapping once instead of maintaining a second key table.
static SCANCODES: LazyLock<HashMap<Physical, u16>> = LazyLock::new(|| {
    (0..128)
        .map(|code| {
            (
                iced_winit::conversion::physical_key(PhysicalKey::from_scancode(u32::from(code))),
                code,
            )
        })
        .collect()
});

pub(crate) fn option_text(physical: Physical, modifiers: Modifiers) -> Option<String> {
    let code = *SCANCODES.get(&physical)?;
    let mut flags = NSEventModifierFlags::Option;
    flags.set(NSEventModifierFlags::Shift, modifiers.shift());
    let empty = NSString::new();
    let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyDown,
        NSPoint::ZERO,
        flags,
        0.0,
        0,
        None,
        &empty,
        &empty,
        false,
        code,
    )?;
    let text = event.charactersByApplyingModifiers(flags)?.to_string();
    if text.is_empty() {
        return None;
    }
    // The terminal commits a spacing glyph now, not an accent for the next key.
    if let Some(main_thread) = objc2::MainThreadMarker::new()
        && let Some(context) = NSTextInputContext::currentInputContext(main_thread)
    {
        context.discardMarkedText();
    }
    Some(text)
}
