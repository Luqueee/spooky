//! Code that exists because of one operating system. A leaf module: it depends on no
//! feature.

#[cfg(target_os = "linux")]
pub(crate) mod kde;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
pub(crate) mod webview;
#[cfg(not(target_os = "linux"))]
pub(crate) mod kde {
    //! KDE Plasma integration exists only on Linux.

    /// What the window manager is asked to do with the window.
    #[derive(Clone, Copy)]
    pub(crate) enum Dim {
        Keep,
        Px(u32),
        Percent { percent: f32, min: u32 },
    }

    pub(crate) enum Action {
        Reveal {
            width: Dim,
            height: Dim,
            above: bool,
        },
        Hide,
    }

    pub(crate) fn active() -> bool {
        false
    }

    pub(crate) fn send(_: Action) {}

    pub(crate) fn bind_shortcut(_: &str, _: impl Fn() + Send + 'static) -> Result<(), String> {
        Err("KDE shortcuts are only available on Linux".into())
    }
}
