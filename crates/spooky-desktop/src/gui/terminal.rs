//! A terminal pane: the viewport the server reports, drawn cell by cell, and the pointer
//! behavior that goes with it (selection, links, mouse reporting, the wheel).

use super::{Sender, color::hsla, geometry_point, keys};
use crate::{
    ContextTarget, Message,
    geometry::{Point as AppPoint, Rectangle as AppRectangle, Size as AppSize},
    keys::Modifiers,
    terminal::{Action, Button, Link, TerminalFont, link_at, metrics, report, wheel_arrows},
    ui::{Color, ColorPalette},
};
use gpui::{
    App, Bounds, CursorStyle, DispatchPhase, Element, ElementId, ElementInputHandler, Entity,
    FocusHandle, Font, FontFeatures, FontStyle, FontWeight, GlobalElementId, Hitbox,
    HitboxBehavior, Hsla, InspectorElementId, IntoElement, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollDelta, ScrollWheelEvent, SharedString, Style,
    Styled, TextAlign, TextRun, UnderlineStyle, Window, canvas, fill, point, px, relative, size,
};
use spooky_core::{CursorShape, MouseTracking, Pane, Snapshot};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Wheel notches forwarded per event, so momentum scrolling cannot flood a pane.
const MAX_WHEEL_EVENTS: u32 = 30;
/// How much of the theme's accent colour a selection lays over the text's background.
const SELECTION_ALPHA: f32 = 0.4;

/// What a pane's pointer handling remembers between events.
#[derive(Default)]
pub(crate) struct Pointer {
    bounds: Option<AppRectangle>,
    anchor: Option<(u16, u16)>,
    dragging: bool,
    /// Button whose press went to the application, until its release does.
    held: Option<Button>,
    /// Last (row, column) reported, so motion within one cell stays silent.
    last_cell: Option<(u16, u16)>,
    /// The link under the pointer while the link modifier is held.
    hover: Option<Link>,
}

pub(crate) type SharedPointer = Rc<RefCell<Pointer>>;

/// What lets a pane take text from the input method.
pub(crate) struct Typing {
    pub root: Entity<super::Root>,
    pub focus: FocusHandle,
    pub cursor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// One terminal pane.
pub(crate) struct TerminalView {
    pub snapshot: Rc<Snapshot>,
    pub pane: u64,
    pub font: TerminalFont,
    pub font_size: f32,
    pub focused: bool,
    pub selection: Option<((u16, u16), (u16, u16))>,
    /// Whether a blinking cursor is in its visible phase.
    pub cursor_on: bool,
    pub colors: ColorPalette,
    pub pointer: SharedPointer,
    pub send: Sender,
    /// Text the input method is composing, shown at the cursor.
    pub composing: Option<String>,
    /// Set while this pane has the keyboard.
    pub typing: Option<Typing>,
}

/// What the pointer handlers share for one frame.
struct Frame {
    snapshot: Rc<Snapshot>,
    pane: u64,
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    cell: (f32, f32),
    pointer: SharedPointer,
    send: Sender,
}

impl Frame {
    fn pane(&self) -> Option<&Pane> {
        self.snapshot.panes.iter().find(|pane| pane.id == self.pane)
    }

    /// The (row, column) at a window position. With `clamp`, a position outside the pane
    /// maps to its nearest edge cell, as a drag that left the pane needs.
    fn cell_at(
        &self,
        pane: &Pane,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<(u16, u16)> {
        if !clamp && !self.bounds.contains(&position) {
            return None;
        }
        let x = f32::from(position.x - self.bounds.origin.x);
        let y = f32::from(position.y - self.bounds.origin.y);
        let (width, height) = self.cell;
        Some((
            ((y / height).max(0.0) as u16).min(pane.rows.saturating_sub(1)),
            ((x / width).max(0.0) as u16).min(pane.cols.saturating_sub(1)),
        ))
    }

    fn send(&self, message: Message, window: &mut Window, cx: &mut App) {
        (self.send)(message, window, cx);
    }
}

fn paint_color(color: [u8; 3]) -> Hsla {
    hsla(Color::from_rgb8(color[0], color[1], color[2]))
}

impl IntoElement for TerminalView {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TerminalView {
    fn font_for(&self, bold: bool, italic: bool) -> Font {
        Font {
            family: SharedString::from(self.font.family.to_string()),
            features: FontFeatures::default(),
            fallbacks: None,
            weight: if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        }
    }

