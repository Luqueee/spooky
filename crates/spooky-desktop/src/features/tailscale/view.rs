//! The picker as drawn: devices, search, and the SSH user.

use super::{Discovery, Picker, user_is_valid};
use crate::{
    Message,
    gui::{Icon, Look, Tone, View, column, row},
};
use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled,
    div, px,
};

/// The message a control sends.
fn tell(message: super::Message) -> Message {
    Message::Tailscale(message)
}

pub(crate) fn view(picker: &Picker, v: &mut View) -> AnyElement {
    let query = picker.query.to_lowercase();
    let user = picker.user.trim();
    let user_valid = user_is_valid(user);
    let colors = v.p;
    let mut devices = column().gap(px(3.));
    match &picker.discovery {
        Discovery::Idle | Discovery::Loading => {
            devices = devices.child(v.text("Reading local Tailscale client…", 11.0, Tone::Muted));
        }
        Discovery::Failed(error) => {
            devices = devices.child(v.text(error.clone(), 11.0, Tone::Text));
        }
        Discovery::Ready(network) => {
            let mut shown = 0;
            for (index, peer) in network.peers.iter().enumerate() {
                if !peer.name.to_lowercase().contains(&query) && !peer.host.contains(&query) {
                    continue;
                }
                shown += 1;
                let name_tone = if peer.online { Tone::Text } else { Tone::Muted };
                let detail = format!(
                    "{} · {} · {}",
                    peer.host,
                    peer.os,
                    if peer.online { "online" } else { "offline" }
                );
                let entry = row()
                    .w_full()
                    .gap(px(8.))
                    .child(v.dot(if peer.online {
                        colors.success
                    } else {
                        colors.muted
                    }))
                    .child(
                        column()
                            .flex_1()
                            .gap(px(3.))
                            .child(v.text(peer.name.clone(), 11.0, name_tone))
                            .child(v.text(detail, 9.0, Tone::Muted)),
                    );
                let message = (peer.online && user_valid)
                    .then(|| tell(super::Message::Select(peer.host.clone())));
                devices = devices.child(
                    v.button(("tailscale-peer", index), entry, Look::Quiet, message)
                        .w_full()
                        .p(px(8.)),
                );
            }
            if shown == 0 {
                devices = devices.child(v.text(
                    if network.peers.is_empty() {
                        "No other devices in this tailnet."
                    } else {
                        "No matching devices."
                    },
                    11.0,
                    Tone::Muted,
                ));
            }
        }
    }
    let loading = matches!(picker.discovery, Discovery::Loading);
    let refresh = v
        .button(
            "tailscale-refresh",
            div().text_size(px(10.)).child("Refresh"),
            Look::Quiet,
            (!loading).then(|| tell(super::Message::Refresh)),
        )
        .px(px(8.))
        .py(px(5.));
    let query_spec = v.line_spec(
        &picker.query,
        "Search devices or IP",
        11.0,
        |query| tell(super::Message::Query(query)),
        None,
    );
    let user_spec = v.line_spec(
        &picker.user,
        "SSH user (optional)",
        11.0,
        |user| tell(super::Message::User(user)),
        None,
    );
    let query_field = v.boxed_field("tailscale-query", query_spec, (7.0, 7.0));
    let user_field = v.boxed_field("tailscale-user", user_spec, (7.0, 7.0));
    column()
        .gap(px(10.))
        .child(
            row()
                .w_full()
                .justify_between()
                .child(v.text("Tailscale devices", 14.0, Tone::Text))
                .child(v.action(
                    "tailscale-close",
                    Icon::Close,
                    "Close Tailscale devices",
                    tell(super::Message::Close),
                )),
        )
        .child(v.text(
            "Uses your installed Tailscale client. Selecting a device prepares SSH; Connect starts the session.",
            10.0,
            Tone::Muted,
        ))
        .child(query_field)
        .child(user_field)
        .child(v.text(
            if user_valid {
                "Remote device needs SSH and spooky-server."
            } else {
                "Invalid SSH user: use letters, digits, dot, underscore or hyphen."
            },
            10.0,
            Tone::Muted,
        ))
        .child(
            div()
                .id("tailscale-devices")
                .h(px(240.))
                .overflow_y_scroll()
                .child(devices),
        )
        .child(div().child(refresh))
        .into_any_element()
}
