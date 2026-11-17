//! A text field: single line, or several wrapped lines for the agent composer.
//!
//! The application owns the value. The field shows what it is given, applies edits locally so
//! that consecutive input events agree, and reports each change as a message. Text arrives
//! through the platform's input method (so accents, dead keys and composition work); keys
//! that move or delete are handled here.

use super::{Root, color::hsla, keys};
use crate::{Message, keys::Press, ui::ColorPalette};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    InteractiveElement, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, SharedString, Style,
    Styled, TextAlign, TextRun, UTF16Selection, UnderlineStyle, WeakEntity, Window, WrappedLine,
    div, fill, point, px, relative, size,
};
use smallvec::SmallVec;
use std::{ops::Range, rc::Rc};
use unicode_segmentation::UnicodeSegmentation;

/// Inspects a key before the field does; the message it returns takes the key.
pub(crate) type KeyHook = Rc<dyn Fn(&Press) -> Option<Message>>;

/// What the application wants the field to be.
#[derive(Clone)]
pub(crate) struct Spec {
    /// The text it holds.
    pub value: String,
    /// What it shows while empty.
    pub placeholder: &'static str,
    /// The font size in pixels.
    pub size: f32,
    /// The fewest and most rows it grows between; `(1, 1)` is a single line.
    pub rows: (usize, usize),
    /// Whether a plain Enter submits (Shift+Enter then inserts a line break) rather than
    /// inserting one. Single lines always submit.
    pub submit_on_enter: bool,
    /// The message an edit sends.
    pub on_input: Rc<dyn Fn(String) -> Message>,
    /// The message Enter sends.
    pub on_submit: Option<Message>,
    /// A key the application handles before the field does, and the message it sends.
    pub on_key: Option<KeyHook>,
    pub colors: ColorPalette,
}

impl Spec {
    fn multiline(&self) -> bool {
        self.rows.1 > 1
    }
}

/// The shaped text of the last paint, for hit testing and the input method.
struct Layout {
    lines: SmallVec<[WrappedLine; 1]>,
    /// Where each logical line starts in the text.
    starts: Vec<usize>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    /// How far the content is scrolled: horizontally for one line, vertically for several.
    scroll: Point<Pixels>,
}

impl Layout {
    /// The position of a byte offset, relative to the top left of the content.
    fn position(&self, offset: usize) -> Point<Pixels> {
        let mut top = Pixels::ZERO;
        for (line, start) in self.lines.iter().zip(&self.starts) {
            let end = start + line.len();
            if offset <= end {
                let inside = line
                    .position_for_index(offset - start, self.line_height)
                    .unwrap_or_default();
                return point(inside.x, top + inside.y);
            }
            top += line.size(self.line_height).height;
        }
        point(Pixels::ZERO, top)
    }

    /// The byte offset closest to a position relative to the top left of the content.
    fn offset(&self, position: Point<Pixels>) -> usize {
        let mut top = Pixels::ZERO;
        for (line, start) in self.lines.iter().zip(&self.starts) {
            let height = line.size(self.line_height).height;
            if position.y < top + height {
                let local = point(position.x, position.y - top);
                let inside = line
                    .closest_index_for_position(local, self.line_height)
                    .unwrap_or_else(|index| index);
                return start + inside;
            }
            top += height;
        }
        self.starts
            .last()
            .zip(self.lines.last())
            .map_or(0, |(start, line)| start + line.len())
    }

    /// The visual rows of the text: the first byte, the last byte and the top, for each.
    fn rows(&self) -> Vec<(usize, usize, Pixels)> {
        let mut rows = Vec::new();
        let mut top = Pixels::ZERO;
        for (line, start) in self.lines.iter().zip(&self.starts) {
            let mut row_start = 0;
            for boundary in line.wrap_boundaries() {
                let Some(glyph) = line
                    .runs()
                    .get(boundary.run_ix)
                    .and_then(|run| run.glyphs.get(boundary.glyph_ix))
                else {
                    continue;
                };
                rows.push((start + row_start, start + glyph.index, top));
                row_start = glyph.index;
                top += self.line_height;
            }
            rows.push((start + row_start, start + line.len(), top));
            top += self.line_height;
        }
        rows
    }
}

pub(crate) struct TextInput {
    root: WeakEntity<Root>,
    focus: FocusHandle,
    spec: Spec,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    layout: Option<Layout>,
    scroll: Point<Pixels>,
    selecting: bool,
}

