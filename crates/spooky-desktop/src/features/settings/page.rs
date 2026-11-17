//! The settings page as drawn: every option of `config.toml`, applied as it changes.

use super::{Change, DEFAULT_FONT, Settings};
use crate::{
    Message,
    config::{Appearance, Config, OnBlur, Osc52, SizeMode},
    features::updates::{Status, Updates},
    gui::{Look, Tone, View, column, hsla, row},
    ui::ThemeCatalog,
};
use gpui::{
    AnyElement, Div, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, anchored, deferred, div, point, px,
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
fn option(
    v: &View,
    label: &'static str,
    hint: impl Into<String>,
    control: impl IntoElement,
) -> AnyElement {
    row()
        .w_full()
        .gap(px(16.))
        .px(px(12.))
        .py(px(10.))
        .child(
            column()
                .flex_1()
                .min_w_0()
                .gap(px(2.))
                .child(v.text(label, 12.0, Tone::Text))
                .child(v.text(hint.into(), 10.0, Tone::Muted)),
        )
        .child(div().flex_shrink_0().child(control))
        .into_any_element()
}

/// A titled group of options.
fn section(v: &View, title: &'static str, options: Vec<AnyElement>) -> AnyElement {
    let mut rows = column().w_full();
    for (index, option) in options.into_iter().enumerate() {
        if index > 0 {
            rows = rows.child(v.separator());
        }
        rows = rows.child(option);
    }
    column()
        .w_full()
        .gap(px(8.))
        .child(v.text(title, 11.0, Tone::Accent))
        .child(v.panel().w_full().child(rows))
        .into_any_element()
}

/// Buttons of which one is selected.
fn choice(v: &View, name: &'static str, options: Vec<(&'static str, bool, Message)>) -> Div {
    let mut buttons = row().gap(px(2.));
    for (index, (label, selected, message)) in options.into_iter().enumerate() {
        let look = if selected {
            Look::Selected
        } else {
            Look::Quiet
        };
        buttons = buttons.child(
            v.button(
                (name, index),
                div().text_size(px(11.)).child(label),
                look,
                Some(message),
            )
            .px(px(10.))
            .py(px(4.)),
        );
    }
    buttons
}

/// A value with buttons to lower and raise it.
fn stepper(v: &View, name: &'static str, value: String, lower: Message, raise: Message) -> Div {
    let lower_id = format!("{name}-lower");
    let raise_id = format!("{name}-raise");
    row()
        .child(step_button_owned(v, lower_id, "−", lower))
        .child(
            div()
                .w(px(64.))
                .flex()
                .justify_center()
                .child(v.text(value, 11.0, Tone::Text)),
        )
        .child(step_button_owned(v, raise_id, "+", raise))
}

fn step_button_owned(
    v: &View,
    id: String,
    label: &'static str,
    message: Message,
) -> impl IntoElement {
    v.button(
        gpui::SharedString::from(id),
        div().text_size(px(13.)).child(label),
        Look::Quiet,
        Some(message),
    )
    .px(px(10.))
    .py(px(2.))
}

/// An on/off switch.
fn switch(v: &View, name: &'static str, on: bool, change: fn(bool) -> Change) -> impl IntoElement {
    let track = if on { v.p.accent } else { v.p.border };
    let knob = div()
        .size(px(12.))
        .rounded(px(6.))
        .bg(hsla(v.p.background.with_alpha(1.0)));
    let mut body = div()
        .w(px(30.))
        .h(px(16.))
        .rounded(px(8.))
        .p(px(2.))
        .flex()
        .items_center()
        .bg(hsla(track));
    body = if on {
        body.justify_end()
    } else {
        body.justify_start()
    };
    div()
        .id(name)
        .cursor_pointer()
        .child(body.child(knob))
        .on_click(v.on_click(apply(change(!on))))
}

/// The message a control sends to change `change`.
fn apply(change: Change) -> Message {
    Message::Settings(super::Message::Change(change))
}

/// A choice list: the current value, and the values when it is open.
fn select(
    v: &View,
    name: &'static str,
    width: f32,
    current: String,
    open: bool,
    items: Vec<(String, bool, Message)>,
) -> Div {
    let toggle = Message::Settings(super::Message::Menu((!open).then_some(name)));
    let button = v
        .button(
            name,
            row()
                .w_full()
                .justify_between()
                .gap(px(6.))
                .child(v.line(current, 11.0, Tone::Text))
                .child(v.text("▾", 10.0, Tone::Muted)),
            Look::Quiet,
            Some(toggle),
        )
        .w(px(width))
        .px(px(9.))
        .py(px(5.))
        .border_1()
        .border_color(hsla(if open { v.p.accent } else { v.p.border }))
        .bg(hsla(v.p.sidebar));
    let mut field = div().child(button);
    if open {
        let mut list = column().gap(px(1.));
        for (index, (label, selected, message)) in items.into_iter().enumerate() {
            let look = if selected {
                Look::Selected
            } else {
                Look::Quiet
            };
            list = list.child(
                v.button(
                    (name, index + 1),
                    v.line(label, 11.0, Tone::Text),
                    look,
                    Some(message),
                )
                .w_full()
                .px(px(9.))
                .py(px(5.)),
            );
        }
        let menu = v
            .panel()
            .id(gpui::SharedString::from(format!("{name}-menu")))
            .w(px(width))
            .max_h(px(240.))
            .overflow_y_scroll()
            .p(px(3.))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(list);
        field = field.child(
            deferred(
                anchored()
                    .offset(point(px(0.), px(28.)))
                    .snap_to_window_with_margin(px(4.))
                    .child(menu),
            )
            .with_priority(2),
        );
    }
    field
}

