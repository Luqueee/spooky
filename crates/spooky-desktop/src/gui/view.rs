//! The vocabulary the views are written in: text in a tone, quiet and primary buttons, menus,
//! modals and text fields, each sending the application's messages.

use super::{
    Root, Ui,
    color::hsla,
    icons::{Icon, icon},
    text_input::{Spec, TextInput},
};
use crate::{Message, geometry, ui::ColorPalette};
use gpui::{
    AnyElement, AnyView, App, AppContext, ClickEvent, Context, CursorStyle, Div, ElementId, Entity,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Pixels, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, Window, anchored, deferred, div,
    point, px,
};
use std::rc::Rc;

/// Which of the palette's colours a piece of text takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone {
    Text,
    Muted,
    Accent,
    Success,
    Warning,
    Danger,
}

/// How a button looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Look {
    /// No background until hovered.
    Quiet,
    /// The selected one of several.
    Selected,
    /// The main action of a dialog.
    Primary,
    /// An action that destroys something.
    Destructive,
}

pub(crate) fn tone(p: &ColorPalette, tone: Tone) -> crate::ui::Color {
    match tone {
        Tone::Text => p.text,
        Tone::Muted => p.muted,
        Tone::Accent => p.accent,
        Tone::Success => p.success,
        Tone::Warning => p.warning,
        Tone::Danger => p.danger,
    }
}

/// A horizontal stack, centred across.
pub(crate) fn row() -> Div {
    div().flex().flex_row().items_center()
}

/// A vertical stack.
pub(crate) fn column() -> Div {
    div().flex().flex_col()
}

/// A window position as the application's own point.
pub(crate) fn geometry_point(position: gpui::Point<Pixels>) -> geometry::Point {
    geometry::Point::new(f32::from(position.x), f32::from(position.y))
}

/// Text in a tooltip.
struct Tip {
    text: SharedString,
    p: ColorPalette,
}

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.))
            .py(px(5.))
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(self.p.border))
            .bg(hsla(self.p.sidebar.with_alpha(1.0)))
            .text_size(px(11.))
            .text_color(hsla(self.p.text))
            .child(self.text.clone())
    }
}

/// What the views draw with: the palette, and the means of making elements that send
/// messages or hold state.
pub(crate) struct View<'a, 'b> {
    pub cx: &'a mut Context<'b, Root>,
    pub ui: &'a mut Ui,
    pub p: ColorPalette,
}

impl View<'_, '_> {
    /// A handler that sends `message` when the element is clicked.
    pub(crate) fn on_click(
        &self,
        message: Message,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        self.cx.listener(move |root, _: &ClickEvent, window, cx| {
            root.dispatch(message.clone(), window, cx);
        })
    }

    /// A handler that sends the message `make` builds when a mouse button goes down.
    pub(crate) fn on_mouse_down(
        &self,
        make: impl Fn(&MouseDownEvent) -> Message + 'static,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
        self.cx
            .listener(move |root, event: &MouseDownEvent, window, cx| {
                root.dispatch(make(event), window, cx);
                cx.stop_propagation();
            })
    }

    /// Text of a size in a tone.
    pub(crate) fn text(&self, text: impl Into<SharedString>, size: f32, tone_: Tone) -> Div {
        div()
            .text_size(px(size))
            .text_color(hsla(tone(&self.p, tone_)))
            .child(text.into())
    }

    /// Text that stays on one line, cut off where the room ends.
    pub(crate) fn line(&self, text: impl Into<SharedString>, size: f32, tone_: Tone) -> Div {
        self.text(text, size, tone_)
            .whitespace_nowrap()
            .overflow_hidden()
            .text_ellipsis()
    }

