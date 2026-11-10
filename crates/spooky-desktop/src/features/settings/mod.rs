//! The settings feature: the page's state and how a change is checked, saved and
//! turned into effects. It does not touch the window, the grids or the global
//! shortcut itself: it reports [`Effect`]s and talks to the shortcut through
//! [`Shortcuts`], so it can be tested without a display.

mod page;

pub(crate) use page::view;

use crate::{
    config::{self, Config, OnBlur, Osc52, SizeMode},
    shortcuts,
};
use iced::keyboard;

/// What the rest of the application is told by this feature.
#[derive(Clone, Debug)]
pub(crate) enum Message {
    /// Show the page.
    Open,
    /// Hide the page.
    Close,
    /// A control on the page changed something.
    Change(Change),
}

/// A change made on the settings page.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Change {
    FontSize(f32),
    Size(SizeMode),
    WidthPercent(f32),
    HeightPercent(f32),
    WidthPixels(f32),
    HeightPixels(f32),
    /// Width and height as percentages of the display.
    Preset(f32, f32),
    OnBlur(OnBlur),
    DockIcon(bool),
    CursorBlink(bool),
    Osc52(bool),
    RecordShortcut,
    CancelRecording,
    ResetShortcut,
}

/// Something the application must do because a setting changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// The font changed: every pane's grid must be recomputed.
    Refit,
    /// The window size setting changed: apply it to the window.
    Resize,
    /// What the window does when it loses the focus changed.
    OnBlur(OnBlur),
    /// Show (`true`) or hide the Dock icon.
    DockIcon(bool),
}

/// The global shortcut, as the settings page needs it.
pub(crate) trait Shortcuts {
    /// Whether the session has a global shortcut at all.
    fn available(&self) -> bool;
    /// Binds the shortcut to `text`; on refusal the previous one keeps working.
    fn rebind(&mut self, text: &str) -> Result<(), String>;
}

/// The settings page: whether it is open and what it last had to say.
#[derive(Default)]
pub(crate) struct Settings {
    page: Option<Page>,
}

#[derive(Default)]
struct Page {
    /// Waiting for the keys of a new global shortcut.
    recording: bool,
    /// Why the last change was refused or could not be saved.
    problem: Option<String>,
}