    fn paint_pane(&self, pane: &Pane, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let (cw, ch) = metrics(self.font_size, self.font.advance);
        let origin = bounds.origin;
        let rect = |window: &mut Window, x: f32, y: f32, width: f32, height: f32, color: Hsla| {
            window.paint_quad(fill(
                Bounds::new(
                    point(origin.x + px(x), origin.y + px(y)),
                    size(px(width), px(height)),
                ),
                color,
            ));
        };
        let selection = self
            .selection
            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let last_col = pane.cols.saturating_sub(1);
        for line in &pane.lines {
            let y = f32::from(line.row) * ch;
            for run in &line.runs {
                if run.explicit_bg || run.bg != spooky_core::DEFAULT_BACKGROUND {
                    rect(
                        window,
                        f32::from(run.col) * cw,
                        y,
                        f32::from(run.columns()) * cw,
                        ch,
                        paint_color(run.bg),
                    );
                }
            }
            if let Some((lo, hi)) = selection
                && (lo.0..=hi.0).contains(&line.row)
            {
                let start = if line.row == lo.0 { lo.1 } else { 0 };
                let end = if line.row == hi.0 { hi.1 } else { last_col };
                rect(
                    window,
                    f32::from(start) * cw,
                    y,
                    f32::from(end.saturating_sub(start) + 1) * cw,
                    ch,
                    hsla(self.colors.accent.scale_alpha(SELECTION_ALPHA)),
                );
            }
            for found in pane.highlights.iter().filter(|found| found.row == line.row) {
                rect(
                    window,
                    f32::from(found.start) * cw,
                    y,
                    f32::from(found.end.saturating_sub(found.start) + 1) * cw,
                    ch,
                    hsla(
                        self.colors
                            .warning
                            .scale_alpha(if found.current { 0.65 } else { 0.3 }),
                    ),
                );
            }
            for run in &line.runs {
                let fg = paint_color(run.fg);
                if !run.text.chars().all(|character| character == ' ') {
                    let text_run = TextRun {
                        len: run.text.len(),
                        font: self.font_for(run.bold, run.italic),
                        color: fg,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    // One glyph per cell, however wide the font draws it. A cell that is
                    // wide or has combining marks is a run of its own.
                    let force = (run.width == 0).then_some(px(cw));
                    let shaped = window.text_system().shape_line(
                        SharedString::from(run.text.clone()),
                        px(self.font_size),
                        &[text_run],
                        force,
                    );
                    shaped
                        .paint(
                            point(origin.x + px(f32::from(run.col) * cw), origin.y + px(y)),
                            px(ch),
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                }
                let x = f32::from(run.col) * cw;
                let width = f32::from(run.columns()) * cw;
                if run.underline {
                    rect(window, x, y + ch - 2.0, width, 1.0, fg);
                }
                if run.strikethrough {
                    rect(window, x, y + ch * 0.5, width, 1.0, fg);
                }
            }
        }
        if let Some(link) = &self.pointer.borrow().hover {
            rect(
                window,
                f32::from(link.start) * cw,
                f32::from(link.row) * ch + ch - 2.0,
                f32::from(link.end.saturating_sub(link.start) + 1) * cw,
                1.0,
                hsla(self.colors.accent),
            );
        }
        if pane.cursor.visible && self.cursor_on {
            let x = f32::from(pane.cursor.col) * cw;
            let y = f32::from(pane.cursor.row) * ch;
            let color = hsla(if self.focused {
                self.colors.accent
            } else {
                self.colors.muted
            });
            match (pane.cursor.shape, self.focused) {
                // A translucent block keeps the glyph under it readable.
                (CursorShape::Block, true) => rect(
                    window,
                    x,
                    y,
                    cw,
                    ch,
                    hsla(if self.focused {
                        self.colors.accent.scale_alpha(0.6)
                    } else {
                        self.colors.muted.scale_alpha(0.6)
                    }),
                ),
                // An unfocused block is an outline, so it cannot pass for the live cursor.
                (CursorShape::Block, false) => {
                    for (dx, dy, width, height) in [
                        (0.0, 0.0, cw, 1.0),
                        (0.0, ch - 1.0, cw, 1.0),
                        (0.0, 0.0, 1.0, ch),
                        (cw - 1.0, 0.0, 1.0, ch),
                    ] {
                        rect(window, x + dx, y + dy, width, height, color);
                    }
                }
                (CursorShape::Underline, _) => rect(window, x, y + ch - 2.0, cw, 2.0, color),
                (CursorShape::Beam, _) => rect(window, x, y, 2.0, ch, color),
            }
        }
        if let Some(typing) = &self.typing {
            let cursor = point(
                origin.x + px(f32::from(pane.cursor.col) * cw),
                origin.y + px(f32::from(pane.cursor.row) * ch),
            );
            typing
                .cursor_bounds
                .set(Some(Bounds::new(cursor, size(px(cw), px(ch)))));
            if let Some(text) = &self.composing {
                let color = hsla(self.colors.text);
                let run = TextRun {
                    len: text.len(),
                    font: self.font_for(false, false),
                    color,
                    background_color: Some(hsla(self.colors.selected)),
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    SharedString::from(text.clone()),
                    px(self.font_size),
                    &[run],
                    Some(px(cw)),
                );
                shaped
                    .paint(cursor, px(ch), TextAlign::Left, None, window, cx)
                    .ok();
            }
        }
    }

    /// Registers the pointer handlers for this frame.
    fn handle_pointer(&self, frame: Rc<Frame>, window: &mut Window) {
        let down = frame.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !down.hitbox.is_hovered(window) {
                return;
            }
            let Some(pane) = down.pane() else { return };
            let modifiers = keys::modifiers(&event.modifiers);
            let Some((row, col)) = down.cell_at(pane, event.position, false) else {
                return;
            };
            let id = pane.id;
            if event.button == MouseButton::Left
                && modifiers.command()
                && let Some(link) = link_at(pane, row, col)
            {
                down.send(Message::OpenLink(link.target), window, cx);
                cx.stop_propagation();
                return;
            }
            // Holding Shift keeps the mouse for selection and scrollback.
            let reporting =
                pane.mouse_tracking != MouseTracking::Off && !pane.exited && !modifiers.shift;
            if reporting && let Some(button) = Button::from_gui(event.button) {
                let bytes = report(
                    pane.mouse_tracking,
                    pane.mouse_encoding,
                    Action::Press,
                    Some(button),
                    modifiers,
                    col,
                    row,
                )
                .unwrap_or_default();
                {
                    let mut state = down.pointer.borrow_mut();
                    state.held = Some(button);
                    state.last_cell = Some((row, col));
                }
                down.send(Message::FocusPane(id, bytes), window, cx);
                cx.stop_propagation();
            } else if event.button == MouseButton::Left {
                {
                    let mut state = down.pointer.borrow_mut();
                    state.anchor = Some((row, col));
                    state.dragging = true;
                }
                down.send(Message::FocusPane(id, Vec::new()), window, cx);
                cx.stop_propagation();
            } else if event.button == MouseButton::Right {
                // Right-click opens the pane menu, unless the program is tracking the
                // mouse (Shift takes the click back).
                down.send(
                    Message::ContextMenu(ContextTarget::Pane(id), geometry_point(event.position)),
                    window,
                    cx,
                );
                cx.stop_propagation();
            }
        });
        let moved = frame.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let Some(pane) = moved.pane() else { return };
            let modifiers = keys::modifiers(&event.modifiers);
            let (dragging, held) = {
                let state = moved.pointer.borrow();
                (state.dragging, state.held)
            };
            // The link modifier makes the link under the pointer clickable.
            if !dragging && held.is_none() {
                let link = if modifiers.command() && moved.hitbox.is_hovered(window) {
                    moved
                        .cell_at(pane, event.position, false)
                        .and_then(|(row, col)| link_at(pane, row, col))
                } else {
                    None
                };
                let changed = {
                    let mut state = moved.pointer.borrow_mut();
                    (link != state.hover).then(|| {
                        let target = link.as_ref().map(|link| link.target.clone());
                        state.hover = link;
                        target
                    })
                };
                if let Some(target) = changed {
                    moved.send(Message::LinkHover(target), window, cx);
                }
            }
            if dragging {
                let anchor = moved.pointer.borrow().anchor;
                if let (Some(anchor), Some(head)) =
                    (anchor, moved.cell_at(pane, event.position, true))
                {
                    moved.send(Message::Select(pane.id, anchor, head), window, cx);
                }
                return;
            }
            let reporting =
                pane.mouse_tracking != MouseTracking::Off && !pane.exited && !modifiers.shift;
            if !reporting || !(held.is_some() || moved.hitbox.is_hovered(window)) {
                return;
            }
            let Some((row, col)) = moved.cell_at(pane, event.position, held.is_some()) else {
                return;
            };
            {
                let mut state = moved.pointer.borrow_mut();
                if state.last_cell == Some((row, col)) {
                    return;
                }
                state.last_cell = Some((row, col));
            }
            if let Some(bytes) = report(
                pane.mouse_tracking,
                pane.mouse_encoding,
                Action::Motion,
                held,
                modifiers,
                col,
                row,
            ) {
                moved.send(Message::PaneInput(pane.id, bytes), window, cx);
            }
        });
        let up = frame.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let Some(pane) = up.pane() else { return };
            let released = Button::from_gui(event.button);
            let (held, last) = {
                let state = up.pointer.borrow();
                (state.held, state.last_cell)
            };
            if let Some(held) = held
                && released == Some(held)
            {
                up.pointer.borrow_mut().held = None;
                let modifiers = keys::modifiers(&event.modifiers);
                if let Some((row, col)) = up.cell_at(pane, event.position, true).or(last)
                    && let Some(bytes) = report(
                        pane.mouse_tracking,
                        pane.mouse_encoding,
                        Action::Release,
                        Some(held),
                        modifiers,
                        col,
                        row,
                    )
                {
                    up.send(Message::PaneInput(pane.id, bytes), window, cx);
                }
                cx.stop_propagation();
                return;
            }
            if event.button == MouseButton::Left && up.pointer.borrow().dragging {
                up.pointer.borrow_mut().dragging = false;
                // The selection is final now: the application may copy it.
                up.send(Message::SelectionEnd(pane.id), window, cx);
                cx.stop_propagation();
            }
        });
        let wheel = frame;
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble || !wheel.hitbox.is_hovered(window) {
                return;
            }
            let Some(pane) = wheel.pane() else { return };
            let modifiers: Modifiers = keys::modifiers(&event.modifiers);
            let lines = match event.delta {
                ScrollDelta::Lines(delta) => delta.y * 3.0,
                ScrollDelta::Pixels(delta) => f32::from(delta.y) / wheel.cell.1,
            }
            .round() as i32;
            cx.stop_propagation();
            if lines == 0 {
                return;
            }
            let up = lines > 0;
            let notches = lines.unsigned_abs().min(MAX_WHEEL_EVENTS);
            let id = pane.id;
            let reporting =
                pane.mouse_tracking != MouseTracking::Off && !pane.exited && !modifiers.shift;
            if reporting {
                let Some((row, col)) = wheel.cell_at(pane, event.position, false) else {
                    return;
                };
                let button = if up {
                    Button::WheelUp
                } else {
                    Button::WheelDown
                };
                if let Some(one) = report(
                    pane.mouse_tracking,
                    pane.mouse_encoding,
                    Action::Press,
                    Some(button),
                    modifiers,
                    col,
                    row,
                ) {
                    wheel.send(
                        Message::PaneInput(id, one.repeat(notches as usize)),
                        window,
                        cx,
                    );
                }
                return;
            }
            if pane.alternate_scroll && !pane.exited {
                let bytes = wheel_arrows(up, pane.application_cursor, notches);
                wheel.send(Message::PaneInput(id, bytes), window, cx);
                return;
            }
            wheel.send(Message::ScrollPane(id, lines), window, cx);
        });
    }
}

