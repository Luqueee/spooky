//! The colours of the interface: the built-in palettes, and a palette derived from a few
//! base colours, which is how custom themes are made.

use super::Color;
use crate::config::Appearance;

pub(crate) const BACKGROUND: Color = Color::from_rgb8(
    spooky_core::DEFAULT_BACKGROUND[0],
    spooky_core::DEFAULT_BACKGROUND[1],
    spooky_core::DEFAULT_BACKGROUND[2],
);
pub(crate) const SIDEBAR: Color = Color::from_rgb8(24, 27, 34);
pub(crate) const HOVER: Color = Color::from_rgb8(36, 40, 49);
pub(crate) const SELECTED: Color = Color::from_rgb8(43, 47, 57);
pub(crate) const BORDER: Color = Color::from_rgb8(48, 52, 62);
pub(crate) const TEXT: Color = Color::from_rgb8(207, 211, 220);
pub(crate) const MUTED: Color = Color::from_rgb8(119, 125, 138);
pub(crate) const ACCENT: Color = Color::from_rgb8(160, 173, 200);
pub(crate) const SUCCESS: Color = Color::from_rgb8(125, 185, 156);
pub(crate) const DANGER: Color = Color::from_rgb8(226, 133, 143);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ColorPalette {
    pub background: Color,
    pub sidebar: Color,
    pub hover: Color,
    pub selected: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

impl ColorPalette {
    pub(crate) fn from_base(
        background: Color,
        text: Color,
        accent: Color,
        success: Color,
        warning: Color,
        danger: Color,
    ) -> Self {
        Self {
            background,
            sidebar: blend(background, text, 0.035),
            hover: blend(background, text, 0.09),
            selected: blend(background, accent, 0.19),
            border: blend(background, text, 0.14),
            text,
            muted: blend(text, background, 0.43),
            accent,
            success,
            warning,
            danger,
        }
    }
}

pub(super) fn blend(from: Color, to: Color, amount: f32) -> Color {
    let inverse = 1.0 - amount;
    Color {
        r: from.r * inverse + to.r * amount,
        g: from.g * inverse + to.g * amount,
        b: from.b * inverse + to.b * amount,
        a: from.a * inverse + to.a * amount,
    }
}

pub(crate) fn color_palette(appearance: Appearance) -> ColorPalette {
    let (
        background,
        sidebar,
        hover,
        selected,
        border,
        text,
        muted,
        accent,
        success,
        warning,
        danger,
    ) = match appearance {
        Appearance::Midnight => {
            return ColorPalette::from_base(
                Color::from_rgb8(15, 16, 20),
                Color::from_rgb8(226, 229, 235),
                Color::from_rgb8(156, 176, 212),
                Color::from_rgb8(126, 190, 156),
                Color::from_rgb8(224, 189, 123),
                Color::from_rgb8(222, 125, 133),
            );
        }
        Appearance::Spooky => (
            BACKGROUND,
            SIDEBAR,
            HOVER,
            SELECTED,
            BORDER,
            TEXT,
            MUTED,
            ACCENT,
            SUCCESS,
            Color::from_rgb8(218, 184, 126),
            DANGER,
        ),
        Appearance::Nord => (
            Color::from_rgb8(46, 52, 64),
            Color::from_rgb8(39, 45, 56),
            Color::from_rgb8(59, 66, 82),
            Color::from_rgb8(67, 76, 94),
            Color::from_rgb8(76, 86, 106),
            Color::from_rgb8(229, 233, 240),
            Color::from_rgb8(163, 169, 183),
            Color::from_rgb8(136, 192, 208),
            Color::from_rgb8(163, 190, 140),
            Color::from_rgb8(235, 203, 139),
            Color::from_rgb8(191, 97, 106),
        ),
        Appearance::Dracula => (
            Color::from_rgb8(40, 42, 54),
            Color::from_rgb8(33, 34, 44),
            Color::from_rgb8(52, 55, 70),
            Color::from_rgb8(68, 71, 90),
            Color::from_rgb8(80, 82, 110),
            Color::from_rgb8(248, 248, 242),
            Color::from_rgb8(168, 169, 185),
            Color::from_rgb8(189, 147, 249),
            Color::from_rgb8(80, 250, 123),
            Color::from_rgb8(241, 250, 140),
            Color::from_rgb8(255, 85, 85),
        ),
    };
    ColorPalette {
        background,
        sidebar,
        hover,
        selected,
        border,
        text,
        muted,
        accent,
        success,
        warning,
        danger,
    }
}

/// Every built-in palette, the default first.
#[cfg(test)]
pub(crate) const APPEARANCES: [Appearance; 4] = [
    Appearance::Midnight,
    Appearance::Spooky,
    Appearance::Nord,
    Appearance::Dracula,
];

#[cfg(test)]
mod tests {
    use super::{APPEARANCES, ColorPalette, color_palette};
    use crate::config::Appearance;

    #[test]
    fn the_builtin_palettes_differ_where_they_should() {
        assert_ne!(
            color_palette(Appearance::Spooky).background,
            color_palette(Appearance::Nord).background
        );
        assert_ne!(
            color_palette(Appearance::Nord).background,
            color_palette(Appearance::Dracula).background
        );
        for appearance in APPEARANCES {
            let palette = color_palette(appearance);
            assert_ne!(
                palette.background, palette.text,
                "{appearance:?} needs contrast"
            );
        }
    }

    #[test]
    fn a_derived_palette_keeps_its_base_colours() {
        let base = color_palette(Appearance::Nord);
        let derived = ColorPalette::from_base(
            base.background,
            base.text,
            base.accent,
            base.success,
            base.warning,
            base.danger,
        );
        assert_eq!(derived.background, base.background);
        assert_eq!(derived.accent, base.accent);
        assert_ne!(derived.hover, derived.background);
    }
}
