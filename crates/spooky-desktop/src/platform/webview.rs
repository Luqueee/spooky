//! The agent view's web page where there is one (macOS); elsewhere the native view is used
//! and none of this does anything.

#[cfg(target_os = "macos")]
pub(crate) use super::macos::webview::{
    create, focus, focus_composer, messages, paste, place, push, show, supported,
};

#[cfg(not(target_os = "macos"))]
mod absent {
    use iced::Rectangle;

    pub(crate) fn supported() -> bool {
        false
    }

    pub(crate) fn create(_: &dyn iced::window::Window) -> Result<(), String> {
        Err("The web agent view is only available on macOS".into())
    }

    pub(crate) fn place(_: Rectangle) {}

    pub(crate) fn show(_: bool) {}

    pub(crate) fn focus() {}

    pub(crate) fn push(_: &str) {}

    pub(crate) fn focus_composer() {}

    pub(crate) fn paste(_: &str) {}

    pub(crate) fn messages() -> impl iced::futures::Stream<Item = String> {
        iced::futures::stream::empty()
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) use absent::{
    create, focus, focus_composer, messages, paste, place, push, show, supported,
};
