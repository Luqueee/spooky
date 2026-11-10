//! The dropdown window: where it goes, when it hides, and the global shortcut that
//! brings it back.

mod dropdown;
mod hotkey;

pub(crate) use dropdown::{
    dropdown_origin, hides_on_blur, initial_position, level_for, usable_area,
};
pub(crate) use hotkey::{Hotkeys, StartError};