    /// A button holding `content`. Without a message it is disabled.
    pub(crate) fn button(
        &self,
        id: impl Into<ElementId>,
        content: impl IntoElement,
        look: Look,
        message: Option<Message>,
    ) -> Stateful<Div> {
        let p = self.p;
        let (resting, hovered, text) = match look {
            Look::Quiet => (None, p.hover, p.muted),
            Look::Selected => (Some(p.selected), p.selected, p.text),
            Look::Primary => (Some(p.selected), p.hover, p.text),
            Look::Destructive => (None, p.hover, p.danger),
        };
        let mut button = div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .rounded(px(4.))
            .text_color(hsla(if message.is_some() {
                text
            } else {
                text.scale_alpha(0.4)
            }))
            .child(content);
        if let Some(resting) = resting {
            button = button.bg(hsla(resting));
        }
        if look == Look::Primary {
            button = button.border_1().border_color(hsla(p.border));
        }
        if let Some(message) = message {
            button = button
                .cursor(CursorStyle::PointingHand)
                .hover(move |style| style.bg(hsla(hovered)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(self.on_click(message));
        }
        button
    }

    /// A text button of a dialog: padded, one line.
    pub(crate) fn label_button(
        &self,
        id: impl Into<ElementId>,
        label: &'static str,
        look: Look,
        message: Option<Message>,
    ) -> Stateful<Div> {
        self.button(id, div().text_size(px(11.)).child(label), look, message)
            .px(px(10.))
            .py(px(6.))
    }

    /// Shows `tip` when the pointer rests on `element`.
    pub(crate) fn tip(
        &self,
        element: Stateful<Div>,
        tip: impl Into<SharedString>,
    ) -> Stateful<Div> {
        let text: SharedString = tip.into();
        let p = self.p;
        element.tooltip(move |_, cx| -> AnyView {
            cx.new(|_| Tip {
                text: text.clone(),
                p,
            })
            .into()
        })
    }

    /// An icon in a tone.
    pub(crate) fn icon(&self, kind: Icon, tone_: Tone) -> impl IntoElement {
        icon(kind, tone(&self.p, tone_), 14.0)
    }

    /// A small icon button with a tooltip.
    pub(crate) fn action(
        &self,
        id: impl Into<ElementId>,
        kind: Icon,
        tip: &'static str,
        message: Message,
    ) -> Stateful<Div> {
        let button = self
            .button(id, self.icon(kind, Tone::Muted), Look::Quiet, Some(message))
            .p(px(5.));
        self.tip(button, tip)
    }

    /// A horizontal rule.
    pub(crate) fn separator(&self) -> Div {
        div().w_full().h(px(1.)).flex_none().bg(hsla(self.p.border))
    }

    /// A vertical rule.
    pub(crate) fn vseparator(&self) -> Div {
        div().h_full().w(px(1.)).flex_none().bg(hsla(self.p.border))
    }

    /// A small status dot.
    pub(crate) fn dot(&self, color: crate::ui::Color) -> Div {
        div()
            .size(px(4.))
            .flex_none()
            .rounded(px(2.))
            .bg(hsla(color))
    }

    /// The panel menus and popovers sit on.
    pub(crate) fn panel(&self) -> Div {
        div()
            .rounded(px(6.))
            .border_1()
            .border_color(hsla(self.p.border))
            .bg(hsla(self.p.sidebar.with_alpha(1.0)))
    }

    /// One entry of a menu. Without a message it is disabled.
    pub(crate) fn menu_item(
        &self,
        id: impl Into<ElementId>,
        kind: Icon,
        label: &'static str,
        message: Option<Message>,
        look: Look,
    ) -> Stateful<Div> {
        let tone_ = if look == Look::Destructive {
            Tone::Danger
        } else {
            Tone::Muted
        };
        self.button(
            id,
            row()
                .gap(px(8.))
                .child(self.icon(kind, tone_))
                .child(div().text_size(px(11.)).child(label)),
            look,
            message,
        )
        .w_full()
        .px(px(8.))
        .py(px(6.))
    }

    /// A text field the application owns the value of.
    pub(crate) fn field(&mut self, id: &'static str, spec: Spec) -> Entity<TextInput> {
        self.ui.field(id, spec, self.cx)
    }

    /// The common specification of a single-line field.
    pub(crate) fn line_spec(
        &self,
        value: &str,
        placeholder: &'static str,
        size: f32,
        on_input: impl Fn(String) -> Message + 'static,
        on_submit: Option<Message>,
    ) -> Spec {
        Spec {
            value: value.to_owned(),
            placeholder,
            size,
            rows: (1, 1),
            submit_on_enter: true,
            on_input: Rc::new(on_input),
            on_submit,
            on_key: None,
            colors: self.p,
        }
    }

    /// A single-line field in a bordered box.
    pub(crate) fn boxed_field(&mut self, id: &'static str, spec: Spec, padding: (f32, f32)) -> Div {
        let field = self.field(id, spec);
        div()
            .w_full()
            .px(px(padding.0))
            .py(px(padding.1))
            .rounded(px(4.))
            .border_1()
            .border_color(hsla(self.p.border))
            .child(field)
    }

    /// `content` as a dialog over everything else. Clicking outside dismisses it when
    /// `dismiss` is a message.
    pub(crate) fn modal(
        &self,
        id: &'static str,
        content: impl IntoElement,
        dismiss: Option<Message>,
    ) -> AnyElement {
        let panel = div()
            .id("modal-panel")
            .occlude()
            .w(px(380.))
            .p(px(20.))
            .rounded(px(7.))
            .border_1()
            .border_color(hsla(self.p.border))
            .bg(hsla(self.p.sidebar.with_alpha(1.0)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(content);
        let mut area = div()
            .id(id)
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(crate::ui::Color::BLACK.with_alpha(0.45)))
            .child(panel);
        if let Some(message) = dismiss {
            area = area.on_mouse_down(
                MouseButton::Left,
                self.on_mouse_down(move |_| message.clone()),
            );
        }
        area.into_any_element()
    }

    /// A menu anchored at a window position, kept inside the window.
    pub(crate) fn popup(&self, at: geometry::Point, content: impl IntoElement) -> AnyElement {
        deferred(
            anchored()
                .position(point(px(at.x), px(at.y)))
                .snap_to_window_with_margin(px(4.))
                .child(content),
        )
        .with_priority(2)
        .into_any_element()
    }

    /// A transparent layer over the window that sends `message` when pressed.
    pub(crate) fn dismiss_layer(&self, id: &'static str, message: Message) -> AnyElement {
        let on_secondary = message.clone();
        div()
            .id(id)
            .occlude()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                self.on_mouse_down(move |_| message.clone()),
            )
            .on_mouse_down(
                MouseButton::Right,
                self.on_mouse_down(move |_| on_secondary.clone()),
            )
            .into_any_element()
    }
}
