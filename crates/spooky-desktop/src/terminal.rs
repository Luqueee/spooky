use crate::Message;
use iced::{Color, Font, Point, Rectangle, Size, Theme, font, mouse, widget::canvas};
use spooky_core::Pane;

pub(crate) fn metrics(size: f32) -> (f32, f32) {
    (size * 0.602, size * 1.35)
}
#[cfg(target_os = "macos")]
const TERMINAL_FONT: Font = Font::with_name("Menlo");
#[cfg(not(target_os = "macos"))]
const TERMINAL_FONT: Font = Font::with_name("DejaVu Sans Mono");
#[derive(Default)]
pub(crate) struct State {
    pane: Option<u64>,
    bounds: Option<Rectangle>,
    anchor: Option<(u16, u16)>,
    dragging: bool,
}
pub(crate) struct Terminal<'a> {
    pub pane: &'a Pane,
    pub font_size: f32,
    pub focused: bool,
    pub selection: Option<((u16, u16), (u16, u16))>,
}
impl canvas::Program<Message> for Terminal<'_> {
    type State = State;
    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if state.pane != Some(self.pane.id) || state.bounds != Some(bounds) {
            state.pane = Some(self.pane.id);
            state.bounds = Some(bounds);
            return Some(canvas::Action::publish(Message::Bounds(
                self.pane.id,
                bounds,
            )));
        }
        let position = || {
            cursor.position_in(bounds).map(|p| {
                let (cw, ch) = metrics(self.font_size);
                (
                    ((p.y / ch).max(0.0) as u16).min(self.pane.rows.saturating_sub(1)),
                    ((p.x / cw).max(0.0) as u16).min(self.pane.cols.saturating_sub(1)),
                )
            })
        };
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if position().is_some() =>
            {
                let p = position()?;
                state.anchor = Some(p);
                state.dragging = true;
                Some(canvas::Action::publish(Message::FocusPane(self.pane.id)).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => Some(
                canvas::Action::publish(Message::Select(self.pane.id, state.anchor?, position()?))
                    .and_capture(),
            ),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                Some(canvas::Action::capture())
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.is_over(bounds) =>
            {
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / metrics(self.font_size).1,
                };
                Some(
                    canvas::Action::publish(Message::ScrollPane(self.pane.id, y.round() as i32))
                        .and_capture(),
                )
            }
            _ => None,
        }
    }
    fn draw(
        &self,
        _: &State,
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (cw, ch) = metrics(self.font_size);
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), crate::chrome::BACKGROUND);
        for c in &self.pane.cells {
            let pos = Point::new(f32::from(c.col) * cw, f32::from(c.row) * ch);
            let width = cw * if c.wide { 2.0 } else { 1.0 };
            let selected = self.selection.is_some_and(|(a, b)| {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                (c.row, c.col) >= lo && (c.row, c.col) <= hi
            });
            let bg = if selected {
                Color::from_rgb8(67, 75, 96)
            } else {
                Color::from_rgb8(c.bg[0], c.bg[1], c.bg[2])
            };
            frame.fill_rectangle(pos, Size::new(width, ch), bg);
            let fg = Color::from_rgb8(c.fg[0], c.fg[1], c.fg[2]);
            if !c.text.is_empty() && c.text != " " {
                frame.fill_text(canvas::Text {
                    content: c.text.clone(),
                    position: pos,
                    color: fg,
                    size: self.font_size.into(),
                    font: Font {
                        weight: if c.bold {
                            font::Weight::Bold
                        } else {
                            font::Weight::Normal
                        },
                        style: if c.italic {
                            font::Style::Italic
                        } else {
                            font::Style::Normal
                        },
                        ..TERMINAL_FONT
                    },
                    ..Default::default()
                });
            }
            if c.underline {
                frame.fill_rectangle(
                    Point::new(pos.x, pos.y + ch - 2.0),
                    Size::new(width, 1.0),
                    fg,
                );
            }
        }
        if self.pane.cursor.visible {
            let p = Point::new(
                f32::from(self.pane.cursor.col) * cw,
                f32::from(self.pane.cursor.row) * ch,
            );
            frame.fill_rectangle(
                p,
                Size::new(2.0, ch),
                if self.focused {
                    crate::chrome::ACCENT
                } else {
                    Color::from_rgb8(100, 110, 125)
                },
            );
        }
        vec![frame.into_geometry()]
    }
}
