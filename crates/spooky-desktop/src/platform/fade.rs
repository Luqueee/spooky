//! The window's overall opacity, for the fade that goes with the dropdown's slide.
//!
//! Wayland offers a client no way to do this, and on KDE the window manager fades the
//! window instead (see `kde`), so here it does nothing. A leaf module: it depends on no
//! feature.

use iced::window::Window;

/// Learns which native window to fade. Call once the window exists, from
/// `iced::window::run`.
pub(crate) fn attach(window: &dyn Window) {
    imp::attach(window);
}

/// Sets the window's opacity, from 0 (invisible) to 1 (opaque).
pub(crate) fn set(opacity: f32) {
    imp::set(opacity);
}

#[cfg(target_os = "linux")]
mod imp {
    //! X11: the `_NET_WM_WINDOW_OPACITY` property, which compositing window managers apply.

    use iced::window::{Window, raw_window_handle::RawWindowHandle};
    use std::sync::Mutex;
    use x11rb::{
        connection::Connection,
        protocol::xproto::{AtomEnum, ConnectionExt, PropMode},
        rust_connection::RustConnection,
        wrapper::ConnectionExt as _,
    };

    struct Target {
        connection: RustConnection,
        window: u32,
        atom: u32,
    }

    static TARGET: Mutex<Option<Target>> = Mutex::new(None);

    pub(super) fn attach(window: &dyn Window) {
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
        let Ok((connection, _)) = RustConnection::connect(None) else {
            return;
        };
        let Some(atom) = connection
            .intern_atom(false, b"_NET_WM_WINDOW_OPACITY")
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.atom)
        else {
            return;
        };
        if let Ok(mut target) = TARGET.lock() {
            *target = Some(Target {
                connection,
                window: id,
                atom,
            });
        }
    }

    pub(super) fn set(opacity: f32) {
        let Ok(target) = TARGET.lock() else {
            return;
        };
        let Some(target) = target.as_ref() else {
            return;
        };
        if opacity >= 1.0 {
            let _ = target
                .connection
                .delete_property(target.window, target.atom);
        } else {
            let value = (opacity.clamp(0.0, 1.0) * u32::MAX as f32) as u32;
            let _ = target.connection.change_property32(
                PropMode::REPLACE,
                target.window,
                target.atom,
                AtomEnum::CARDINAL,
                &[value],
            );
        }
        let _ = target.connection.flush();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use iced::window::Window;

    pub(super) fn attach(_: &dyn Window) {}

    pub(super) fn set(opacity: f32) {
        super::super::macos::app::set_opacity(opacity);
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    use iced::window::Window;

    pub(super) fn attach(_: &dyn Window) {}

    pub(super) fn set(_: f32) {}
}
