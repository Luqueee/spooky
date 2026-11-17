//! What the windowing library offers no way to do to the one window: show and hide it,
//! move it, keep it on top, ask whether it is minimized, and set its overall opacity for
//! the dropdown's fade.
//!
//! On macOS this is AppKit; on X11 it is the window manager's properties and messages. On
//! Wayland a client can do none of it, and on KDE the window manager does it instead (see
//! `kwin`), so there these do nothing. A leaf module: it depends on no feature.

use crate::geometry::Point;
use raw_window_handle::HasWindowHandle;

/// Learns which native window the rest of this module acts on. Call once the window
/// exists.
pub(crate) fn attach(window: &dyn HasWindowHandle) {
    imp::attach(window);
}

/// Sets the window's opacity, from 0 (invisible) to 1 (opaque).
pub(crate) fn set_opacity(opacity: f32) {
    imp::set_opacity(opacity);
}

/// Puts the window on screen, or takes it off.
pub(crate) fn set_visible(visible: bool) {
    imp::set_visible(visible);
}

/// Brings the window back from the minimized state.
pub(crate) fn unminimize() {
    imp::unminimize();
}

/// Whether the window is minimized, when that can be told.
pub(crate) fn is_minimized() -> Option<bool> {
    imp::is_minimized()
}

/// Moves the window's top left corner to a position in the desktop's pixels.
pub(crate) fn move_to(at: Point) {
    imp::move_to(at);
}

/// Keeps the window above the others, or lets it stack normally.
pub(crate) fn set_always_on_top(on_top: bool) {
    imp::set_always_on_top(on_top);
}

/// Whether the system asks for solid backgrounds instead of transparency.
pub(crate) fn reduce_transparency() -> bool {
    imp::reduce_transparency()
}

#[cfg(target_os = "linux")]
mod imp {
    use crate::geometry::Point;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::sync::Mutex;
    use x11rb::{
        connection::Connection,
        protocol::xproto::{
            AtomEnum, ClientMessageEvent, ConfigureWindowAux, ConnectionExt, EventMask, PropMode,
            StackMode,
        },
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    struct Target {
        connection: RustConnection,
        root: u32,
        window: u32,
    }

    static TARGET: Mutex<Option<Target>> = Mutex::new(None);

    pub(super) fn attach(window: &dyn HasWindowHandle) {
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let id = match handle.as_raw() {
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window).ok(),
            RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
            _ => None,
        };
        let Some(id) = id else {
            return;
        };
        let Ok((connection, screen)) = RustConnection::connect(None) else {
            return;
        };
        let root = connection.setup().roots[screen].root;
        if let Ok(mut target) = TARGET.lock() {
            *target = Some(Target {
                connection,
                root,
                window: id,
            });
        }
    }

    fn atom(target: &Target, name: &[u8]) -> Option<u32> {
        target
            .connection
            .intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|reply| reply.atom)
    }

    fn with_target<R>(act: impl FnOnce(&Target) -> Option<R>) -> Option<R> {
        let target = TARGET.lock().ok()?;
        act(target.as_ref()?)
    }

    pub(super) fn set_opacity(opacity: f32) {
        with_target(|target| {
            let property = atom(target, b"_NET_WM_WINDOW_OPACITY")?;
            if opacity >= 1.0 {
                target
                    .connection
                    .delete_property(target.window, property)
                    .ok()?;
            } else {
                let value = (opacity.clamp(0.0, 1.0) * u32::MAX as f32) as u32;
                target
                    .connection
                    .change_property32(
                        PropMode::REPLACE,
                        target.window,
                        property,
                        AtomEnum::CARDINAL,
                        &[value],
                    )
                    .ok()?;
            }
            target.connection.flush().ok()
        });
    }

    pub(super) fn set_visible(visible: bool) {
        with_target(|target| {
            if visible {
                target.connection.map_window(target.window).ok()?;
            } else {
                target.connection.unmap_window(target.window).ok()?;
            }
            target.connection.flush().ok()
        });
    }

    pub(super) fn unminimize() {
        with_target(|target| {
            target.connection.map_window(target.window).ok()?;
            target
                .connection
                .configure_window(
                    target.window,
                    &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
                )
                .ok()?;
            target.connection.flush().ok()
        });
    }

    pub(super) fn is_minimized() -> Option<bool> {
        with_target(|target| {
            let state = atom(target, b"_NET_WM_STATE")?;
            let hidden = atom(target, b"_NET_WM_STATE_HIDDEN")?;
            let reply = target
                .connection
                .get_property(false, target.window, state, AtomEnum::ATOM, 0, 64)
                .ok()?
                .reply()
                .ok()?;
            Some(reply.value32()?.any(|value| value == hidden))
        })
    }

    pub(super) fn move_to(at: Point) {
        with_target(|target| {
            target
                .connection
                .configure_window(
                    target.window,
                    &ConfigureWindowAux::new()
                        .x(at.x.round() as i32)
                        .y(at.y.round() as i32),
                )
                .ok()?;
            target.connection.flush().ok()
        });
    }

    pub(super) fn set_always_on_top(on_top: bool) {
        with_target(|target| {
            let state = atom(target, b"_NET_WM_STATE")?;
            let above = atom(target, b"_NET_WM_STATE_ABOVE")?;
            let event = ClientMessageEvent::new(
                32,
                target.window,
                state,
                [u32::from(on_top), above, 0, 1, 0],
            );
            target
                .connection
                .send_event(
                    false,
                    target.root,
                    EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )
                .ok()?;
            target.connection.flush().ok()
        });
    }

    pub(super) fn reduce_transparency() -> bool {
        false
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::{geometry::Point, platform::macos::app};
    use raw_window_handle::HasWindowHandle;

    pub(super) fn attach(_: &dyn HasWindowHandle) {}

    pub(super) fn set_opacity(opacity: f32) {
        app::set_opacity(opacity);
    }

    pub(super) fn set_visible(visible: bool) {
        app::set_visible(visible);
    }

    pub(super) fn unminimize() {
        app::unminimize();
    }

    pub(super) fn is_minimized() -> Option<bool> {
        app::is_minimized()
    }

    pub(super) fn move_to(at: Point) {
        app::move_to(at);
    }

    pub(super) fn set_always_on_top(on_top: bool) {
        app::set_always_on_top(on_top);
    }

    pub(super) fn reduce_transparency() -> bool {
        app::reduce_transparency()
    }

    /// Shows or hides the native blur under a translucent window.
    pub(crate) fn set_glass(on: bool) -> bool {
        app::set_glass(on)
    }
}

#[cfg(target_os = "macos")]
pub(crate) use imp::set_glass;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use crate::geometry::Point;
    use raw_window_handle::HasWindowHandle;

    pub(super) fn attach(_: &dyn HasWindowHandle) {}
    pub(super) fn set_opacity(_: f32) {}
    pub(super) fn set_visible(_: bool) {}
    pub(super) fn unminimize() {}
    pub(super) fn is_minimized() -> Option<bool> {
        None
    }
    pub(super) fn move_to(_: Point) {}
    pub(super) fn set_always_on_top(_: bool) {}
    pub(super) fn reduce_transparency() -> bool {
        false
    }
}