impl TextInput {
    pub(crate) fn new(root: WeakEntity<Root>, spec: Spec, cx: &mut Context<Self>) -> Self {
        let end = spec.value.len();
        Self {
            root,
            focus: cx.focus_handle(),
            spec,
            selected: end..end,
            reversed: false,
            marked: None,
            layout: None,
            scroll: Point::default(),
            selecting: false,
        }
    }

    /// Takes what the application now wants the field to be.
    pub(crate) fn configure(&mut self, spec: Spec) {
        if spec.value != self.spec.value {
            let end = spec.value.len();
            self.selected = clamp(&self.selected, end);
            if self.selected.is_empty() && self.selected.start == self.spec.value.len() {
                // The caret was at the end: it stays there.
                self.selected = end..end;
            }
            self.marked = None;
        }
        self.spec = spec;
    }

    pub(crate) fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    pub(crate) fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected = 0..self.spec.value.len();
        self.reversed = false;
        cx.notify();
    }

    fn send(&self, message: Message, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        cx.defer_in(window, move |_, window, cx| {
            root.update(cx, |root, cx| root.dispatch(message, window, cx))
                .ok();
        });
    }

    /// Replaces the text and selection, and reports the change.
    fn commit(
        &mut self,
        text: String,
        selected: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = text != self.spec.value;
        self.selected = selected;
        self.reversed = false;
        if changed {
            self.spec.value = text.clone();
            let message = (self.spec.on_input)(text);
            self.send(message, window, cx);
        }
        cx.notify();
    }

    fn replace(
        &mut self,
        range: Range<usize>,
        with: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = clamp(&range, self.spec.value.len());
        let with = if self.spec.multiline() {
            with.replace("\r\n", "\n").replace('\r', "\n")
        } else {
            with.replace(['\r', '\n'], " ")
        };
        let mut text = String::with_capacity(self.spec.value.len() + with.len());
        text.push_str(&self.spec.value[..range.start]);
        text.push_str(&with);
        text.push_str(&self.spec.value[range.end..]);
        let caret = range.start + with.len();
        self.marked = None;
        self.commit(text, caret..caret, window, cx);
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    /// Moves the caret, extending the selection when asked.
    fn move_to(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        if extend {
            let anchor = if self.reversed {
                self.selected.end
            } else {
                self.selected.start
            };
            let anchor = if self.selected.is_empty() {
                self.selected.start
            } else {
                anchor
            };
            self.reversed = offset < anchor;
            self.selected = offset.min(anchor)..offset.max(anchor);
        } else {
            self.selected = offset..offset;
            self.reversed = false;
        }
        self.scroll_to_caret_next_paint();
        cx.notify();
    }

    fn scroll_to_caret_next_paint(&mut self) {
        // Painting keeps the caret in view; nothing to record beyond the redraw.
    }

    /// The offset a vertical move lands on, from the current caret.
    fn vertical(&self, rows: isize) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let at = layout.position(self.cursor());
        let target = point(
            at.x,
            (at.y + layout.line_height * (rows as f32 + 0.5)).max(Pixels::ZERO),
        );
        Some(layout.offset(target))
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let press = keys::press(&event.keystroke);
        if let Some(hook) = &self.spec.on_key
            && let Some(message) = hook(&press)
        {
            self.send(message, window, cx);
            cx.stop_propagation();
            return;
        }
        let modifiers = event.keystroke.modifiers;
        let shift = modifiers.shift;
        let command = if cfg!(target_os = "macos") {
            modifiers.platform
        } else {
            modifiers.control
        };
        let by_word = modifiers.alt || (!cfg!(target_os = "macos") && modifiers.control);
        let text = self.spec.value.clone();
        let cursor = self.cursor();
        let selection = self.selected.clone();
        let handled = match event.keystroke.key.as_str() {
            "left" => {
                let target = if command && cfg!(target_os = "macos") {
                    line_start(&text, cursor)
                } else if by_word {
                    word_start(&text, cursor)
                } else if !selection.is_empty() && !shift {
                    selection.start
                } else {
                    previous_boundary(&text, cursor)
                };
                self.move_to(target, shift, cx);
                true
            }
            "right" => {
                let target = if command && cfg!(target_os = "macos") {
                    line_end(&text, cursor)
                } else if by_word {
                    word_end(&text, cursor)
                } else if !selection.is_empty() && !shift {
                    selection.end
                } else {
                    next_boundary(&text, cursor)
                };
                self.move_to(target, shift, cx);
                true
            }
            "up" if self.spec.multiline() => {
                let target = self.vertical(-1).unwrap_or(0);
                self.move_to(target, shift, cx);
                true
            }
            "down" if self.spec.multiline() => {
                let target = self.vertical(1).unwrap_or(text.len());
                self.move_to(target, shift, cx);
                true
            }
            "up" => {
                self.move_to(0, shift, cx);
                true
            }
            "down" => {
                self.move_to(text.len(), shift, cx);
                true
            }
            "home" => {
                self.move_to(line_start(&text, cursor), shift, cx);
                true
            }
            "end" => {
                self.move_to(line_end(&text, cursor), shift, cx);
                true
            }
            "backspace" => {
                let range = if !selection.is_empty() {
                    selection
                } else if command && cfg!(target_os = "macos") {
                    line_start(&text, cursor)..cursor
                } else if by_word {
                    word_start(&text, cursor)..cursor
                } else {
                    previous_boundary(&text, cursor)..cursor
                };
                self.replace(range, "", window, cx);
                true
            }
            "delete" => {
                let range = if !selection.is_empty() {
                    selection
                } else if by_word {
                    cursor..word_end(&text, cursor)
                } else {
                    cursor..next_boundary(&text, cursor)
                };
                self.replace(range, "", window, cx);
                true
            }
            "enter" => {
                let submits = !self.spec.multiline() || (self.spec.submit_on_enter != shift);
                if submits {
                    if let Some(message) = self.spec.on_submit.clone() {
                        self.send(message, window, cx);
                    }
                } else {
                    self.replace(selection, "\n", window, cx);
                }
                true
            }
            "a" if command => {
                self.select_all(cx);
                true
            }
            "c" if command => {
                if !selection.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text[selection].to_owned()));
                }
                true
            }
            "x" if command => {
                if !selection.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(
                        text[selection.clone()].to_owned(),
                    ));
                    self.replace(selection, "", window, cx);
                }
                true
            }
            "v" if command => {
                if let Some(pasted) = cx.read_from_clipboard().and_then(|item| item.text()) {
                    self.replace(selection, &pasted, window, cx);
                }
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
        }
    }

    fn offset_at(&self, position: Point<Pixels>) -> usize {
        let Some(layout) = &self.layout else {
            return self.spec.value.len();
        };
        let local = position - layout.bounds.origin + layout.scroll;
        layout.offset(local)
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        let offset = self.offset_at(event.position);
        match event.click_count {
            2 => {
                let text = &self.spec.value;
                self.selected = word_start(text, offset)..word_end(text, offset);
                self.reversed = false;
            }
            3.. => self.selected = 0..self.spec.value.len(),
            _ => {
                self.selecting = true;
                self.move_to(offset, event.modifiers.shift, cx);
            }
        }
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.dragging() {
            let offset = self.offset_at(event.position);
            self.move_to(offset, true, cx);
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn utf16_offset(&self, offset: usize) -> usize {
        self.spec.value[..offset.min(self.spec.value.len())]
            .encode_utf16()
            .count()
    }

    fn utf8_offset(&self, utf16: usize) -> usize {
        let mut units = 0;
        for (index, character) in self.spec.value.char_indices() {
            if units >= utf16 {
                return index;
            }
            units += character.len_utf16();
        }
        self.spec.value.len()
    }

    fn utf16_range(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_offset(range.start)..self.utf16_offset(range.end)
    }

    fn utf8_range(&self, range: &Range<usize>) -> Range<usize> {
        self.utf8_offset(range.start)..self.utf8_offset(range.end)
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.utf8_range(&range);
        actual.replace(self.utf16_range(&range));
        self.spec.value.get(range).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.utf16_range(&self.selected),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.utf16_range(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.utf8_range(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        self.replace(range, text, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.utf8_range(&range))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selected.clone());
        let range = clamp(&range, self.spec.value.len());
        let mut value = String::with_capacity(self.spec.value.len() + text.len());
        value.push_str(&self.spec.value[..range.start]);
        value.push_str(text);
        value.push_str(&self.spec.value[range.end..]);
        let marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        let caret = selected
            .map(|selected| {
                let inside = text_offsets(text, &selected);
                range.start + inside.start..range.start + inside.end
            })
            .unwrap_or_else(|| range.start + text.len()..range.start + text.len());
        self.commit(value, caret, window, cx);
        self.marked = marked;
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.utf8_range(&range);
        let layout = self.layout.as_ref()?;
        let start = layout.position(range.start);
        let end = layout.position(range.end);
        Some(Bounds::from_corners(
            point(
                bounds.left() + start.x - layout.scroll.x,
                bounds.top() + start.y - layout.scroll.y,
            ),
            point(
                bounds.left() + end.x - layout.scroll.x,
                bounds.top() + end.y - layout.scroll.y + layout.line_height,
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_at(position);
        Some(self.utf16_offset(offset))
    }
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = self.spec.colors;
        let element = TextElement { input: cx.entity() };
        div()
            .id(ElementId::Name(SharedString::from("text-input")))
            .key_context("TextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .w_full()
            .text_size(px(self.spec.size))
            .text_color(hsla(colors.text))
            .child(element)
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct Prepaint {
    lines: SmallVec<[WrappedLine; 1]>,
    starts: Vec<usize>,
    line_height: Pixels,
    scroll: Point<Pixels>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl TextElement {
    /// Shapes the field's text for a width, one wrapped line per logical line.
    fn shape(
        input: &TextInput,
        width: Pixels,
        window: &mut Window,
    ) -> (SmallVec<[WrappedLine; 1]>, Vec<usize>, Pixels) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = font_size * 1.4;
        let empty = input.spec.value.is_empty();
        let (text, color) = if empty {
            (
                SharedString::from(input.spec.placeholder),
                hsla(input.spec.colors.muted),
            )
        } else {
            (SharedString::from(input.spec.value.clone()), style.color)
        };
        let base = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match input.marked.as_ref().filter(|_| !empty) {
            Some(marked) => [
                TextRun {
                    len: marked.start,
                    ..base.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..base
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect::<Vec<_>>(),
            None => vec![base],
        };
        let wrap = input.spec.multiline().then_some(width);
        let lines = window
            .text_system()
            .shape_text(text.clone(), font_size, &runs, wrap, None)
            .unwrap_or_default();
        let mut starts = Vec::with_capacity(lines.len());
        let mut at = 0;
        for line in &lines {
            starts.push(at);
            at += line.len() + 1;
        }
        (lines, starts, line_height)
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

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
        let input = self.input.clone();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, cx| {
            let input = input.read(cx);
            let (shaped, reported) = match (known.width, available.width) {
                (Some(width), _) | (None, gpui::AvailableSpace::Definite(width)) => (width, width),
                // With no width to fit, the field asks for none (so it never pushes its
                // neighbours aside) and is shaped unwrapped to measure its height.
                (None, gpui::AvailableSpace::MinContent) => (px(10_000.), Pixels::ZERO),
                (None, gpui::AvailableSpace::MaxContent) => (px(10_000.), px(160.)),
            };
            let (lines, _, line_height) = TextElement::shape(input, shaped, window);
            let rows: usize = lines
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum();
            let (least, most) = input.spec.rows;
            size(
                reported,
                line_height * rows.clamp(least.max(1), most.max(1)) as f32,
            )
        });
        let _ = cx;
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let (lines, starts, line_height) = Self::shape(input, bounds.size.width, window);
        let mut scroll = input.scroll;
        let probe = Layout {
            lines,
            starts,
            bounds,
            line_height,
            scroll,
        };
        let caret = probe.position(input.cursor());
        if input.spec.multiline() {
            let total = probe
                .lines
                .iter()
                .map(|line| line.size(line_height).height)
                .fold(Pixels::ZERO, |sum, height| sum + height);
            let overflow = (total - bounds.size.height).max(Pixels::ZERO);
            if caret.y < scroll.y {
                scroll.y = caret.y;
            } else if caret.y + line_height > scroll.y + bounds.size.height {
                scroll.y = caret.y + line_height - bounds.size.height;
            }
            scroll.y = scroll.y.clamp(Pixels::ZERO, overflow);
            scroll.x = Pixels::ZERO;
        } else {
            let slack = px(2.);
            if caret.x < scroll.x {
                scroll.x = caret.x;
            } else if caret.x + slack > scroll.x + bounds.size.width {
                scroll.x = caret.x + slack - bounds.size.width;
            }
            scroll.x = scroll.x.max(Pixels::ZERO);
            scroll.y = Pixels::ZERO;
        }
        let Layout { lines, starts, .. } = probe;
        Prepaint {
            lines,
            starts,
            line_height,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let colors = self.input.read(cx).spec.colors;
        let focused = focus.is_focused(window);
        let selected = self.input.read(cx).selected.clone();
        let cursor = self.input.read(cx).cursor();
        let empty = self.input.read(cx).spec.value.is_empty();
        let layout = Layout {
            lines: std::mem::take(&mut prepaint.lines),
            starts: std::mem::take(&mut prepaint.starts),
            bounds,
            line_height: prepaint.line_height,
            scroll: prepaint.scroll,
        };
        let origin = bounds.origin - layout.scroll;
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if focused && !selected.is_empty() {
                let selection = hsla(colors.accent.with_alpha(0.3));
                for (start, end, top) in layout.rows() {
                    let from = selected.start.max(start);
                    let to = selected.end.min(end);
                    if from >= to {
                        continue;
                    }
                    let left = layout.position(from).x;
                    let right = layout.position(to).x;
                    window.paint_quad(fill(
                        Bounds::new(
                            point(origin.x + left, origin.y + top),
                            size(right - left, layout.line_height),
                        ),
                        selection,
                    ));
                }
            }
            let mut top = Pixels::ZERO;
            for line in &layout.lines {
                line.paint(
                    point(origin.x, origin.y + top),
                    layout.line_height,
                    TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                )
                .ok();
                top += line.size(layout.line_height).height;
            }
            if focused && selected.is_empty() {
                let at = if empty {
                    Point::default()
                } else {
                    layout.position(cursor)
                };
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x + at.x, origin.y + at.y),
                        size(px(1.5), layout.line_height),
                    ),
                    hsla(colors.text),
                ));
            }
        });
        let scroll = layout.scroll;
        self.input.update(cx, |input, _| {
            input.scroll = scroll;
            input.layout = Some(layout);
        });
    }
}

fn clamp(range: &Range<usize>, len: usize) -> Range<usize> {
    range.start.min(len)..range.end.min(len)
}

/// The byte offsets in `text` of a UTF-16 range.
fn text_offsets(text: &str, range: &Range<usize>) -> Range<usize> {
    let offset = |utf16: usize| {
        let mut units = 0;
        for (index, character) in text.char_indices() {
            if units >= utf16 {
                return index;
            }
            units += character.len_utf16();
        }
        text.len()
    };
    offset(range.start)..offset(range.end)
}

fn previous_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .rev()
        .find_map(|(index, _)| (index < offset).then_some(index))
        .unwrap_or(0)
}

fn next_boundary(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .find_map(|(index, _)| (index > offset).then_some(index))
        .unwrap_or(text.len())
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |index| index + 1)
}

fn line_end(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

fn is_word(grapheme: &str) -> bool {
    grapheme
        .chars()
        .next()
        .is_some_and(|character| character.is_alphanumeric() || character == '_')
}

fn word_start(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut at = offset;
    let mut in_word = false;
    for (index, grapheme) in text[..offset].grapheme_indices(true).rev() {
        let word = is_word(grapheme);
        if in_word && !word {
            break;
        }
        in_word |= word;
        at = index;
    }
    at
}

fn word_end(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    let mut at = offset;
    let mut in_word = false;
    for (index, grapheme) in text[offset..].grapheme_indices(true) {
        let word = is_word(grapheme);
        if in_word && !word {
            break;
        }
        in_word |= word;
        at = offset + index + grapheme.len();
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundaries_follow_graphemes_not_bytes() {
        let text = "a👨‍👩‍👧e\u{301}b";
        let family = "a".len();
        let after_family = family + "👨‍👩‍👧".len();
        assert_eq!(next_boundary(text, 0), family);
        assert_eq!(next_boundary(text, family), after_family);
        assert_eq!(previous_boundary(text, after_family), family);
        // A letter and its combining accent are one step.
        let after_accent = after_family + "e\u{301}".len();
        assert_eq!(next_boundary(text, after_family), after_accent);
        assert_eq!(previous_boundary(text, after_accent), after_family);
    }

    #[test]
    fn words_stop_at_punctuation_and_whitespace() {
        let text = "foo bar_baz  qux";
        assert_eq!(word_end(text, 0), 3);
        assert_eq!(word_end(text, 3), 11);
        assert_eq!(word_start(text, 11), 4);
        assert_eq!(word_start(text, 4), 0);
        assert_eq!(word_start(text, 13), 4);
    }

    #[test]
    fn lines_are_separated_by_newlines() {
        let text = "one\ntwo\nthree";
        assert_eq!(line_start(text, 6), 4);
        assert_eq!(line_end(text, 5), 7);
        assert_eq!(line_start(text, 2), 0);
        assert_eq!(line_end(text, 9), text.len());
    }

    #[test]
    fn utf16_offsets_map_to_byte_offsets() {
        // 😀 is two UTF-16 units and four bytes.
        let text = "a😀b";
        assert_eq!(text_offsets(text, &(1..3)), 1..5);
        assert_eq!(text_offsets(text, &(3..4)), 5..6);
    }
}
