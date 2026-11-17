//! The shared visual language that belongs to no toolkit: colours and themes. A leaf
//! module: it depends on no feature. What draws with them is in `gui`.

mod color;
mod palette;
mod themes;

pub(crate) use color::Color;
pub(crate) use palette::{ColorPalette, color_palette};
pub(crate) use themes::{ThemeCatalog, themes_directory, valid_theme_id};
