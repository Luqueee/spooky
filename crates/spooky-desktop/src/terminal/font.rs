//! The terminal's font: which family, and how wide its cells are.

use std::sync::{Arc, LazyLock};

#[cfg(target_os = "macos")]
pub(crate) const DEFAULT_FAMILY: &str = "Menlo";
#[cfg(not(target_os = "macos"))]
pub(crate) const DEFAULT_FAMILY: &str = "DejaVu Sans Mono";

/// The width of a cell, as a share of the font size, when it cannot be measured. It is
/// the advance of both default families.
const DEFAULT_ADVANCE: f32 = 0.602;

/// The fonts installed, read once.
static FONTS: LazyLock<fontdb::Database> = LazyLock::new(|| {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();
    database
});

/// The terminal's font and the width of one cell in it.
#[derive(Clone, Debug)]
pub(crate) struct TerminalFont {
    pub family: Arc<str>,
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
            family: Arc::from(name),
            advance: measure(name).unwrap_or(DEFAULT_ADVANCE),
        }
    }
}

/// The font file of one style of `family`, and which face of a collection it is.
pub(super) fn face_data(family: &str, bold: bool, italic: bool) -> Option<(Vec<u8>, u32)> {
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        weight: if bold {
            fontdb::Weight::BOLD
        } else {
            fontdb::Weight::NORMAL
        },
        style: if italic {
            fontdb::Style::Italic
        } else {
            fontdb::Style::Normal
        },
        ..fontdb::Query::default()
    };
    let id = FONTS.query(&query)?;
    FONTS.with_face_data(id, |data, index| (data.to_vec(), index))
}
/// The advance of a capital M in `family`, as a share of the font size.
fn measure(family: &str) -> Option<f32> {
    let query = fontdb::Query {
        families: &[fontdb::Family::Name(family)],
        ..fontdb::Query::default()
    };
    let id = FONTS.query(&query)?;
    let width = FONTS.with_face_data(id, |data, index| {
        let face = ttf_parser::Face::parse(data, index).ok()?;
        let glyph = face.glyph_index('M')?;
        let advance = face.glyph_hor_advance(glyph)?;
        Some(f32::from(advance) / f32::from(face.units_per_em()))
    })??;
    (width.is_finite() && width > 0.1 && width < 2.0).then_some(width)
}

/// The monospace families installed, sorted and without duplicates.
pub(crate) fn installed_monospace() -> Vec<String> {
    let mut families: Vec<String> = FONTS
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
    fn every_installed_monospace_family_resolves_to_a_sensible_cell() {
        for family in installed_monospace() {
            // A family with no usable capital M (a bitmap font, say) falls back to the
            // default advance; one that measures must measure a plausible cell.
            if let Some(advance) = measure(&family) {
                assert!(advance > 0.2 && advance < 1.2, "{family}");
            }
            let font = TerminalFont::resolve(Some(&family));
            assert!(font.advance > 0.1 && font.advance < 2.0, "{family}");
        }
    }

    #[test]
    fn a_cell_has_a_sensible_width_even_for_a_font_that_is_not_installed() {
        let font = TerminalFont::resolve(Some("No Such Font Anywhere"));
        assert!(font.advance > 0.1 && font.advance < 2.0);
    }
}
