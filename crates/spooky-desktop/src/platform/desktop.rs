//! Which kind of session the application runs in, and so who can place, hide and raise
//! its window and who registers its global shortcut. Decided once, from the environment.

use std::sync::OnceLock;

/// The session, as far as the window and the global shortcut are concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Session {
    /// Linux on X11, whatever the desktop: the window manager honours what the client
    /// asks of its window, and the application grabs its own key.
    X11,
    /// KDE Plasma on Wayland: a client can do neither, so KWin and the desktop portal
    /// do both for it.
    KdeWayland,
    /// Any other Wayland compositor (GNOME, sway, …).
    Wayland,
    /// macOS and anything else: the windowing library places the window and the
    /// application grabs its own key.
    Native,
}

/// Who places, hides and raises the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowControl {
    /// The application, through the windowing library.
    Client,
    /// KWin, asked over D-Bus (`kwin`).
    Kwin,
    /// Nobody: the compositor leaves it to the user.
    Unavailable,
}

impl Session {
    /// Works the session out from environment variables, given as a lookup so it can be
    /// tested.
    pub(crate) fn detect(var: impl Fn(&str) -> Option<String>) -> Self {
        if !cfg!(target_os = "linux") {
            return Self::Native;
        }
        if var("WAYLAND_DISPLAY").is_none() {
            return Self::X11;
        }
        let kde = var("XDG_CURRENT_DESKTOP")
            .is_some_and(|desktop| desktop.split(':').any(|d| d.eq_ignore_ascii_case("KDE")));
        if kde { Self::KdeWayland } else { Self::Wayland }
    }

    /// Who places, hides and raises the window.
    pub(crate) fn window_control(self) -> WindowControl {
        match self {
            Self::X11 | Self::Native => WindowControl::Client,
            Self::KdeWayland => WindowControl::Kwin,
            Self::Wayland => WindowControl::Unavailable,
        }
    }
}

/// The session this process runs in.
pub(crate) fn session() -> Session {
    static SESSION: OnceLock<Session> = OnceLock::new();
    *SESSION.get_or_init(|| Session::detect(|name| std::env::var(name).ok()))
}

/// Who places, hides and raises the window in this session.
pub(crate) fn window_control() -> WindowControl {
    session().window_control()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_session_follows_the_display_server_and_the_desktop() {
        assert_eq!(Session::detect(env(&[("DISPLAY", ":0")])), Session::X11);
        assert_eq!(
            Session::detect(env(&[("DISPLAY", ":0"), ("XDG_CURRENT_DESKTOP", "KDE")])),
            Session::X11,
            "on X11 the desktop does not matter"
        );
        assert_eq!(
            Session::detect(env(&[
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("XDG_CURRENT_DESKTOP", "KDE")
            ])),
            Session::KdeWayland
        );
        assert_eq!(
            Session::detect(env(&[
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")
            ])),
            Session::Wayland
        );
        assert_eq!(
            Session::detect(env(&[("WAYLAND_DISPLAY", "wayland-0")])),
            Session::Wayland,
            "an unnamed desktop is not assumed to be KDE"
        );
    }

    #[test]
    fn only_kde_wayland_has_a_window_manager_to_ask() {
        assert_eq!(Session::X11.window_control(), WindowControl::Client);
        assert_eq!(Session::Native.window_control(), WindowControl::Client);
        assert_eq!(Session::KdeWayland.window_control(), WindowControl::Kwin);
        assert_eq!(
            Session::Wayland.window_control(),
            WindowControl::Unavailable
        );
    }
}
