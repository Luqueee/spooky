//! The search strip as drawn.

use super::Search;
use crate::{
    Message,
    gui::{Icon, Tone, View, hsla, row},
};
use gpui::{AnyElement, IntoElement, ParentElement, Styled, div, px};
use spooky_core::FindDirection;

/// The message a control sends.
fn tell(message: super::Message) -> Message {
    Message::Search(message)
}

/// The scrollback search strip above the terminal; nothing while the search is closed.
pub(crate) fn view(search: &Search, v: &mut View) -> Option<AnyElement> {
    let bar = search.bar.as_ref()?;
    let result = match bar.result {
        None => String::new(),
        Some(None) => "No matches".to_owned(),
        Some(Some((current, total))) => format!("{current} of {total}"),
    };
    let spec = v.line_spec(
        &bar.query,
        "Find in scrollback",
        11.0,
        |query| tell(super::Message::Input(query)),
        Some(tell(super::Message::Step(FindDirection::Next))),
    );
    let input = v.field("search-input", spec);
    let strip = row()
        .w_full()
        .gap(px(8.))
        .px(px(8.))
        .py(px(3.))
        .bg(hsla(v.p.sidebar))
        .child(v.icon(Icon::Search, Tone::Muted))
        .child(div().flex_1().px(px(6.)).py(px(4.)).child(input))
        .child(v.text(result, 10.0, Tone::Muted))
        .child(v.action(
            "search-previous",
            Icon::ChevronUp,
            "Previous match",
            tell(super::Message::Step(FindDirection::Previous)),
        ))
        .child(v.action(
            "search-next",
            Icon::Chevron,
            "Next match",
            tell(super::Message::Step(FindDirection::Next)),
        ))
        .child(v.action(
            "search-close",
            Icon::Close,
            "Close search",
            tell(super::Message::Close),
        ));
    Some(strip.into_any_element())
}
