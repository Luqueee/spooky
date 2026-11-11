//! Features of the application. Each is a folder with its own state, message type,
//! update logic and view. Features do not import one another: they meet in `App`,
//! which composes them, and talk through messages and small traits it implements.

pub(crate) mod agent;
pub(crate) mod attachments;
pub(crate) mod connection;
pub(crate) mod hyperlinks;
pub(crate) mod palette;
pub(crate) mod search;
pub(crate) mod settings;
pub(crate) mod splits;
pub(crate) mod tailscale;
pub(crate) mod window;
