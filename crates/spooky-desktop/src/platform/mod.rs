//! Code that exists because of one operating system or desktop. A leaf module: it depends
//! on no feature.

pub(crate) mod desktop;
pub(crate) mod fade;
#[cfg(target_os = "linux")]
pub(crate) mod kwin;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "linux")]
pub(crate) mod portal;
pub(crate) mod webview;

pub(crate) use desktop::{WindowControl, window_control};

/// The application id: the window class winit gives the window (which the KWin script
/// matches), the portal's application id, and the name of the desktop entry.
#[cfg(target_os = "linux")]
pub(crate) const APP_ID: &str = "dev.spooky.terminal";
