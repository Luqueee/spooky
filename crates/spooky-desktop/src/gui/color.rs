//! Conversions from the application's colours to GPUI's.

use crate::ui::Color;
use gpui::{Hsla, Rgba};

/// The colour GPUI paints with.
pub(crate) fn hsla(color: Color) -> Hsla {
    Rgba {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a,
    }
    .into()
}
