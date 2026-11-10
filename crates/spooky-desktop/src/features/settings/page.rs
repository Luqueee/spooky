//! The settings page as drawn: every option of `config.toml`, applied as it changes.

use super::{Change, Settings};
use crate::{
    Message,
    config::{Config, OnBlur, Osc52, SizeMode},
    ui::*,
};
use iced::{
    Alignment, Border, Element, Length,
    widget::{button, column, container, row, scrollable, text, toggler},
};

/// Percent points one press of a size stepper moves.
const PERCENT_STEP: f32 = 5.0;
/// Pixels one press of a size stepper moves.
const PIXEL_STEP: f32 = 40.0;
/// The presets, as `(label, width %, height %)`.
const PRESETS: [(&str, f32, f32); 3] = [
    ("Full screen", 100.0, 100.0),
    ("Top half", 100.0, 50.0),
    ("Top third", 100.0, 34.0),
];

/// One option: its name and a line of explanation on the left, its control on the right.
fn option<'a>(
    label: &'static str,
    hint: impl Into<String>,
    control: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    container(
        row![
            column![
                text(label).size(12).color(TEXT),
                text(hint.into()).size(10).color(MUTED)
            ]
            .spacing(2)
            .width(Length::Fill),
            control.into(),
        ]
        .spacing(16)
        .align_y(Alignment::Center),
    )
    .padding([10, 12])
    .into()
}

/// A titled group of options.
fn section<'a>(title: &'static str, options: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut rows = column![].width(Length::Fill);
    for (index, option) in options.into_iter().enumerate() {
        if index > 0 {
            rows = rows.push(separator());
        }
        rows = rows.push(option);
    }
    column![
        text(title).size(11).color(ACCENT),
        container(rows).style(|_| container::Style {
            border: Border {
                color: BORDER,
                width: 1.0,
                radius: 6.0.into(),
            },
            ..surface(SIDEBAR)
        }),
    ]
    .spacing(8)
    .into()
}

/// Buttons of which one is selected.
fn choice<'a>(options: Vec<(&'static str, bool, Message)>) -> Element<'a, Message> {
    let mut buttons = row![].spacing(2);
    for (label, selected, message) in options {
        buttons = buttons.push(
            button(text(label).size(11))
                .padding([4, 10])
                .style(move |theme, status| selectable(selected, theme, status))
                .on_press(message),
        );
    }
    buttons.into()
}

fn step_button<'a>(label: &'static str, message: Message) -> Element<'a, Message> {
    button(text(label).size(13))
        .padding([2, 10])
        .style(quiet)
        .on_press(message)
        .into()
}