impl Settings {
    /// Whether the page is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.page.is_some()
    }

    /// Whether the next key press is the new shortcut.
    pub(crate) fn recording(&self) -> bool {
        self.page.as_ref().is_some_and(|page| page.recording)
    }

    /// Why the last change was refused or could not be saved.
    pub(crate) fn problem(&self) -> Option<&str> {
        self.page.as_ref()?.problem.as_deref()
    }

    /// Shows the page.
    pub(crate) fn open(&mut self) {
        self.page.get_or_insert_with(Page::default);
    }

    /// Hides the page; whether it was showing.
    pub(crate) fn close(&mut self) -> bool {
        self.page.take().is_some()
    }

    fn report(&mut self, problem: Option<String>) {
        if let Some(page) = self.page.as_mut() {
            page.problem = problem;
        }
    }

    fn set_recording(&mut self, recording: bool) {
        if let Some(page) = self.page.as_mut() {
            page.recording = recording;
        }
    }

    /// Applies a change from the page: checks it, saves the file, and returns what the
    /// application must update. A refused change leaves `config` as it was.
    pub(crate) fn update(
        &mut self,
        change: Change,
        config: &mut Config,
        shortcuts: &mut impl Shortcuts,
    ) -> Vec<Effect> {
        let mut next = config.clone();
        let clamp = |value: f32, range: &std::ops::RangeInclusive<f32>| {
            value.clamp(*range.start(), *range.end())
        };
        match change {
            Change::FontSize(value) => next.font_size = clamp(value, &config::FONT_SIZE),
            Change::Size(mode) => next.size = Some(mode),
            Change::WidthPercent(value) => {
                next.size = Some(SizeMode::Percent);
                next.width_percent = clamp(value, &config::WIDTH_PERCENT);
            }
            Change::HeightPercent(value) => {
                next.size = Some(SizeMode::Percent);
                next.height_percent = clamp(value, &config::HEIGHT_PERCENT);
            }
            Change::WidthPixels(value) => {
                next.size = Some(SizeMode::Pixels);
                next.width = Some(clamp(value, &config::WIDTH_PX));
            }
            Change::HeightPixels(value) => {
                next.size = Some(SizeMode::Pixels);
                next.height = Some(clamp(value, &config::HEIGHT_PX));
            }
            Change::Preset(width, height) => {
                next.size = Some(SizeMode::Percent);
                next.width_percent = width;
                next.height_percent = height;
            }
            Change::OnBlur(on_blur) => {
                if on_blur == OnBlur::Hide && !shortcuts.available() {
                    self.report(Some(
                        "Hiding needs the global shortcut, which is not available here".into(),
                    ));
                    return Vec::new();
                }
                next.on_blur = on_blur;
            }
            Change::DockIcon(show) => next.dock_icon = show,
            Change::CursorBlink(on) => next.cursor_blink = on,
            Change::Osc52(on) => next.osc52 = if on { Osc52::Write } else { Osc52::Off },
            Change::RecordShortcut => {
                if shortcuts.available() {
                    self.set_recording(true);
                    self.report(None);
                } else {
                    self.report(Some(
                        "The global shortcut is not available in this session".into(),
                    ));
                }
                return Vec::new();
            }
            Change::CancelRecording => {
                self.set_recording(false);
                return Vec::new();
            }
            Change::ResetShortcut => {
                return self.rebind(Config::default().shortcut, config, shortcuts);
            }
        }
        self.apply(next, config, shortcuts.available())
    }

    /// Takes a key press as the new global shortcut while the page records one.
    pub(crate) fn record(
        &mut self,
        key: &keyboard::Key,
        physical: keyboard::key::Physical,
        mods: keyboard::Modifiers,
        config: &mut Config,
        shortcuts: &mut impl Shortcuts,
    ) -> Vec<Effect> {
        if *key == keyboard::Key::Named(keyboard::key::Named::Escape) {
            self.set_recording(false);
            return Vec::new();
        }
        match shortcuts::hotkey_text(key, physical, mods) {
            // Only modifiers are held so far.
            None => Vec::new(),
            Some(Err(problem)) => {
                self.report(Some(problem.to_owned()));
                Vec::new()
            }
            Some(Ok(text)) => {
                self.set_recording(false);
                self.rebind(text, config, shortcuts)
            }
        }
    }

    /// Binds the global shortcut to `text` and saves it.
    fn rebind(
        &mut self,
        text: String,
        config: &mut Config,
        shortcuts: &mut impl Shortcuts,
    ) -> Vec<Effect> {
        match shortcuts.rebind(&text) {
            Ok(()) => {
                let mut next = config.clone();
                next.shortcut = text;
                self.apply(next, config, shortcuts.available())
            }
            Err(problem) => {
                self.report(Some(problem));
                Vec::new()
            }
        }
    }

    /// Checks and saves `next`, making it the configuration.
    fn apply(&mut self, next: Config, config: &mut Config, has_shortcut: bool) -> Vec<Effect> {
        if let Err(problem) = next.validate() {
            self.report(Some(problem));
            return Vec::new();
        }
        let previous = std::mem::replace(config, next);
        let saved = config.save();
        self.report(saved.err().map(|error| format!("Not saved: {error:#}")));
        effects(&previous, config, has_shortcut)
    }
}

