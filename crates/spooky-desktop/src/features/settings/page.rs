//! The settings page as drawn: every option of `config.toml`, applied as it changes.

use super::{Change, DEFAULT_FONT, Settings};
use crate::{
    Message,
    config::{Appearance, Config, OnBlur, Osc52, SizeMode},
    features::updates::{Status, Updates},
    ui::*,
};
use iced::{
    Alignment, Border, Element, Length,
    widget::{button, column, container, pick_list, row, scrollable, text, toggler},
};
use spooky_core::update::{CURRENT, Channel};

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
                text(label).size(12).style(text_tone(TextTone::Text)),
                text(hint.into()).size(10).style(text_tone(TextTone::Muted))
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
        text(title).size(11).style(text_tone(TextTone::Accent)),
        container(rows).style(|theme| {
            let colors = color_palette_for(theme);
            container::Style {
                border: Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..surface(colors.sidebar)
            }
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
            .style(text_tone(TextTone::Text))
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

/// The release channel, the check at startup, and what is known about newer releases.
fn updates_section<'a>(config: &'a Config, updates: &'a Updates) -> Element<'a, Message> {
    let channel = |channel: Channel| {
        (
            channel.label(),
            config.update_channel == channel,
            apply(Change::UpdateChannel(channel)),
        )
    };
    let status = match updates.status() {
        Status::Idle => "Not checked yet.".to_owned(),
        Status::Checking => "Checking…".to_owned(),
        Status::UpToDate => format!(
            "This is the newest {} release.",
            config.update_channel.label().to_lowercase()
        ),
        Status::Available(available) => format!(
            "Version {} is available{}.",
            available.version,
            if available.prerelease {
                " (pre-release)"
            } else {
                ""
            }
        ),
        Status::Failed(reason) => format!("Could not check: {reason}"),
    };
    let mut buttons = row![
        button(text("Check now").size(11))
            .padding([4, 10])
            .style(quiet)
            .on_press_maybe(
                (*updates.status() != Status::Checking).then_some(Message::UpdateCheck)
            ),
    ]
    .spacing(6);
    if updates.available().is_some() {
        buttons = buttons.push(
            button(text("Update…").size(11))
                .padding([4, 10])
                .style(primary)
                .on_press(Message::UpdateAsk),
        );
    }
    section(
        "Updates",
        vec![
            option(
                "Release channel",
                "Stable follows production releases. Development also takes pre-releases such as 0.2.0-dev.1.",
                choice(vec![channel(Channel::Stable), channel(Channel::Dev)]),
            ),
            option(
                "Check when Spooky starts",
                "Asks GitHub's public release list for a newer version and sends nothing else.",
                switch(config.check_updates, Change::CheckUpdates),
            ),
            option("Spooky", format!("Version {CURRENT}. {status}"), buttons),
        ],
    )
}

/// What this session offers the page, which shapes the options it shows.
#[derive(Clone, Copy)]
pub(crate) struct Session {
    /// Whether the session has a global shortcut.
    pub shortcuts_available: bool,
    /// Whether that shortcut can also hide the window and bring it back.
    pub can_hide: bool,
    /// Whether the window's background can be translucent here.
    pub glass_available: bool,
    /// Whether the system asks for solid backgrounds.
    pub reduce_transparency: bool,
}

/// The page that replaces the terminal while the settings tab is selected.
pub(crate) fn view<'a>(
    settings: &'a Settings,
    config: &'a Config,
    themes: &'a ThemeCatalog,
    updates: &'a Updates,
    session: Session,
) -> Element<'a, Message> {
    let Session {
        shortcuts_available,
        can_hide,
        glass_available,
        reduce_transparency,
    } = session;
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
    window.push(option(
        "When another app is clicked",
        if can_hide {
            "Hiding brings the window back with the global shortcut"
        } else {
            "Hiding needs a global shortcut and a desktop that lets Spooky hide its window, which is not available here"
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
    window.push(option(
        "Background opacity",
        if !glass_available {
            "Transparency unavailable; the background stays solid"
        } else if reduce_transparency {
            "macOS Reduce Transparency is on; the background stays solid"
        } else if cfg!(target_os = "macos") {
            "Subtle background blur; 100% uses a solid background"
        } else {
            "Lets the desktop show through, blurred where the desktop offers it (KDE's Blur effect); 100% is solid"
        },
        stepper(
            format!("{} %", config.background_opacity),
            apply(Change::BackgroundOpacity(
                config.background_opacity - PERCENT_STEP,
            )),
            apply(Change::BackgroundOpacity(
                config.background_opacity + PERCENT_STEP,
            )),
        ),
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
            container(text(shown).size(11).style(text_tone(TextTone::Text)))
                .padding([4, 10])
                .style(|theme| {
                    let colors = color_palette_for(theme);
                    container::Style {
                        border: Border {
                            color: colors.border,
                            width: 1.0,
                            radius: 5.0.into(),
                        },
                        ..surface(colors.hover)
                    }
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
        Element::from(
            text("Not available")
                .size(11)
                .style(text_tone(TextTone::Muted)),
        )
    };
    let shortcut = vec![option(
        "Global shortcut",
        if recording {
            "Hold Ctrl, Alt or Cmd with a key, or press a function key. Escape cancels"
        } else if can_hide {
            "Shows and hides the window from any application"
        } else {
            "Brings the window forward from any application; this desktop does not let Spooky hide or place it"
        },
        shortcut,
    )];

    let family = config.font_family.as_deref().unwrap_or(DEFAULT_FONT);
    let terminal = vec![
        option(
            "Font",
            "Monospace families installed on this machine; the default follows the platform",
            pick_list(settings.fonts(), Some(family.to_owned()), |name: String| {
                apply(Change::FontFamily((name != DEFAULT_FONT).then_some(name)))
            })
            .text_size(11)
            .width(220),
        ),
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
    let themes_path = Config::themes_dir()
        .map(|path| format!("Theme files: {}/*.toml", path.display()))
        .unwrap_or_default();
    let theme_issues = themes.issues().join("\n");
    let missing_theme = config
        .custom_theme
        .as_deref()
        .filter(|id| themes.get(id).is_none())
        .map(|id| format!("Selected theme '{id}' is missing; using the built-in theme."));
    let problem = settings.problem().unwrap_or_default().to_owned();
    // The bar stays put while the sections scroll, so a refused change is seen
    // wherever on the page it was made.
    let bar = container(
        row![
            text("Settings").size(16).style(text_tone(TextTone::Text)),
            container(text(problem).size(11).style(text_tone(TextTone::Danger)))
                .width(Length::Fill),
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
    let mut sections = column![
        section("Window", window),
        section(
            "Appearance",
            vec![option(
                "Color theme",
                format!(
                    "Choose a palette or add .toml theme files in {}",
                    Config::themes_dir().map_or_else(
                        |_| "the Spooky config directory".to_owned(),
                        |path| path.display().to_string(),
                    )
                ),
                pick_list(
                    themes.themes(),
                    config
                        .custom_theme
                        .as_deref()
                        .and_then(|id| themes.get(id))
                        .or_else(|| {
                            themes.get(match config.appearance {
                                Appearance::Midnight => "midnight",
                                Appearance::Spooky => "spooky",
                                Appearance::Nord => "nord",
                                Appearance::Dracula => "dracula",
                            })
                        }),
                    |theme: ThemeDefinition| { apply(Change::Appearance(theme.id)) },
                )
                .width(150)
                .text_size(11)
                .padding([5, 9])
                .menu_height(240)
                .style(|theme, status| {
                    let colors = color_palette_for(theme);
                    let mut style = iced::widget::pick_list::default(theme, status);
                    style.background = colors.sidebar.into();
                    style.text_color = colors.text;
                    style.placeholder_color = colors.muted;
                    style.handle_color = colors.muted;
                    style.border = Border {
                        color: if matches!(status, iced::widget::pick_list::Status::Active) {
                            colors.border
                        } else {
                            colors.accent
                        },
                        width: 1.0,
                        radius: 5.0.into(),
                    };
                    style
                })
                .menu_style(|theme| {
                    let colors = color_palette_for(theme);
                    let mut style = iced::overlay::menu::default(theme);
                    style.background = colors.sidebar.into();
                    style.text_color = colors.text;
                    style.selected_text_color = colors.text;
                    style.selected_background = colors.selected.into();
                    style.border = Border {
                        color: colors.border,
                        width: 1.0,
                        radius: 5.0.into(),
                    };
                    style
                }),
            )],
        ),
        section("Shortcut", shortcut),
        section("Terminal", terminal),
        updates_section(config, updates),
        text(file).size(10).style(text_tone(TextTone::Muted)),
        text(themes_path).size(10).style(text_tone(TextTone::Muted)),
    ]
    .spacing(20)
    .width(640);
    if let Some(missing_theme) = missing_theme {
        sections = sections.push(
            text(missing_theme)
                .size(10)
                .style(text_tone(TextTone::Warning)),
        );
    }
    if !theme_issues.is_empty() {
        sections = sections.push(
            text(format!("Some theme files were skipped:\n{theme_issues}"))
                .size(10)
                .style(text_tone(TextTone::Danger)),
        );
    }

    container(column![
        bar,
        separator(),
        scrollable(container(sections).padding([16, 0]).center_x(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
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
