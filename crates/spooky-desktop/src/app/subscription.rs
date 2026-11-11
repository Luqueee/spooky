use super::{App, Message};
use crate::{
    features::attachments::{self},
    link::{self},
    shortcuts,
};
use iced::{Subscription, event, keyboard, window};
use std::time::Duration;

fn hotkey_events() -> impl iced::futures::Stream<Item = Message> {
    use iced::futures::SinkExt;
    iced::stream::channel(8, async |mut output| {
        while let Some(id) = link::next_hotkey().await {
            if output.send(Message::Hotkey(id)).await.is_err() {
                return;
            }
        }
    })
}
fn background_events() -> impl iced::futures::Stream<Item = Message> {
    use iced::futures::SinkExt;
    iced::stream::channel(32, async |mut output| {
        while let Some(event) = link::next_event().await {
            if output.send(Message::Link(event)).await.is_err() {
                return;
            }
        }
    })
}
pub(crate) fn subscription(app: &App) -> Subscription<Message> {
    let blink = if app.blink_wanted() {
        iced::time::every(Duration::from_millis(530)).map(|_| Message::Blink)
    } else {
        Subscription::none()
    };
    Subscription::batch([
        Subscription::run(background_events),
        Subscription::run(hotkey_events),
        blink,
        event::listen_with(|event, status, id| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                physical_key,
                text,
                ..
            }) if status == event::Status::Ignored
                || key == keyboard::Key::Named(keyboard::key::Named::Escape)
                || shortcuts::primary(modifiers) =>
            {
                Some(Message::Key(
                    key,
                    physical_key,
                    modifiers,
                    text.map(|s| s.to_string()),
                    status,
                ))
            }
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::Modifiers(modifiers))
            }
            iced::Event::Window(window::Event::Opened { .. }) => Some(Message::Opened(id)),
            iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocus(true)),
            iced::Event::Window(window::Event::Unfocused) => Some(Message::WindowFocus(false)),
            iced::Event::Window(window::Event::CloseRequested) => Some(Message::Close(id)),
            iced::Event::Window(window::Event::FileDropped(path)) => {
                Some(Message::Attachments(attachments::Message::Dropped(path)))
            }
            _ => None,
        }),
    ])
}
