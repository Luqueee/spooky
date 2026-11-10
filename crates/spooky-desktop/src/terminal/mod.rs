//! The terminal as the user touches it: the canvas that draws a pane, and the
//! translation of pointer, keyboard, paste and selection into what programs read and
//! what the clipboard holds.

mod canvas;
mod copy;
mod input;
mod links;
mod pointer;

pub(crate) use canvas::*;
pub(crate) use copy::{new_clipboard_writes, selection_text};
pub(crate) use input::{
    ClipboardAction, clipboard_action, focus_reports, is_chord_echo, key_bytes, paste_bytes,
    terminal_text,
};
