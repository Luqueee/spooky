//! The colours of programs running in a terminal, as a theme draws them.
//!
//! The server resolves every colour a program asks for to RGB and sends that. Its
//! sixteen ANSI colours, its default text colour and its 256-colour cube are fixed
//! ([`spooky_core::ANSI_PALETTE`], [`spooky_core::DEFAULT_FOREGROUND`]); a theme
//! replaces them here, when a pane is drawn, by recognising those exact values. Three
//! consequences follow, all by design:
//!
//! - Black and bright white are never replaced. They are the values 24-bit programs
//!   use most (`#000000`, `#ffffff`), and the wire cannot tell the two apart. The other
//!   fourteen collide with a 24-bit colour only if a program picked the very same RGB.
//! - Text the program dims is scaled to two thirds by the server before it arrives; the
//!   scaled values are recognised too, and replaced by the theme's colour scaled the
//!   same way.
//! - A colour of the 256-colour cube or grey ramp (16 to 255) is replaced only when the
//!   theme names that index, so a theme opts into each collision with a 24-bit colour.

use super::{Color, ColorPalette};
use spooky_core::{ANSI_PALETTE, DEFAULT_FOREGROUND};

/// The names a theme file uses for the colours it may set, in ANSI order. Black and
/// bright white are absent: see the module documentation.
pub(crate) const NAMES: [(&str, usize); 14] = [
    ("red", 1),
    ("green", 2),
    ("yellow", 3),
    ("blue", 4),
    ("magenta", 5),
    ("cyan", 6),
    ("white", 7),
    ("bright_black", 8),
    ("bright_red", 9),
    ("bright_green", 10),
    ("bright_yellow", 11),
    ("bright_blue", 12),
    ("bright_magenta", 13),
    ("bright_cyan", 14),
];

/// Indices of the ANSI colours that are never replaced.
const KEPT: [usize; 2] = [0, 15];

/// The palette a theme draws terminal text with. The default is the server's own, which
/// changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalColors {
    ansi: [[u8; 3]; 16],
    foreground: [u8; 3],
    /// Replacements for indices 16 to 255, where the theme names one.
    indexed: [Option<[u8; 3]>; 240],
}

impl Default for TerminalColors {
    fn default() -> Self {
        Self {
            ansi: ANSI_PALETTE,
            foreground: DEFAULT_FOREGROUND,
            indexed: [None; 240],
        }
    }
}

impl TerminalColors {
    /// Colours chosen to sit with an interface palette: its status colours for red,
    /// green and yellow, its accent for blue, and mixes of those for the rest.
    pub(crate) fn derive(palette: &ColorPalette) -> Self {
        let lift = |color: Color| super::palette::blend(color, palette.text, 0.25);
        let magenta = super::palette::blend(palette.danger, palette.accent, 0.5);
        let cyan = super::palette::blend(palette.success, palette.accent, 0.5);
        let white = super::palette::blend(palette.text, palette.background, 0.15);
        let mut colors = Self::default();
        let chosen = [
            (1, palette.danger),
            (2, palette.success),
            (3, palette.warning),
            (4, palette.accent),
            (5, magenta),
            (6, cyan),
            (7, white),
            (8, palette.muted),
            (9, lift(palette.danger)),
            (10, lift(palette.success)),
            (11, lift(palette.warning)),
            (12, lift(palette.accent)),
            (13, lift(magenta)),
            (14, lift(cyan)),
        ];
        for (index, color) in chosen {
            colors.ansi[index] = color.rgb8();
        }
        colors.foreground = palette.text.rgb8();
        colors
    }

    /// Replaces the colour `name` (one of [`NAMES`], or `foreground`). Returns false for
    /// any other name.
    pub(crate) fn set(&mut self, name: &str, rgb: [u8; 3]) -> bool {
        if name == "foreground" {
            self.foreground = rgb;
            return true;
        }
        if let Some(index) = name
            .strip_prefix("color")
            .filter(|digits| !digits.starts_with('0') && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<usize>().ok())
            .filter(|index| (16..=255).contains(index))
        {
            self.indexed[index - 16] = Some(rgb);
            return true;
        }
        match NAMES.iter().find(|(known, _)| *known == name) {
            Some(&(_, index)) => {
                self.ansi[index] = rgb;
                true
            }
            None => false,
        }
    }

    /// The Nord scheme's own terminal colours.
    pub(crate) fn nord() -> Self {
        Self::scheme(
            0xd8dee9,
            [
                0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0, 0x4c566a,
                0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb,
            ],
        )
        .with_indexed(208, 0xd08770)
    }

    /// The Dracula scheme's own terminal colours.
    pub(crate) fn dracula() -> Self {
        Self::scheme(
            0xf8f8f2,
            [
                0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2, 0x6272a4,
                0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff,
            ],
        )
        .with_indexed(208, 0xffb86c)
    }

