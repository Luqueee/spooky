//! The dropdown window: where it goes, when it hides, and the global shortcut that
//! brings it back.

mod dropdown;
mod hotkey;
mod slide;

pub(crate) use dropdown::{
    dropdown_origin, hides_on_blur, initial_position, level_for, usable_area,
};
pub(crate) use hotkey::{Hotkeys, StartError};
pub(crate) use slide::{FIRST_DELAY, Slide};
