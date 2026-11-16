//! The picker as drawn: devices, search, and the SSH user.

use super::{Discovery, Picker, user_is_valid};
use crate::{Message, ui::*};
use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, scrollable, text, text_input},
};

/// The message a control sends.
fn tell(message: super::Message) -> Message {
    Message::Tailscale(message)
}

pub(crate) fn view(picker: &Picker, colors: ColorPalette) -> Element<'_, Message> {
    let query = picker.query.to_lowercase();
    let user = picker.user.trim();
    let user_valid = user_is_valid(user);
    let mut devices = column![].spacing(3);
    match &picker.discovery {
        Discovery::Idle | Discovery::Loading => {
            devices = devices.push(
                text("Reading local Tailscale client…")
                    .size(11)
                    .style(text_tone(TextTone::Muted)),
            );
        }
        Discovery::Failed(error) => {
            devices = devices.push(text(error).size(11).style(text_tone(TextTone::Text)));
        }
        Discovery::Ready(network) => {
            let mut shown = 0;
            for peer in &network.peers {
                if !peer.name.to_lowercase().contains(&query) && !peer.host.contains(&query) {
                    continue;
                }
                shown += 1;
                let entry = button(
                    row![
                        dot(if peer.online {
                            colors.success
                        } else {
                            colors.muted
                        }),
                        column![
                            text(&peer.name).size(11).style(text_tone(if peer.online {
                                TextTone::Text
                            } else {
                                TextTone::Muted
                            })),
                            text(format!(
                                "{} · {} · {}",
                                peer.host,
                                peer.os,
                                if peer.online { "online" } else { "offline" }
                            ))
                            .size(9)
                            .style(text_tone(TextTone::Muted)),
                        ]
                        .spacing(3)
                        .width(Length::Fill),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center),
                )
                .padding(8)
                .width(Length::Fill)
                .style(quiet);
                devices = devices.push(if peer.online && user_valid {
                    entry.on_press(tell(super::Message::Select(peer.host.clone())))
                } else {
                    entry
                });
            }
            if shown == 0 {
                devices = devices.push(
                    text(if network.peers.is_empty() {
                        "No other devices in this tailnet."
                    } else {
                        "No matching devices."
                    })
                    .size(11)
                    .style(text_tone(TextTone::Muted)),
                );
            }
        }
    }
    let refresh = button(text("Refresh").size(10))
        .padding([5, 8])
        .style(quiet);
    column![
        row![
            text("Tailscale devices").size(14).style(text_tone(TextTone::Text)).width(Length::Fill),
            action(Icon::Close, "Close Tailscale devices", tell(super::Message::Close)),
        ].align_y(Alignment::Center),
        text("Uses your installed Tailscale client. Selecting a device prepares SSH; Connect starts the session.").size(10).style(text_tone(TextTone::Muted)),
        text_input("Search devices or IP", &picker.query).size(11).padding(7).on_input(|query| tell(super::Message::Query(query))),
        text_input("SSH user (optional)", &picker.user).size(11).padding(7).on_input(|user| tell(super::Message::User(user))),
        text(if user_valid { "Remote device needs SSH and spooky-server." } else { "Invalid SSH user: use letters, digits, dot, underscore or hyphen." }).size(10).style(text_tone(TextTone::Muted)),
        container(scrollable(devices)).height(240),
        if matches!(picker.discovery, Discovery::Loading) { refresh } else { refresh.on_press(tell(super::Message::Refresh)) },
    ].spacing(10).into()
}
