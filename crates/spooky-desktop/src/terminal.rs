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
        let selection = self
            .selection
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let last_col = self.pane.cols.saturating_sub(1);
        for line in &self.pane.lines {
            let y = f32::from(line.row) * ch;
            for run in &line.runs {
                if run.bg != spooky_core::DEFAULT_BACKGROUND {
                    frame.fill_rectangle(
                        Point::new(f32::from(run.col) * cw, y),
                        Size::new(f32::from(run.columns()) * cw, ch),
                        Color::from_rgb8(run.bg[0], run.bg[1], run.bg[2]),
                    );
                }
            }
            if let Some((lo, hi)) = selection
                && (lo.0..=hi.0).contains(&line.row)
            {
                let start = if line.row == lo.0 { lo.1 } else { 0 };
                let end = if line.row == hi.0 { hi.1 } else { last_col };
                frame.fill_rectangle(
                    Point::new(f32::from(start) * cw, y),
                    Size::new(f32::from(end.saturating_sub(start) + 1) * cw, ch),
                    Color::from_rgb8(67, 75, 96),
                );
            }
            for run in &line.runs {
                let fg = Color::from_rgb8(run.fg[0], run.fg[1], run.fg[2]);
                let font = Font {
                    weight: if run.bold {
                        font::Weight::Bold
                    } else {
                        font::Weight::Normal
                    },
                    style: if run.italic {
                        font::Style::Italic
                    } else {
                        font::Style::Normal
                    },
                    ..TERMINAL_FONT
                };
                for (col, text, _) in run.cells() {
                    if text != " " {
                        frame.fill_text(canvas::Text {
                            content: text.to_owned(),
                            position: Point::new(f32::from(col) * cw, y),
                            color: fg,
                            size: self.font_size.into(),
                            font,
                            ..Default::default()
                        });
                    }
                }
                if run.underline {
                    frame.fill_rectangle(
                        Point::new(f32::from(run.col) * cw, y + ch - 2.0),
                        Size::new(f32::from(run.columns()) * cw, 1.0),
                        fg,
                    );
                }
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
