//! The terminal as the user touches it: the geometry of its cells and the translation of
//! pointer, keyboard, paste and selection into what programs read and what the clipboard
//! holds. What draws a pane is in `gui`.

mod capture;
mod copy;
mod font;
mod input;
mod links;
mod pointer;

pub(crate) use capture::render as capture;
pub(crate) use copy::{new_clipboard_writes, selection_text};
pub(crate) use font::{DEFAULT_FAMILY, TerminalFont, installed_monospace};
pub(crate) use input::{ClipboardAction, clipboard_action, focus_reports, key_bytes, paste_bytes};
pub(crate) use links::{Link, link_at};
pub(crate) use pointer::{Action, Button, report, wheel_arrows};

/// The width and height of a cell at `size` in a font whose cells are `advance` wide.
pub(crate) fn metrics(size: f32, advance: f32) -> (f32, f32) {
    (size * advance, size * 1.35)
}
