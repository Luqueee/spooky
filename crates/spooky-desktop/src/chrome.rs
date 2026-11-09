//! Shared visual tokens and small vector controls for the native shell.
use crate::Message;
use iced::{
    Alignment, Border, Color, Element, Length, Point, Rectangle, Theme, mouse,
    widget::{button, canvas, container, text, tooltip},
};
use std::time::Duration;

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

#[derive(Clone, Copy)]
pub enum Icon {
    Ghost,
    Folder,
    Plus,
    Terminal,
    SplitColumns,
    SplitRows,
    Paperclip,
    Camera,
    More,
    Sidebar,
    Server,
    Chevron,
    Close,
    Pencil,
}

struct Vector {
    icon: Icon,
    color: Color,
}
impl canvas::Program<Message> for Vector {
    type State = ();
    fn draw(
        &self,
        _: &(),
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        frame.scale(bounds.width / 24.0);
        let stroke = canvas::Stroke::default()
            .with_color(self.color)
            .with_width(1.6);
        let path = canvas::Path::new(|p| {
            let mut line = |points: &[(f32, f32)]| {
                if let Some(&(x, y)) = points.first() {
                    p.move_to(Point::new(x, y));
                }
                for &(x, y) in points.iter().skip(1) {
                    p.line_to(Point::new(x, y));
                }
            };
            match self.icon {
                Icon::Plus => {
                    line(&[(12., 5.), (12., 19.)]);
                    line(&[(5., 12.), (19., 12.)]);
                }
                Icon::Terminal => {
                    line(&[(5., 6.), (10., 11.), (5., 16.)]);
                    line(&[(12., 17.), (19., 17.)]);
                }
                Icon::Folder => line(&[
                    (3., 18.),
                    (3., 6.),
                    (9., 6.),
                    (11., 8.),
                    (21., 8.),
                    (21., 18.),
                    (3., 18.),
                ]),
                Icon::SplitColumns | Icon::SplitRows | Icon::Sidebar | Icon::Server => {
                    line(&[(4., 4.), (20., 4.), (20., 20.), (4., 20.), (4., 4.)]);
                    match self.icon {
                        Icon::SplitRows => line(&[(4., 12.), (20., 12.)]),
                        Icon::Sidebar => line(&[(10., 4.), (10., 20.)]),
                        Icon::Server => {
                            line(&[(4., 12.), (20., 12.)]);
                            line(&[(7., 8.), (9., 8.)]);
                            line(&[(7., 16.), (9., 16.)]);
                        }
                        _ => line(&[(12., 4.), (12., 20.)]),
                    }
                }
                Icon::Paperclip => line(&[
                    (9., 15.),
                    (16., 8.),
                    (18., 10.),
                    (10., 18.),
                    (6., 18.),
                    (4., 16.),
                    (4., 12.),
                    (14., 2.),
                    (18., 2.),
                    (22., 6.),
                    (22., 10.),
                    (12., 20.),
                ]),
                Icon::Camera => line(&[
                    (3., 8.),
                    (8., 8.),
                    (10., 5.),
                    (15., 5.),
                    (17., 8.),
                    (21., 8.),
                    (21., 20.),
                    (3., 20.),
                    (3., 8.),
                ]),
                Icon::Chevron => line(&[(8., 10.), (12., 14.), (16., 10.)]),
                Icon::Close => {
                    line(&[(6., 6.), (18., 18.)]);
                    line(&[(18., 6.), (6., 18.)]);
                }
                Icon::Pencil => line(&[
                    (5., 16.),
                    (5., 20.),
                    (9., 20.),
                    (20., 9.),
                    (16., 5.),
                    (5., 16.),
                    (9., 20.),
                ]),
                Icon::Ghost => {
                    p.move_to(Point::new(5., 20.));
                    p.line_to(Point::new(5., 10.));
                    p.bezier_curve_to(
                        Point::new(5., 1.),
                        Point::new(19., 1.),
                        Point::new(19., 10.),
                    );
                    for (x, y) in [(19., 20.), (15.5, 17.), (12., 20.), (8.5, 17.), (5., 20.)] {
                        p.line_to(Point::new(x, y));
                    }
                }
                Icon::More => {}
            }
        });
        frame.stroke(&path, stroke);
        match self.icon {
            Icon::Ghost => {
                for x in [9., 15.] {
                    frame.fill(&canvas::Path::circle(Point::new(x, 10.), 1.), self.color);
                }
            }
            Icon::Camera => frame.stroke(&canvas::Path::circle(Point::new(12., 14.), 3.5), stroke),
            Icon::More => {
                for x in [5., 12., 19.] {
                    frame.fill(&canvas::Path::circle(Point::new(x, 12.), 1.1), self.color);
                }
            }
            _ => {}
        }
        vec![frame.into_geometry()]
    }
}

pub fn icon(kind: Icon, color: Color) -> Element<'static, Message> {
    canvas(Vector { icon: kind, color })
        .width(12)
        .height(12)
        .into()
}

pub fn action(kind: Icon, label: &'static str, message: Message) -> Element<'static, Message> {
    tooltip(
        button(icon(kind, MUTED))
            .padding(5)
            .style(quiet)
            .on_press(message),
        container(text(label).size(11))
            .padding([5, 8])
            .style(|_| container::Style {
                border: Border {
                    color: BORDER,
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..surface(HOVER)
            }),
        tooltip::Position::Bottom,
    )
    .delay(Duration::from_millis(450))
    .into()
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
