//! The graphical shell: it owns the window, draws the application with GPUI and turns what
//! the user and the system do into the application's messages.
//!
//! `shell` holds [`Root`], the window's view: it owns the `App`, carries out the tasks it
//! returns and feeds it events. The rest is the toolkit-specific vocabulary the views are
//! written in: colours, icons, text fields, the terminal pane and the helpers in `view`.

mod color;
mod icons;
pub(crate) mod keys;
mod shell;
mod terminal;
mod text_input;
mod view;

pub(crate) use color::hsla;
pub(crate) use icons::{Icon, icon};
pub(crate) use shell::{Launch, Root, Sender, Ui, open};
pub(crate) use terminal::{TerminalView, Typing, scroll_thumb};
pub(crate) use text_input::Spec;
pub(crate) use view::{Look, Tone, View, column, geometry_point, row, tone};