    fn with_indexed(mut self, index: usize, hex: u32) -> Self {
        self.indexed[index - 16] = Some([(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]);
        self
    }

    /// A scheme given as `0xRRGGBB` values for ANSI 1 to 14.
    fn scheme(foreground: u32, ansi: [u32; 14]) -> Self {
        let rgb = |hex: u32| [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8];
        let mut colors = Self::default();
        for (offset, hex) in ansi.into_iter().enumerate() {
            colors.ansi[offset + 1] = rgb(hex);
        }
        colors.foreground = rgb(foreground);
        colors
    }

    /// The colour to draw for one RGB value the server sent.
    pub(crate) fn resolve(&self, rgb: [u8; 3]) -> [u8; 3] {
        if rgb == DEFAULT_FOREGROUND {
            return self.foreground;
        }
        if let Some(index) = ANSI_PALETTE.iter().position(|server| *server == rgb) {
            return if KEPT.contains(&index) {
                rgb
            } else {
                self.ansi[index]
            };
        }
        if rgb == dim(DEFAULT_FOREGROUND) {
            return dim(self.foreground);
        }
        if let Some(index) = (1..15).find(|index| dim(ANSI_PALETTE[*index]) == rgb) {
            return dim(self.ansi[index]);
        }
        indexed_position(rgb)
            .and_then(|index| self.indexed[index - 16])
            .unwrap_or(rgb)
    }
}

/// A colour as the server dims it: each channel to two thirds.
fn dim(rgb: [u8; 3]) -> [u8; 3] {
    rgb.map(|channel| (u16::from(channel) * 2 / 3) as u8)
}

/// The index (16 to 255) the server resolved to `rgb`, if it is one of the cube's or
/// the grey ramp's colours.
fn indexed_position(rgb: [u8; 3]) -> Option<usize> {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let level = |channel: u8| LEVELS.iter().position(|known| *known == channel);
    if let (Some(red), Some(green), Some(blue)) = (level(rgb[0]), level(rgb[1]), level(rgb[2])) {
        return Some(16 + 36 * red + 6 * green + blue);
    }
    let [red, green, blue] = rgb;
    (red == green && green == blue && (8..=238).contains(&red) && (red - 8) % 10 == 0)
        .then(|| 232 + usize::from((red - 8) / 10))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Appearance, ui::color_palette};

    #[test]
    fn the_default_changes_no_colour() {
        let colors = TerminalColors::default();
        for rgb in ANSI_PALETTE
            .into_iter()
            .chain([DEFAULT_FOREGROUND, [1, 2, 3], [0, 0, 0]])
        {
            assert_eq!(colors.resolve(rgb), rgb);
        }
    }

    #[test]
    fn a_scheme_replaces_the_ansi_colours_and_the_default_text() {
        let nord = TerminalColors::nord();
        assert_eq!(nord.resolve(ANSI_PALETTE[1]), [0xbf, 0x61, 0x6a]);
        assert_eq!(nord.resolve(ANSI_PALETTE[8]), [0x4c, 0x56, 0x6a]);
        assert_eq!(nord.resolve(DEFAULT_FOREGROUND), [0xd8, 0xde, 0xe9]);
    }

    #[test]
    fn black_white_and_other_rgb_values_are_left_alone() {
        let nord = TerminalColors::nord();
        assert_eq!(nord.resolve([0, 0, 0]), [0, 0, 0]);
        assert_eq!(nord.resolve([255, 255, 255]), [255, 255, 255]);
        assert_eq!(nord.resolve([204, 49, 49]), [204, 49, 49]);
    }

    #[test]
    fn set_accepts_only_the_names_a_theme_may_use() {
        let mut colors = TerminalColors::default();
        assert!(colors.set("bright_cyan", [1, 2, 3]));
        assert_eq!(colors.resolve(ANSI_PALETTE[14]), [1, 2, 3]);
        assert!(colors.set("foreground", [4, 5, 6]));
        assert_eq!(colors.resolve(DEFAULT_FOREGROUND), [4, 5, 6]);
        for refused in ["black", "bright_white", "purple", ""] {
            assert!(!colors.set(refused, [9, 9, 9]), "{refused}");
        }
    }

    #[test]
    fn a_derived_palette_takes_its_status_colours_from_the_interface() {
        let palette = color_palette(Appearance::Nord);
        let colors = TerminalColors::derive(&palette);
        assert_eq!(colors.resolve(ANSI_PALETTE[1]), palette.danger.rgb8());
        assert_eq!(colors.resolve(ANSI_PALETTE[2]), palette.success.rgb8());
        assert_eq!(colors.resolve(ANSI_PALETTE[4]), palette.accent.rgb8());
        assert_eq!(colors.resolve(DEFAULT_FOREGROUND), palette.text.rgb8());
        assert_ne!(
            colors.resolve(ANSI_PALETTE[9]),
            colors.resolve(ANSI_PALETTE[1])
        );
    }

    #[test]
    fn dimmed_text_is_replaced_by_the_themes_colour_dimmed_the_same_way() {
        let nord = TerminalColors::nord();
        // The server dims red (205, 49, 49) to two thirds.
        assert_eq!(nord.resolve([136, 32, 32]), dim([0xbf, 0x61, 0x6a]));
        assert_eq!(
            nord.resolve(dim(DEFAULT_FOREGROUND)),
            dim([0xd8, 0xde, 0xe9])
        );
        assert_eq!(
            TerminalColors::default().resolve([136, 32, 32]),
            [136, 32, 32]
        );
    }

    #[test]
    fn only_the_indices_a_theme_names_are_replaced() {
        // Index 208 is (255, 135, 0); 209 is (255, 135, 95).
        assert_eq!(indexed_position([255, 135, 0]), Some(208));
        assert_eq!(indexed_position([255, 135, 95]), Some(209));
        assert_eq!(indexed_position([8, 8, 8]), Some(232));
        assert_eq!(indexed_position([238, 238, 238]), Some(255));
        assert_eq!(indexed_position([1, 2, 3]), None);
        let dracula = TerminalColors::dracula();
        assert_eq!(dracula.resolve([255, 135, 0]), [0xff, 0xb8, 0x6c]);
        assert_eq!(dracula.resolve([255, 135, 95]), [255, 135, 95]);
        let mut mine = TerminalColors::default();
        assert!(mine.set("color232", [1, 2, 3]));
        assert_eq!(mine.resolve([8, 8, 8]), [1, 2, 3]);
        for refused in [
            "color15", "color256", "color016", "color", "color2x", "color-1",
        ] {
            assert!(!mine.set(refused, [9, 9, 9]), "{refused}");
        }
    }
}
