//! The terminal's font: which family, and how wide its cells are.

use iced::{
    Font,
    advanced::graphics::text::{
        cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping},
        font_system,
    },
};
use std::sync::Mutex;

#[cfg(target_os = "macos")]
const DEFAULT_FAMILY: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
const DEFAULT_FAMILY: &str = "DejaVu Sans Mono";

/// The width of a cell, as a share of the font size, when it cannot be measured. It is
/// the advance of both default families.
const DEFAULT_ADVANCE: f32 = 0.602;

/// The terminal's font and the width of one cell in it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerminalFont {
    pub font: Font,
    /// The width of a cell, as a share of the font size.
    pub advance: f32,
}

impl Default for TerminalFont {
    fn default() -> Self {
        Self::resolve(None)
    }
}

impl TerminalFont {
    /// The font for `family` (the platform's default when `None`) with its cell width
    /// measured, because fonts differ: a font wider than the cell would overlap itself.
    pub(crate) fn resolve(family: Option<&str>) -> Self {
        let name = family.unwrap_or(DEFAULT_FAMILY);
        Self {
            font: Font::with_name(intern(name)),
            advance: measure(name).unwrap_or(DEFAULT_ADVANCE),
        }
    }
}

/// `Font::with_name` wants a name that lives for the whole run. Each distinct name is
/// kept once, so choosing fonts over and over does not grow memory.
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let Ok(mut names) = NAMES.lock() else {
        return DEFAULT_FAMILY;
    };
    if let Some(known) = names.iter().find(|known| **known == name) {
        return known;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.push(leaked);
    leaked
}

/// The advance of a capital M in `family`, as a share of the font size.
fn measure(family: &str) -> Option<f32> {
    const SIZE: f32 = 100.0;
    let mut system = font_system().write().ok()?;
    let system = system.raw();
    let mut buffer = Buffer::new(system, Metrics::new(SIZE, SIZE * 1.2));
    buffer.set_text(
        system,
        "M",
        &Attrs::new().family(Family::Name(family)),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(system, false);
    let width = buffer.layout_runs().next()?.line_w / SIZE;
    (width.is_finite() && width > 0.1 && width < 2.0).then_some(width)
}

/// The monospace families installed, sorted and without duplicates.
pub(crate) fn installed_monospace() -> Vec<String> {
    let Ok(mut system) = font_system().write() else {
        return Vec::new();
    };
    let mut families: Vec<String> = system
        .raw()
        .db()
        .faces()
        .filter(|face| face.monospaced)
        .filter_map(|face| face.families.first().map(|(name, _)| name.clone()))
        .collect();
    families.sort_unstable();
    families.dedup();
    families
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_kept_once_however_often_it_is_chosen() {
        let first = intern("Spooky Test Font");
        let second = intern("Spooky Test Font");
        assert!(std::ptr::eq(first, second));
    }

    #[test]
    fn every_installed_monospace_family_has_a_measurable_cell() {
        for family in installed_monospace() {
            let advance = measure(&family);
            assert!(advance.is_some_and(|a| a > 0.2 && a < 1.2), "{family}");
        }
    }

    #[test]
    fn a_cell_has_a_sensible_width_even_for_a_font_that_is_not_installed() {
        let font = TerminalFont::resolve(Some("No Such Font Anywhere"));
        assert!(font.advance > 0.1 && font.advance < 2.0);
    }
}
