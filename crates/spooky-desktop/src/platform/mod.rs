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

    pub(crate) fn active() -> bool {
        false
    }

    pub(crate) fn bind_shortcut(_: &str, _: impl Fn() + Send + 'static) -> Result<(), String> {
        Err("KDE shortcuts are only available on Linux".into())
    }
}
