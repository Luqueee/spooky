//! Code that exists because of one operating system or desktop. A leaf module: it depends
//! on no feature.

pub(crate) mod desktop;
#[cfg(target_os = "linux")]
pub(crate) mod kwin;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "linux")]
pub(crate) mod portal;
pub(crate) mod webview;
pub(crate) mod window;

pub(crate) use desktop::{WindowControl, window_control};
pub(crate) use window::reduce_transparency;

/// The application id: the window class the window gets (which the KWin script matches),
/// the portal's application id, and the name of the desktop entry.
pub(crate) const APP_ID: &str = "dev.spooky.terminal";
