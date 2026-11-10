//! The search strip as drawn.

use super::Search;
use crate::{Message, ui::*};
use iced::{
    Alignment, Element,
    widget::{container, row, text, text_input},
};
use spooky_core::FindDirection;

/// The message a control sends.
fn tell(message: super::Message) -> Message {
    Message::Search(message)
}

/// The scrollback search strip above the terminal; nothing while the search is closed.
pub(crate) fn view(search: &Search) -> Option<Element<'_, Message>> {
    let bar = search.bar.as_ref()?;
    let result = match bar.result {
        None => String::new(),
        Some(None) => "No matches".to_owned(),
        Some(Some((current, total))) => format!("{current} of {total}"),
    };
    Some(
        container(
            row![
                icon(Icon::Search, MUTED),
                text_input("Find in scrollback", &bar.query)
                    .id("search-input")
                    .size(11)
                    .padding([4, 6])
                    .on_input(|query| tell(super::Message::Input(query)))
                    .on_submit(tell(super::Message::Step(FindDirection::Next))),
                text(result).size(10).color(MUTED),
                action(
                    Icon::ChevronUp,
                    "Previous match",
                    tell(super::Message::Step(FindDirection::Previous))
                ),
                action(
                    Icon::Chevron,
                    "Next match",
                    tell(super::Message::Step(FindDirection::Next))
                ),
                action(Icon::Close, "Close search", tell(super::Message::Close)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding([4, 10])
        .style(|_| surface(SIDEBAR))
        .into(),
    )
}
