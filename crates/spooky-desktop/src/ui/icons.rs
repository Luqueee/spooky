//! Vector icons and the small icon buttons built from them.

use super::theme::{BORDER, HOVER, MUTED, quiet, surface};
use crate::Message;
use iced::{
    Border, Color, Element, Point, Rectangle, Theme, mouse,
    widget::{button, canvas, container, text, tooltip},
};
use std::time::Duration;

#[derive(Clone, Copy)]
pub enum Icon {
    Ghost,
    Folder,
    Plus,
    Terminal,
    SplitColumns,
    SplitRows,
    More,
    Sidebar,
    Server,
    Chevron,
    ChevronRight,
    Close,
    Pencil,
    Settings,
    Search,
    ChevronUp,
    Copy,
    Paste,
    Agent,
    Send,
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
                Icon::Chevron => line(&[(8., 10.), (12., 14.), (16., 10.)]),
                Icon::ChevronRight => line(&[(10., 8.), (14., 12.), (10., 16.)]),
                Icon::ChevronUp => line(&[(8., 14.), (12., 10.), (16., 14.)]),
                Icon::Search => line(&[(15.5, 15.5), (20., 20.)]),
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
                Icon::Settings => {
                    line(&[(4., 7.), (20., 7.)]);
                    line(&[(4., 12.), (20., 12.)]);
                    line(&[(4., 17.), (20., 17.)]);
                }
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
                Icon::Copy => {
                    line(&[(9., 9.), (20., 9.), (20., 20.), (9., 20.), (9., 9.)]);
                    line(&[(5., 15.), (5., 4.), (15., 4.), (15., 9.)]);
                }
                Icon::Send => {
                    line(&[(12., 19.), (12., 5.)]);
                    line(&[(6., 11.), (12., 5.), (18., 11.)]);
                }
                Icon::Agent => line(&[
                    (12., 3.),
                    (14., 10.),
                    (21., 12.),
                    (14., 14.),
                    (12., 21.),
                    (10., 14.),
                    (3., 12.),
                    (10., 10.),
                    (12., 3.),
                ]),
                Icon::Paste => {
                    line(&[(5., 5.), (19., 5.), (19., 21.), (5., 21.), (5., 5.)]);
                    line(&[(9., 3.), (15., 3.), (15., 7.), (9., 7.), (9., 3.)]);
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
            Icon::Search => frame.stroke(&canvas::Path::circle(Point::new(11., 11.), 6.), stroke),
            Icon::More => {
                for x in [5., 12., 19.] {
                    frame.fill(&canvas::Path::circle(Point::new(x, 12.), 1.1), self.color);
                }
            }
            Icon::Settings => {
                for (x, y) in [(9., 7.), (15., 12.), (9., 17.)] {
                    frame.fill(&canvas::Path::circle(Point::new(x, y), 2.2), self.color);
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