impl Element for TerminalView {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(pane) = self.snapshot.panes.iter().find(|pane| pane.id == self.pane) else {
            return;
        };
        // The application sizes the grid from where the pane is.
        let area = AppRectangle::new(
            AppPoint::new(f32::from(bounds.origin.x), f32::from(bounds.origin.y)),
            AppSize::new(f32::from(bounds.size.width), f32::from(bounds.size.height)),
        );
        let changed = {
            let mut state = self.pointer.borrow_mut();
            (state.bounds != Some(area)).then(|| {
                state.bounds = Some(area);
            })
        };
        if changed.is_some() {
            let send = self.send.clone();
            let id = self.pane;
            window.defer(cx, move |window, cx| {
                send(Message::Bounds(id, area), window, cx)
            });
        }
        let frame = Rc::new(Frame {
            snapshot: self.snapshot.clone(),
            pane: self.pane,
            bounds,
            hitbox: hitbox.clone(),
            cell: metrics(self.font_size, self.font.advance),
            pointer: self.pointer.clone(),
            send: self.send.clone(),
        });
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            self.paint_pane(pane, bounds, window, cx);
        });
        let style = if self.pointer.borrow().hover.is_some() {
            CursorStyle::PointingHand
        } else if pane.mouse_tracking != MouseTracking::Off {
            CursorStyle::Arrow
        } else {
            CursorStyle::IBeam
        };
        window.set_cursor_style(style, hitbox);
        self.handle_pointer(frame, window);
        if let Some(typing) = &self.typing {
            window.handle_input(
                &typing.focus,
                ElementInputHandler::new(bounds, typing.root.clone()),
                cx,
            );
        }
    }
}

/// The scrollback thumb, drawn at a pane's outer edge while it is scrolled back.
pub(crate) fn scroll_thumb(snapshot: Rc<Snapshot>, pane: u64, color: Color) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let Some(pane) = snapshot.panes.iter().find(|candidate| candidate.id == pane) else {
                return;
            };
            if pane.scrolled == 0 || pane.history == 0 {
                return;
            }
            let height = f32::from(bounds.size.height);
            let history = pane.history as f32;
            let rows = f32::from(pane.rows);
            let thumb = (height * rows / (rows + history)).clamp(0.0, height);
            let thumb = thumb.max(16.0_f32.min(height));
            let from_top = 1.0 - pane.scrolled as f32 / history;
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        bounds.origin.x + bounds.size.width - px(2.0),
                        bounds.origin.y + px((height - thumb) * from_top),
                    ),
                    size(px(2.0), px(thumb)),
                ),
                hsla(color.scale_alpha(0.55)),
            ));
        },
    )
    .size_full()
}