/// A value with buttons to lower and raise it.
fn stepper<'a>(value: String, lower: Message, raise: Message) -> Element<'a, Message> {
    row![
        step_button("−", lower),
        text(value)
            .size(11)
            .color(TEXT)
            .width(64)
            .align_x(Alignment::Center),
        step_button("+", raise),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn switch<'a>(on: bool, change: fn(bool) -> Change) -> Element<'a, Message> {
    toggler(on)
        .size(16)
        .on_toggle(move |value| apply(change(value)))
        .into()
}

/// The message a control sends to change `change`.
fn apply(change: Change) -> Message {
    Message::Settings(super::Message::Change(change))
}

/// The page that replaces the terminal while it is open. `shortcuts_available` is
/// whether the session has a global shortcut that can be recorded and can bring a
/// hidden window back.
pub(crate) fn view<'a>(
    settings: &'a Settings,
    config: &'a Config,
    shortcuts_available: bool,
) -> Element<'a, Message> {
    let recording = settings.recording();

    let percent = config.size_mode() == SizeMode::Percent;
    let (width, height, lower_width, raise_width, lower_height, raise_height) = if percent {
        (
            format!("{} %", config.width_percent),
            format!("{} %", config.height_percent),
            Change::WidthPercent(config.width_percent - PERCENT_STEP),
            Change::WidthPercent(config.width_percent + PERCENT_STEP),
            Change::HeightPercent(config.height_percent - PERCENT_STEP),
            Change::HeightPercent(config.height_percent + PERCENT_STEP),
        )
    } else {
        (
            format!("{} px", config.width_px()),
            format!("{} px", config.height_px()),
            Change::WidthPixels(config.width_px() - PIXEL_STEP),
            Change::WidthPixels(config.width_px() + PIXEL_STEP),
            Change::HeightPixels(config.height_px() - PIXEL_STEP),
            Change::HeightPixels(config.height_px() + PIXEL_STEP),
        )
    };
    let mut window = vec![
        option(
            "Size",
            "A share of the display, pinned to its top edge, or fixed pixels",
            choice(vec![
                ("Percent", percent, apply(Change::Size(SizeMode::Percent))),
                ("Pixels", !percent, apply(Change::Size(SizeMode::Pixels))),
            ]),
        ),
        option(
            "Width",
            "Of the window when it opens",
            stepper(width, apply(lower_width), apply(raise_width)),
        ),
        option(
            "Height",
            "Of the window when it opens",
            stepper(height, apply(lower_height), apply(raise_height)),
        ),
        option(
            "Presets",
            "Width and height as a share of the display",
            choice(
                PRESETS
                    .iter()
                    .map(|&(label, width, height)| {
                        (
                            label,
                            percent
                                && config.width_percent == width
                                && config.height_percent == height,
                            apply(Change::Preset(width, height)),
                        )
                    })
                    .collect(),
            ),
        ),
    ];
    let can_hide = shortcuts_available;
    window.push(option(
        "When another app is clicked",
        if can_hide {
            "Hiding brings the window back with the global shortcut"
        } else {
            "Hiding needs the global shortcut, which is not available here"
        },
        choice(vec![
            (
                "Nothing",
                config.on_blur == OnBlur::None,
                apply(Change::OnBlur(OnBlur::None)),
            ),
            (
                "Hide",
                config.on_blur == OnBlur::Hide,
                apply(Change::OnBlur(OnBlur::Hide)),
            ),
            (
                "Stay on top",
                config.on_blur == OnBlur::Top,
                apply(Change::OnBlur(OnBlur::Top)),
            ),
        ]),
    ));
    if cfg!(target_os = "macos") {
        window.push(option(
            "Dock icon",
            "Off runs without a Dock icon; the global shortcut is the way back",
            switch(config.dock_icon, Change::DockIcon),
        ));
    }

    let shortcut = if shortcuts_available {
        let shown = if recording {
            "Press the new keys…".to_owned()
        } else {
            config.shortcut.clone()
        };
        row![
            container(text(shown).size(11).color(TEXT))
                .padding([4, 10])
                .style(|_| container::Style {
                    border: Border {
                        color: BORDER,
                        width: 1.0,
                        radius: 5.0.into(),
                    },
                    ..surface(HOVER)
                }),
            if recording {
                choice(vec![("Cancel", false, apply(Change::CancelRecording))])
            } else {
                choice(vec![
                    ("Record", false, apply(Change::RecordShortcut)),
                    ("Reset", false, apply(Change::ResetShortcut)),
                ])
            },
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    } else {
        Element::from(text("Not available").size(11).color(MUTED))
    };
    let shortcut = vec![option(
        "Global shortcut",
        if recording {
            "Hold Ctrl, Alt or Cmd with a key, or press a function key. Escape cancels"
        } else {
            "Shows and hides the window from any application"
        },
        shortcut,
    )];

    let terminal = vec![
        option(
            "Font size",
            "Terminal text, in points",
            stepper(
                format!("{} pt", config.font_size),
                apply(Change::FontSize(config.font_size - 1.0)),
                apply(Change::FontSize(config.font_size + 1.0)),
            ),
        ),
        option(
            "Blinking cursor",
            "Off keeps every cursor solid, whatever a program asks for",
            switch(config.cursor_blink, Change::CursorBlink),
        ),
        option(
            "Clipboard writes from programs",
            "OSC 52: how tmux and vim over SSH copy; reading is never allowed",
            switch(config.osc52 == Osc52::Write, Change::Osc52),
        ),
    ];

    let file = Config::path()
        .map(|path| format!("Settings file: {}", path.display()))
        .unwrap_or_default();
    let problem = settings.problem().unwrap_or_default().to_owned();
    // The bar stays put while the sections scroll, so a refused change is seen
    // wherever on the page it was made.
    let bar = container(
        row![
            text("Settings").size(16).color(TEXT),
            container(text(problem).size(11).color(DANGER)).width(Length::Fill),
            button(text("Done").size(11))
                .padding([5, 14])
                .style(primary)
                .on_press(Message::Settings(super::Message::Close)),
        ]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(640),
    )
    .padding([10, 0])
    .center_x(Length::Fill);
    let sections = column![
        section("Window", window),
        section("Shortcut", shortcut),
        section("Terminal", terminal),
        text(file).size(10).color(MUTED),
    ]
    .spacing(20)
    .width(640);

    container(column![
        bar,
        separator(),
        scrollable(container(sections).padding([16, 0]).center_x(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| surface(BACKGROUND))
    .into()
}

#[cfg(test)]
mod tests {
    use super::PRESETS;
    use crate::config::Config;

    #[test]
    fn every_preset_is_a_size_the_file_accepts() {
        for (label, width_percent, height_percent) in PRESETS {
            let config = Config {
                width_percent,
                height_percent,
                ..Config::default()
            };
            assert_eq!(config.validate(), Ok(()), "{label}");
        }
    }
}
