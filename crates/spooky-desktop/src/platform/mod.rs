//! Code that exists because of one operating system. A leaf module: it depends on no
//! feature.

#[cfg(target_os = "macos")]
pub(crate) mod macos;