/// The release channel, the check at startup, and what is known about newer releases.
fn updates_section(v: &View, config: &Config, updates: &Updates) -> AnyElement {
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
    let mut buttons = row().gap(px(6.)).child(
        v.button(
            "updates-check",
            div().text_size(px(11.)).child("Check now"),
            Look::Quiet,
            (*updates.status() != Status::Checking).then_some(Message::UpdateCheck),
        )
        .px(px(10.))
        .py(px(4.)),
    );
    if updates.available().is_some() {
        buttons = buttons.child(
            v.button(
                "updates-update",
                div().text_size(px(11.)).child("Update…"),
                Look::Primary,
                Some(Message::UpdateAsk),
            )
            .px(px(10.))
            .py(px(4.)),
        );
    }
    section(
        v,
        "Updates",
        vec![
            option(
                v,
                "Release channel",
                "Stable follows production releases. Development also takes pre-releases such as 0.2.0-dev.1.",
                choice(
                    v,
                    "update-channel",
                    vec![channel(Channel::Stable), channel(Channel::Dev)],
                ),
            ),
            option(
                v,
                "Check when Spooky starts",
                "Asks GitHub's public release list for a newer version and sends nothing else.",
                switch(
                    v,
                    "check-updates",
                    config.check_updates,
                    Change::CheckUpdates,
                ),
            ),
            option(v, "Spooky", format!("Version {CURRENT}. {status}"), buttons),
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
    /// Whether the system asks for solid backgrounds.
    pub reduce_transparency: bool,
}

/// The page that replaces the terminal while the settings tab is selected.
pub(crate) fn view(
    v: &mut View,
    settings: &Settings,
    config: &Config,
    themes: &ThemeCatalog,
    updates: &Updates,
    session: Session,
) -> AnyElement {
    let Session {
        shortcuts_available,
        can_hide,
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
            v,
            "Size",
            "A share of the display, pinned to its top edge, or fixed pixels",
            choice(
                v,
                "size-mode",
                vec![
                    ("Percent", percent, apply(Change::Size(SizeMode::Percent))),
                    ("Pixels", !percent, apply(Change::Size(SizeMode::Pixels))),
                ],
            ),
        ),
        option(
            v,
            "Width",
            "Of the window when it opens",
            stepper(v, "width", width, apply(lower_width), apply(raise_width)),
        ),
        option(
            v,
            "Height",
            "Of the window when it opens",
            stepper(
                v,
                "height",
                height,
                apply(lower_height),
                apply(raise_height),
            ),
        ),
        option(
            v,
            "Presets",
            "Width and height as a share of the display",
            choice(
                v,
                "presets",
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
        v,
        "When another app is clicked",
        if can_hide {
            "Hiding brings the window back with the global shortcut"
        } else {
            "Hiding needs a global shortcut and a desktop that lets Spooky hide its window, which is not available here"
        },
        choice(
            v,
            "on-blur",
            vec![
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
            ],
        ),
    ));
    window.push(option(
        v,
        "Background opacity",
        if reduce_transparency {
            "macOS Reduce Transparency is on; the background stays solid"
        } else if cfg!(target_os = "macos") {
            "Subtle background blur; 100% uses a solid background"
        } else {
            "Lets the desktop show through, blurred where the desktop offers it (KDE's Blur effect); 100% is solid"
        },
        stepper(
            v,
            "opacity",
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
            v,
            "Dock icon",
            "Off runs without a Dock icon; the global shortcut is the way back",
            switch(v, "dock-icon", config.dock_icon, Change::DockIcon),
        ));
    }

    let shortcut_control: AnyElement = if shortcuts_available {
        let shown = if recording {
            "Press the new keys…".to_owned()
        } else {
            config.shortcut.clone()
        };
        let actions = if recording {
            choice(
                v,
                "shortcut-actions",
                vec![("Cancel", false, apply(Change::CancelRecording))],
            )
        } else {
            choice(
                v,
                "shortcut-actions",
                vec![
                    ("Record", false, apply(Change::RecordShortcut)),
                    ("Reset", false, apply(Change::ResetShortcut)),
                ],
            )
        };
        row()
            .gap(px(8.))
            .child(
                div()
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(px(5.))
                    .border_1()
                    .border_color(hsla(v.p.border))
                    .bg(hsla(v.p.hover))
                    .child(v.text(shown, 11.0, Tone::Text)),
            )
            .child(actions)
            .into_any_element()
    } else {
        v.text("Not available", 11.0, Tone::Muted)
            .into_any_element()
    };
    let shortcut = vec![option(
        v,
        "Global shortcut",
        if recording {
            "Hold Ctrl, Alt or Cmd with a key, or press a function key. Escape cancels"
        } else if can_hide {
            "Shows and hides the window from any application"
        } else {
            "Brings the window forward from any application; this desktop does not let Spooky hide or place it"
        },
        shortcut_control,
    )];

    let family = config.font_family.as_deref().unwrap_or(DEFAULT_FONT);
    let fonts = select(
        v,
        "font-family",
        220.0,
        family.to_owned(),
        settings.menu() == Some("font-family"),
        settings
            .fonts()
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    name == family,
                    apply(Change::FontFamily(
                        (name != DEFAULT_FONT).then(|| name.clone()),
                    )),
                )
            })
            .collect(),
    );
    let terminal = vec![
        option(
            v,
            "Font",
            "Monospace families installed on this machine; the default follows the platform",
            fonts,
        ),
        option(
            v,
            "Font size",
            "Terminal text, in points",
            stepper(
                v,
                "font-size",
                format!("{} pt", config.font_size),
                apply(Change::FontSize(config.font_size - 1.0)),
                apply(Change::FontSize(config.font_size + 1.0)),
            ),
        ),
        option(
            v,
            "Blinking cursor",
            "Off keeps every cursor solid, whatever a program asks for",
            switch(v, "cursor-blink", config.cursor_blink, Change::CursorBlink),
        ),
        option(
            v,
            "Copy on select",
            "Selecting text with the mouse copies it when you let go; Shift-drag selects in programs that use the mouse",
            switch(
                v,
                "copy-on-select",
                config.copy_on_select,
                Change::CopyOnSelect,
            ),
        ),
        option(
            v,
            "Clipboard writes from programs",
            "OSC 52: how tmux and vim over SSH copy; reading is never allowed",
            switch(v, "osc52", config.osc52 == Osc52::Write, Change::Osc52),
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
    let bar = div().w_full().flex().justify_center().py(px(10.)).child(
        row()
            .w(px(640.))
            .gap(px(16.))
            .child(v.text("Settings", 16.0, Tone::Text))
            .child(div().flex_1().child(v.text(problem, 11.0, Tone::Danger)))
            .child(
                v.button(
                    "settings-done",
                    div().text_size(px(11.)).child("Done"),
                    Look::Primary,
                    Some(Message::Settings(super::Message::Close)),
                )
                .px(px(14.))
                .py(px(5.)),
            ),
    );
    let current_theme = config
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
        });
    let theme_select = select(
        v,
        "color-theme",
        150.0,
        current_theme.map_or_else(String::new, |theme| theme.to_string()),
        settings.menu() == Some("color-theme"),
        themes
            .themes()
            .iter()
            .map(|theme| {
                let selected = current_theme.is_some_and(|current| current.id == theme.id);
                (
                    theme.to_string(),
                    selected,
                    apply(Change::Appearance(theme.id.clone())),
                )
            })
            .collect(),
    );
    let mut sections = column()
        .w(px(640.))
        .gap(px(20.))
        .child(section(v, "Window", window))
        .child(section(
            v,
            "Appearance",
            vec![option(
                v,
                "Color theme",
                format!(
                    "Choose a palette or add .toml theme files in {}",
                    Config::themes_dir().map_or_else(
                        |_| "the Spooky config directory".to_owned(),
                        |path| path.display().to_string(),
                    )
                ),
                theme_select,
            )],
        ))
        .child(section(v, "Shortcut", shortcut))
        .child(section(v, "Terminal", terminal))
        .child(updates_section(v, config, updates))
        .child(v.text(file, 10.0, Tone::Muted))
        .child(v.text(themes_path, 10.0, Tone::Muted));
    if let Some(missing_theme) = missing_theme {
        sections = sections.child(v.text(missing_theme, 10.0, Tone::Warning));
    }
    if !theme_issues.is_empty() {
        sections = sections.child(v.text(
            format!("Some theme files were skipped:\n{theme_issues}"),
            10.0,
            Tone::Danger,
        ));
    }
    let mut page = column().size_full().child(bar).child(v.separator()).child(
        div()
            .id("settings-scroll")
            .w_full()
            .flex_1()
            .overflow_y_scroll()
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .py(px(16.))
                    .child(sections),
            ),
    );
    if settings.menu().is_some() {
        page = page.child(v.dismiss_layer(
            "settings-menu-dismiss",
            Message::Settings(super::Message::Menu(None)),
        ));
    }
    page.into_any_element()
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