/// What differs between two configurations that something must be told about.
fn effects(previous: &Config, next: &Config, has_shortcut: bool) -> Vec<Effect> {
    let size = |config: &Config| {
        (
            config.size_mode(),
            config.width_px(),
            config.height_px(),
            config.width_percent,
            config.height_percent,
        )
    };
    let mut effects = Vec::new();
    if previous.font_size != next.font_size {
        effects.push(Effect::Refit);
    }
    if size(previous) != size(next) {
        effects.push(Effect::Resize);
    }
    if previous.on_blur != next.on_blur {
        effects.push(Effect::OnBlur(next.on_blur));
    }
    if previous.dock_icon != next.dock_icon {
        // Without a global shortcut nothing could bring a buried window back.
        effects.push(Effect::DockIcon(next.dock_icon || !has_shortcut));
    }
    effects
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::keyboard::{
        Key,
        key::{Code, Named, Physical},
    };

    /// A shortcut that works, or refuses everything, and remembers what it was asked.
    struct Fake {
        available: bool,
        refuses: bool,
        asked: Vec<String>,
    }

    impl Fake {
        fn new(available: bool, refuses: bool) -> Self {
            Self {
                available,
                refuses,
                asked: Vec::new(),
            }
        }
    }

    impl Shortcuts for Fake {
        fn available(&self) -> bool {
            self.available
        }
        fn rebind(&mut self, text: &str) -> Result<(), String> {
            self.asked.push(text.to_owned());
            if self.refuses {
                Err(format!("Cannot use {text}"))
            } else {
                Ok(())
            }
        }
    }

    fn open() -> Settings {
        let mut settings = Settings::default();
        settings.open();
        settings
    }

    #[test]
    fn only_what_a_change_touches_is_reported() {
        let base = Config::default();
        let with = |change: &dyn Fn(&mut Config)| {
            let mut next = base.clone();
            change(&mut next);
            effects(&base, &next, true)
        };
        assert!(effects(&base, &base, true).is_empty());
        assert_eq!(with(&|c| c.font_size = 14.0), [Effect::Refit]);
        assert_eq!(with(&|c| c.height_percent = 60.0), [Effect::Resize]);
        assert_eq!(
            with(&|c| c.size = Some(SizeMode::Pixels)),
            [Effect::Resize],
            "the same numbers in another unit are another size"
        );
        assert_eq!(
            with(&|c| c.on_blur = OnBlur::Top),
            [Effect::OnBlur(OnBlur::Top)]
        );
        assert_eq!(with(&|c| c.cursor_blink = false), [], "read at draw time");
    }

    #[test]
    fn the_dock_icon_stays_when_nothing_else_could_bring_the_window_back() {
        let base = Config::default();
        let mut next = base.clone();
        next.dock_icon = false;
        let mut shown = base.clone();
        shown.dock_icon = true;
        assert_eq!(
            effects(&shown, &next, true),
            [Effect::DockIcon(false)],
            "with a global shortcut the icon may go"
        );
        assert_eq!(
            effects(&shown, &next, false),
            [Effect::DockIcon(true)],
            "without one it must stay"
        );
    }

    #[test]
    fn hiding_is_refused_without_a_global_shortcut_and_nothing_changes() {
        let mut settings = open();
        let mut config = Config::default();
        let effects = settings.update(
            Change::OnBlur(OnBlur::Hide),
            &mut config,
            &mut Fake::new(false, false),
        );
        assert!(effects.is_empty());
        assert_eq!(config, Config::default());
        assert!(settings.problem().is_some());
    }

    #[test]
    fn recording_needs_a_shortcut_to_record() {
        let mut settings = open();
        let mut config = Config::default();
        settings.update(
            Change::RecordShortcut,
            &mut config,
            &mut Fake::new(false, false),
        );
        assert!(!settings.recording());
        assert!(settings.problem().is_some());
        settings.update(
            Change::RecordShortcut,
            &mut config,
            &mut Fake::new(true, false),
        );
        assert!(settings.recording());
        assert!(settings.problem().is_none(), "starting clears the message");
        settings.update(
            Change::CancelRecording,
            &mut config,
            &mut Fake::new(true, false),
        );
        assert!(!settings.recording());
    }

    #[test]
    fn a_refused_shortcut_keeps_the_configured_one_and_says_why() {
        let mut settings = open();
        let mut config = Config::default();
        let mut shortcuts = Fake::new(true, true);
        settings.update(Change::RecordShortcut, &mut config, &mut shortcuts);
        let press = |settings: &mut Settings, config: &mut Config, shortcuts: &mut Fake| {
            settings.record(
                &Key::Named(Named::F9),
                Physical::Code(Code::F9),
                keyboard::Modifiers::empty(),
                config,
                shortcuts,
            )
        };
        assert!(press(&mut settings, &mut config, &mut shortcuts).is_empty());
        assert_eq!(shortcuts.asked, ["F9"]);
        assert_eq!(config.shortcut, Config::default().shortcut);
        assert_eq!(settings.problem(), Some("Cannot use F9"));
        assert!(!settings.recording(), "the key was taken, even if refused");
    }

    #[test]
    fn while_recording_a_bad_key_explains_and_escape_gives_up() {
        let mut settings = open();
        let mut config = Config::default();
        let mut shortcuts = Fake::new(true, false);
        settings.update(Change::RecordShortcut, &mut config, &mut shortcuts);
        let letter = Key::Character("a".into());
        // Only modifiers held: nothing to say yet.
        settings.record(
            &Key::Named(Named::Control),
            Physical::Code(Code::ControlLeft),
            keyboard::Modifiers::CTRL,
            &mut config,
            &mut shortcuts,
        );
        assert!(settings.problem().is_none());
        // A bare letter would take that key from every application.
        settings.record(
            &letter,
            Physical::Code(Code::KeyA),
            keyboard::Modifiers::empty(),
            &mut config,
            &mut shortcuts,
        );
        assert!(settings.problem().is_some());
        assert!(settings.recording(), "still waiting for a usable key");
        assert!(shortcuts.asked.is_empty());
        settings.record(
            &Key::Named(Named::Escape),
            Physical::Code(Code::Escape),
            keyboard::Modifiers::empty(),
            &mut config,
            &mut shortcuts,
        );
        assert!(!settings.recording());
        assert!(shortcuts.asked.is_empty());
    }
}
