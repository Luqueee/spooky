//! Colours, the application theme and button and container styles.

use crate::Message;
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

pub fn theme() -> Theme {
    static THEME: std::sync::LazyLock<Theme> = std::sync::LazyLock::new(|| {
        Theme::custom(
            "Spooky",
            iced::theme::Palette {
                background: BACKGROUND,
                text: TEXT,
                primary: ACCENT,
                success: SUCCESS,
                warning: Color::from_rgb8(218, 184, 126),
                danger: DANGER,
            },
        )
    });
    THEME.clone()
}

pub fn surface(color: Color) -> container::Style {
    container::Style {
        background: Some(color.into()),
        text_color: Some(TEXT),
        ..Default::default()
    }
}

pub fn quiet(_: &Theme, status: button::Status) -> button::Style {
    button::Style {
        background: match status {
            button::Status::Hovered | button::Status::Pressed => Some(HOVER.into()),
            _ => None,
        },
        text_color: if status == button::Status::Disabled {
            MUTED.scale_alpha(0.4)
        } else {
            MUTED
        },
        border: Border {
            radius: 4.0.into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

pub fn selectable(selected: bool, theme: &Theme, status: button::Status) -> button::Style {
    let mut style = quiet(theme, status);
    if selected {
        style.background = Some(SELECTED.into());
        style.text_color = TEXT;
        style.border.radius = 4.0.into();
    }
    style
}

pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = quiet(theme, status);
    style.background = Some(
        if status == button::Status::Hovered {
            HOVER
        } else {
            SELECTED
        }
        .into(),
    );
    style.border.color = BORDER;
    style.border.width = 1.0;
    style.text_color = TEXT;
    style
}

pub fn destructive(theme: &Theme, status: button::Status) -> button::Style {
    let mut style = quiet(theme, status);
    style.text_color = DANGER;
    style
}

pub fn separator() -> Element<'static, Message> {
    container(iced::widget::Space::new().height(1))
        .width(Length::Fill)
        .style(|_| surface(BORDER))
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
