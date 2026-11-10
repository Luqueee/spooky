//! The palette's list, drawn inside a modal.

use super::{Message as PaletteMessage, Palette};
use crate::{Message, ui::*};
use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, scrollable, text, text_input},
};

/// The message a control sends.
fn tell(message: PaletteMessage) -> Message {
    Message::Palette(message)
}

/// The palette's contents, or `None` while it is closed.
pub(crate) fn view(palette: &Palette) -> Option<Element<'_, Message>> {
    let open = palette.open.as_ref()?;
    let mut list = column![].spacing(2);
    for (position, command) in open.results.iter().enumerate() {
        let selected = position == open.selected;
        list = list.push(
            button(
                row![
                    text(command.title()).size(12).color(TEXT),
                    iced::widget::Space::new().width(Length::Fill),
                    text(command.id()).size(10).color(MUTED),
                ]
                .align_y(Alignment::Center),
            )
            .padding([6, 8])
            .width(Length::Fill)
            .style(move |theme, status| selectable(selected, theme, status))
            .on_press(tell(PaletteMessage::Run(*command))),
        );
    }
    if open.results.is_empty() {
        list = list.push(text("No command matches").size(11).color(MUTED));
    }
    Some(
        column![
            text_input("Type a command", &open.query)
                .id("palette-input")
                .size(12)
                .padding(8)
                .on_input(|query| tell(PaletteMessage::Query(query)))
                .on_submit(tell(PaletteMessage::Submit)),
            container(scrollable(container(list).padding(iced::Padding {
                right: 12.0,
                ..iced::Padding::ZERO
            })))
            .height(260),
        ]
        .spacing(10)
        .into(),
    )
}
