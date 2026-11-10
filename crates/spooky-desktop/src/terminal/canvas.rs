use super::links;
use super::pointer::{self, Action, Button};
use crate::{ContextTarget, Message};
use iced::{Color, Font, Point, Rectangle, Size, Theme, font, keyboard, mouse, widget::canvas};
use spooky_core::{CursorShape, MouseTracking, Pane};

/// Wheel notches forwarded per event, so momentum scrolling cannot flood a pane.
const MAX_WHEEL_EVENTS: u32 = 30;

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
    modifiers: keyboard::Modifiers,
    /// Button whose press went to the application, until its release does.
    held: Option<Button>,
    /// Last (row, column) reported, so motion within one cell stays silent.
    last_cell: Option<(u16, u16)>,
    /// The link under the pointer while the link modifier is held.
    hover: Option<links::Link>,
}
pub(crate) struct Terminal<'a> {
    pub pane: &'a Pane,
    pub font_size: f32,
    pub focused: bool,
    pub selection: Option<((u16, u16), (u16, u16))>,
    /// Whether a blinking cursor is in its visible phase.
    pub cursor_on: bool,
}
impl Terminal<'_> {
    /// The (row, column) under the cursor. With `clamp`, a cursor outside the
    /// pane maps to its nearest edge cell, as a drag that left the pane needs.
    fn cell(&self, cursor: mouse::Cursor, bounds: Rectangle, clamp: bool) -> Option<(u16, u16)> {
        let (x, y) = if clamp {
            let p = cursor.position()?;
            (p.x - bounds.x, p.y - bounds.y)
        } else {
            let p = cursor.position_in(bounds)?;
            (p.x, p.y)
        };
        let (cw, ch) = metrics(self.font_size);
        Some((
            ((y / ch).max(0.0) as u16).min(self.pane.rows.saturating_sub(1)),
            ((x / cw).max(0.0) as u16).min(self.pane.cols.saturating_sub(1)),
        ))
    }
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
        // Mouse events carry no modifiers; follow them from the keyboard.
        if let canvas::Event::Keyboard(event) = event {
            match event {
                keyboard::Event::ModifiersChanged(modifiers) => state.modifiers = *modifiers,
                keyboard::Event::KeyPressed { modifiers, .. }
                | keyboard::Event::KeyReleased { modifiers, .. } => state.modifiers = *modifiers,
            }
        }
        if state.pane != Some(self.pane.id) || state.bounds != Some(bounds) {
            state.pane = Some(self.pane.id);
            state.bounds = Some(bounds);
            return Some(canvas::Action::publish(Message::Bounds(
                self.pane.id,
                bounds,
            )));
        }
        // The link modifier (Cmd, or Ctrl elsewhere) makes the link under the
        // pointer clickable. It only reports the link; the event is otherwise
        // handled as usual once the hover state has settled.
        let hover_event = matches!(
            event,
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. })
                | canvas::Event::Keyboard(
                    keyboard::Event::ModifiersChanged(_)
                        | keyboard::Event::KeyPressed { .. }
                        | keyboard::Event::KeyReleased { .. }
                )
        );
        if hover_event && !state.dragging && state.held.is_none() {
            let link = if state.modifiers.command() {
                self.cell(cursor, bounds, false)
                    .and_then(|(row, col)| links::link_at(self.pane, row, col))
            } else {
                None
            };
            if link != state.hover {
                let target = link.as_ref().map(|link| link.target.clone());
                state.hover = link;
                return Some(canvas::Action::publish(Message::LinkHover(target)));
            }
        }
        let id = self.pane.id;
        // Holding Shift keeps the mouse for selection and scrollback.
        let reporting = self.pane.mouse_tracking != MouseTracking::Off
            && !self.pane.exited
            && !state.modifiers.shift();
        let report = |action, button, modifiers, row, col| {
            pointer::report(
                self.pane.mouse_tracking,
                self.pane.mouse_encoding,
                action,
                button,
                modifiers,
                col,
                row,
            )
        };
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(pressed))
                if cursor.is_over(bounds) =>
            {
                let (row, col) = self.cell(cursor, bounds, false)?;
                if *pressed == mouse::Button::Left
                    && state.modifiers.command()
                    && let Some(link) = links::link_at(self.pane, row, col)
                {
                    return Some(
                        canvas::Action::publish(Message::OpenLink(link.target)).and_capture(),
                    );
                }
                if reporting && let Some(button) = Button::from_mouse(*pressed) {
                    state.held = Some(button);
                    state.last_cell = Some((row, col));
                    let bytes = report(Action::Press, Some(button), state.modifiers, row, col)
                        .unwrap_or_default();
                    return Some(
                        canvas::Action::publish(Message::FocusPane(id, bytes)).and_capture(),
                    );
                }
                if *pressed == mouse::Button::Left {
                    state.anchor = Some((row, col));
                    state.dragging = true;
                    return Some(
                        canvas::Action::publish(Message::FocusPane(id, Vec::new())).and_capture(),
                    );
                }
                // Right-click opens the pane menu, unless the program is
                // tracking the mouse (Shift takes the click back).
                if *pressed == mouse::Button::Right {
                    return Some(
                        canvas::Action::publish(Message::ContextMenu(
                            ContextTarget::Pane(id),
                            cursor.position()?,
                        ))
                        .and_capture(),
                    );
                }
                None
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.dragging {
                    let head = self.cell(cursor, bounds, false)?;
                    return Some(
                        canvas::Action::publish(Message::Select(id, state.anchor?, head))
                            .and_capture(),
                    );
                }
                if !reporting {
                    return None;
                }
                let (row, col) = self.cell(cursor, bounds, state.held.is_some())?;
                if state.last_cell == Some((row, col)) {
                    return None;
                }
                state.last_cell = Some((row, col));
                let bytes = report(Action::Motion, state.held, state.modifiers, row, col)?;
                Some(canvas::Action::publish(Message::PaneInput(id, bytes)))
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(released)) => {
                if let Some(held) = state.held
                    && Button::from_mouse(*released) == Some(held)
                {
                    state.held = None;
                    let (row, col) = self.cell(cursor, bounds, true).or(state.last_cell)?;
                    let bytes = report(Action::Release, Some(held), state.modifiers, row, col)?;
                    return Some(
                        canvas::Action::publish(Message::PaneInput(id, bytes)).and_capture(),
                    );
                }
                if *released == mouse::Button::Left && state.dragging {
                    state.dragging = false;
                    return Some(canvas::Action::capture());
                }
                None
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.is_over(bounds) =>
            {
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => *y / metrics(self.font_size).1,
                }
                .round() as i32;
                if lines == 0 {
                    return Some(canvas::Action::capture());
                }
                let up = lines > 0;
                let notches = lines.unsigned_abs().min(MAX_WHEEL_EVENTS);
                let input = |bytes| {
                    Some(canvas::Action::publish(Message::PaneInput(id, bytes)).and_capture())
                };
                if reporting {
                    let (row, col) = self.cell(cursor, bounds, false)?;
                    let button = if up {
                        Button::WheelUp
                    } else {
                        Button::WheelDown
                    };
                    return match report(Action::Press, Some(button), state.modifiers, row, col) {
                        Some(one) => input(one.repeat(notches as usize)),
                        None => Some(canvas::Action::capture()),
                    };
                }
                if self.pane.alternate_scroll && !self.pane.exited {
                    return input(pointer::wheel_arrows(
                        up,
                        self.pane.application_cursor,
                        notches,
                    ));
                }
                Some(canvas::Action::publish(Message::ScrollPane(id, lines)).and_capture())
            }
            _ => None,
        }
    }
    fn draw(
        &self,
        state: &State,
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (cw, ch) = metrics(self.font_size);
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), crate::ui::BACKGROUND);
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
            for found in self
                .pane
                .highlights
                .iter()
                .filter(|found| found.row == line.row)
            {
                frame.fill_rectangle(
                    Point::new(f32::from(found.start) * cw, y),
                    Size::new(
                        f32::from(found.end.saturating_sub(found.start) + 1) * cw,
                        ch,
                    ),
                    Color::from_rgb8(224, 175, 104).scale_alpha(if found.current {
                        0.65
                    } else {
                        0.3
                    }),
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
                let x = f32::from(run.col) * cw;
                let width = f32::from(run.columns()) * cw;
                if run.underline {
                    frame.fill_rectangle(Point::new(x, y + ch - 2.0), Size::new(width, 1.0), fg);
                }
                if run.strikethrough {
                    frame.fill_rectangle(Point::new(x, y + ch * 0.5), Size::new(width, 1.0), fg);
                }
            }
        }
        if let Some(link) = &state.hover {
            frame.fill_rectangle(
                Point::new(
                    f32::from(link.start) * cw,
                    f32::from(link.row) * ch + ch - 2.0,
                ),
                Size::new(f32::from(link.end.saturating_sub(link.start) + 1) * cw, 1.0),
                crate::ui::ACCENT,
            );
        }
        if self.pane.cursor.visible && self.cursor_on {
            let p = Point::new(
                f32::from(self.pane.cursor.col) * cw,
                f32::from(self.pane.cursor.row) * ch,
            );
            let color = if self.focused {
                crate::ui::ACCENT
            } else {
                Color::from_rgb8(100, 110, 125)
            };
            match (self.pane.cursor.shape, self.focused) {
                // A translucent block keeps the glyph under it readable.
                (CursorShape::Block, true) => {
                    frame.fill_rectangle(p, Size::new(cw, ch), color.scale_alpha(0.6));
                }
                // An unfocused block is an outline, so it cannot pass for the live cursor.
                (CursorShape::Block, false) => {
                    for (dx, dy, width, height) in [
                        (0.0, 0.0, cw, 1.0),
                        (0.0, ch - 1.0, cw, 1.0),
                        (0.0, 0.0, 1.0, ch),
                        (cw - 1.0, 0.0, 1.0, ch),
                    ] {
                        frame.fill_rectangle(
                            Point::new(p.x + dx, p.y + dy),
                            Size::new(width, height),
                            color,
                        );
                    }
                }
                (CursorShape::Underline, _) => {
                    frame.fill_rectangle(
                        Point::new(p.x, p.y + ch - 2.0),
                        Size::new(cw, 2.0),
                        color,
                    );
                }
                (CursorShape::Beam, _) => {
                    frame.fill_rectangle(p, Size::new(2.0, ch), color);
                }
            }
        }
        if self.pane.scrolled > 0 && self.pane.history > 0 {
            let history = self.pane.history as f32;
            let track = bounds.height;
            let thumb = (track * f32::from(self.pane.rows) / (f32::from(self.pane.rows) + history))
                .clamp(16.0, track);
            let from_top = 1.0 - self.pane.scrolled as f32 / history;
            frame.fill_rectangle(
                Point::new(bounds.width - 4.0, (track - thumb) * from_top),
                Size::new(3.0, thumb),
                crate::ui::MUTED.scale_alpha(0.7),
            );
            let label = format!("↑ {} of {} lines", self.pane.scrolled, self.pane.history);
            let width = label.chars().count() as f32 * 6.2 + 14.0;
            frame.fill_rectangle(
                Point::new(bounds.width - width - 12.0, 4.0),
                Size::new(width, 18.0),
                crate::ui::SIDEBAR.scale_alpha(0.9),
            );
            frame.fill_text(canvas::Text {
                content: label,
                position: Point::new(bounds.width - width - 5.0, 8.0),
                color: crate::ui::TEXT,
                size: 10.0.into(),
                font: TERMINAL_FONT,
                ..Default::default()
            });
        }
        vec![frame.into_geometry()]
    }
    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            mouse::Interaction::None
        } else if state.hover.is_some() {
            mouse::Interaction::Pointer
        } else if self.pane.mouse_tracking != MouseTracking::Off && !state.modifiers.shift() {
            mouse::Interaction::None
        } else {
            mouse::Interaction::Text
        }
    }
}
