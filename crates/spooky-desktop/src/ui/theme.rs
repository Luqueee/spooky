//! Colours, the application theme and button and container styles.

use crate::{Message, config::Appearance};
use iced::{
    Alignment, Border, Color, Element, Length, Theme,
    widget::{button, container},
};

pub const BACKGROUND: Color = Color::from_rgb8(
    spooky_core::DEFAULT_BACKGROUND[0],
    spooky_core::DEFAULT_BACKGROUND[1],
    spooky_core::DEFAULT_BACKGROUND[2],
);
pub const SIDEBAR: Color = Color::from_rgb8(24, 27, 34);
pub const HOVER: Color = Color::from_rgb8(36, 40, 49);
pub const SELECTED: Color = Color::from_rgb8(43, 47, 57);
pub const BORDER: Color = Color::from_rgb8(48, 52, 62);
pub const TEXT: Color = Color::from_rgb8(207, 211, 220);
pub const MUTED: Color = Color::from_rgb8(119, 125, 138);
pub const ACCENT: Color = Color::from_rgb8(160, 173, 200);
pub const SUCCESS: Color = Color::from_rgb8(125, 185, 156);
pub const DANGER: Color = Color::from_rgb8(226, 133, 143);

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

#[derive(Clone, Copy)]
pub(crate) enum TextTone {
    Text,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

pub(crate) fn text_tone(tone: TextTone) -> impl Fn(&Theme) -> iced::widget::text::Style {
    move |theme| {
        let colors = color_palette_for(theme);
        let color = match tone {
            TextTone::Text => colors.text,
            TextTone::Muted => colors.muted,
            TextTone::Accent => colors.accent,
            TextTone::Success => colors.success,
            TextTone::Warning => colors.warning,
            TextTone::Danger => colors.danger,
        };
        iced::widget::text::Style { color: Some(color) }
    }
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

fn blend(from: Color, to: Color, amount: f32) -> Color {
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

pub(crate) fn color_palette_for(theme: &Theme) -> ColorPalette {
    let palette = theme.palette();
    for appearance in [Appearance::Spooky, Appearance::Nord, Appearance::Dracula] {
        let builtin = color_palette(appearance);
        if palette.background == builtin.background
            && palette.text == builtin.text
            && palette.primary == builtin.accent
            && palette.success == builtin.success
            && palette.warning == builtin.warning
            && palette.danger == builtin.danger
        {
            return builtin;
        }
    }
    ColorPalette::from_base(
        palette.background,
        palette.text,
        palette.primary,
        palette.success,
        palette.warning,
        palette.danger,
    )
}

pub fn theme(appearance: Appearance) -> Theme {
    let name = match appearance {
        Appearance::Spooky => "Spooky",
        Appearance::Nord => "Nord",
        Appearance::Dracula => "Dracula",
    };
    theme_with_palette(name, color_palette(appearance))
}

pub(crate) fn theme_with_palette(name: &str, colors: ColorPalette) -> Theme {
    Theme::custom(
        name.to_owned(),
        iced::theme::Palette {
            background: colors.background,
            text: colors.text,
            primary: colors.accent,
            success: colors.success,
            warning: colors.warning,
            danger: colors.danger,
        },
    )
}

pub fn surface(color: Color) -> container::Style {
    container::Style {
        background: Some(color.into()),
        text_color: None,
        ..Default::default()
    }
}

pub fn quiet(theme: &Theme, status: button::Status) -> button::Style {
    let colors = color_palette_for(theme);
    button::Style {
        background: match status {
            button::Status::Hovered | button::Status::Pressed => Some(colors.hover.into()),
            _ => None,
        },
        text_color: if status == button::Status::Disabled {
            colors.muted.scale_alpha(0.4)
        } else {
            colors.muted
        },
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn selectable(selected: bool, theme: &Theme, status: button::Status) -> button::Style {
    let colors = color_palette_for(theme);
    let mut style = quiet(theme, status);
    if selected {
        style.background = Some(colors.selected.into());
        style.text_color = colors.text;
        style.border.radius = 4.0.into();
    }
    style
}

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let colors = color_palette_for(theme);
    let mut style = quiet(theme, status);
    style.background = Some(
        if status == button::Status::Hovered {
            colors.hover
        } else {
            colors.selected
        }
        .into(),
    );
    style.border.color = colors.border;
    style.border.width = 1.0;
    style.text_color = colors.text;
    style
}

pub fn destructive(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = quiet(theme, status);
    style.text_color = color_palette_for(theme).danger;
    style
}

pub fn separator() -> Element<'static, Message> {
    container(iced::widget::Space::new().height(1))
        .width(Length::Fill)
        .style(|theme| surface(color_palette_for(theme).border))
        .into()
}

pub fn dot(color: Color) -> Element<'static, Message> {
    container(iced::widget::Space::new().width(4).height(4))
        .style(move |_| container::Style {
            border: Border {
                radius: 3.0.into(),
                ..Default::default()
            },
            ..surface(color)
        })
        .align_y(Alignment::Center)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{color_palette, color_palette_for, theme};
    use crate::config::Appearance;

    #[test]
    fn each_builtin_theme_selects_its_matching_surfaces() {
        for appearance in [Appearance::Spooky, Appearance::Nord, Appearance::Dracula] {
            let colors = color_palette(appearance);
            assert_eq!(color_palette_for(&theme(appearance)), colors);
        }
        assert_ne!(
            color_palette(Appearance::Spooky).background,
            color_palette(Appearance::Nord).background
        );
        assert_ne!(
            color_palette(Appearance::Nord).background,
            color_palette(Appearance::Dracula).background
        );
    }
}
