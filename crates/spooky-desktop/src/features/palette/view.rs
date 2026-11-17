//! The palette's list, drawn inside a modal.

use super::{Message as PaletteMessage, Palette};
use crate::{
    Message,
    gui::{Look, Tone, View, column, row},
};
use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, px,
};

/// The message a control sends.
fn tell(message: PaletteMessage) -> Message {
    Message::Palette(message)
}

/// The palette's contents, or `None` while it is closed.
pub(crate) fn view(palette: &Palette, v: &mut View) -> Option<AnyElement> {
    let open = palette.open.as_ref()?;
    let mut list = column().gap(px(2.));
    for (position, command) in open.results.iter().enumerate() {
        let selected = position == open.selected;
        let entry = row()
            .w_full()
            .justify_between()
            .child(v.text(command.title(), 12.0, Tone::Text))
            .child(v.text(command.id(), 10.0, Tone::Muted));
        let look = if selected {
            Look::Selected
        } else {
            Look::Quiet
        };
        list = list.child(
            v.button(
                ("palette-command", position),
                entry,
                look,
                Some(tell(PaletteMessage::Run(*command))),
            )
            .w_full()
            .px(px(8.))
            .py(px(6.)),
        );
    }
    if open.results.is_empty() {
        list = list.child(v.text("No command matches", 11.0, Tone::Muted));
    }
    let spec = v.line_spec(
        &open.query,
        "Type a command",
        12.0,
        |query| tell(PaletteMessage::Query(query)),
        Some(tell(PaletteMessage::Submit)),
    );
    let input = v.boxed_field("palette-input", spec, (8.0, 8.0));
    Some(
        column()
            .gap(px(10.))
            .child(input)
            .child(
                div()
                    .id("palette-list")
                    .h(px(260.))
                    .overflow_y_scroll()
                    .child(div().pr(px(12.)).child(list)),
            )
            .into_any_element(),
    )
}
