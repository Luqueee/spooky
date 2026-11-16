//! The shared visual language: colours, styles and icons. A leaf module: it depends on
//! no feature.

mod icons;
mod theme;
mod themes;
pub(crate) use themes::{ThemeCatalog, ThemeDefinition, themes_directory, valid_theme_id};

pub(crate) use icons::*;
pub(crate) use theme::*;
