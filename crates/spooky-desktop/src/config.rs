//! The user's configuration: what `config.toml` may say, how it is checked, and how
//! the settings page writes it back.

use crate::platform::kde::Dim;
use iced::Size;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// What programs in a terminal may do to the clipboard with OSC 52.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Osc52 {
    /// Programs may set the clipboard. Reading it is never allowed.
    Write,
    /// Clipboard requests from programs are ignored.
    Off,
}

/// How the window size is given.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SizeMode {
    /// A share of the display, anchored to its top edge.
    Percent,
    /// Fixed logical pixels.
    Pixels,
}

/// What the window does when another application takes the keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OnBlur {
    /// Stays where it is, behind whatever was clicked.
    None,
    /// Hides itself; the global shortcut brings it back.
    Hide,
    /// Stays above every other window.
    Top,
}

/// Smallest window the application lays out well.
pub(crate) const MIN_WINDOW: Size = Size::new(720.0, 440.0);
/// Window size used until the display is known, when the size is a percentage.
const PLACEHOLDER_WINDOW: Size = Size::new(1100.0, 600.0);
const DEFAULT_WIDTH_PX: f32 = 1440.0;
const DEFAULT_HEIGHT_PX: f32 = 900.0;

pub(crate) const FONT_SIZE: std::ops::RangeInclusive<f32> = 8.0..=48.0;
pub(crate) const WIDTH_PX: std::ops::RangeInclusive<f32> = 400.0..=4096.0;
pub(crate) const HEIGHT_PX: std::ops::RangeInclusive<f32> = 200.0..=2160.0;
pub(crate) const WIDTH_PERCENT: std::ops::RangeInclusive<f32> = 30.0..=100.0;
pub(crate) const HEIGHT_PERCENT: std::ops::RangeInclusive<f32> = 20.0..=100.0;

/// Everything `config.toml` can set. Unknown keys are an error, so a typo is never
/// silently ignored.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub font_size: f32,
    /// How `width_percent`/`height_percent` or `width`/`height` give the window
    /// size. When absent: pixels if the file sets `width` or `height` (as older
    /// files did), otherwise percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<SizeMode>,
    /// Window width in pixels, used when the size is in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// Window height in pixels, used when the size is in pixels.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f32>,
    pub width_percent: f32,
    pub height_percent: f32,
    pub shortcut: String,
    /// Keep a Dock icon on macOS. By default Spooky runs without one, like other
    /// dropdown terminals, whenever the global shortcut is available to bring it back.
    pub dock_icon: bool,
    /// Whether a program's OSC 52 request may set the clipboard: `"write"` (the
    /// default; the footer says when it happens) or `"off"`.
    pub osc52: Osc52,
    /// Whether a blinking cursor blinks. Off keeps every cursor solid.
    pub cursor_blink: bool,
    /// What the window does when it loses the keyboard focus.
    pub on_blur: OnBlur,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            size: None,
            width: None,
            height: None,
            width_percent: 100.0,
            height_percent: 50.0,
            shortcut: "Ctrl+Shift+Space".into(),
            dock_icon: false,
            osc52: Osc52::Write,
            cursor_blink: true,
            on_blur: OnBlur::None,
        }
    }
}

impl Config {
    /// `$XDG_CONFIG_HOME/spooky/config.toml`, or `~/.config/spooky/config.toml`.
    pub(crate) fn path() -> anyhow::Result<PathBuf> {
        let home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .ok_or_else(|| anyhow::anyhow!("Cannot determine configuration directory"))?;
        Ok(home.join("spooky/config.toml"))
    }

    /// The configuration file, or the defaults when there is none.
    pub(crate) fn load() -> anyhow::Result<Self> {
        Self::load_from(&Self::path()?)
    }

    pub(crate) fn load_from(path: &Path) -> anyhow::Result<Self> {
        let config: Self = match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(anyhow::anyhow!("{}: {error}", path.display())),
        };
        config
            .validate()
            .map_err(|problem| anyhow::anyhow!("{}: {problem}", path.display()))?;
        Ok(config)
    }

    /// Why the values are unusable, if they are.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let check = |name: &str, value: f32, range: &std::ops::RangeInclusive<f32>| {
            if range.contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "{name} must be between {} and {}",
                    range.start(),
                    range.end()
                ))
            }
        };
        check("font_size", self.font_size, &FONT_SIZE)?;
        check("width", self.width_px(), &WIDTH_PX)?;
        check("height", self.height_px(), &HEIGHT_PX)?;
        check("width_percent", self.width_percent, &WIDTH_PERCENT)?;
        check("height_percent", self.height_percent, &HEIGHT_PERCENT)
    }

    /// Writes the file atomically: a reader sees the old or the new one, never half.
    /// Comments in an existing file are not preserved.
    pub(crate) fn save(&self) -> anyhow::Result<()> {
        self.save_to(&Self::path()?)
    }

    pub(crate) fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        use std::io::Write;
        let text = toml::to_string(self)?;
        let dir = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("{} has no directory", path.display()))?;
        fs::create_dir_all(dir)?;
        let temporary = path.with_extension("toml.tmp");
        let written = fs::File::create(&temporary).and_then(|mut file| {
            file.write_all(b"# Written by Spooky's settings page; comments are not kept.\n")?;
            file.write_all(text.as_bytes())?;
            file.sync_all()
        });
        if let Err(error) = written.and_then(|()| fs::rename(&temporary, path)) {
            let _ = fs::remove_file(&temporary);
            return Err(anyhow::anyhow!("{}: {error}", path.display()));
        }
        Ok(())
    }

    pub(crate) fn width_px(&self) -> f32 {
        self.width.unwrap_or(DEFAULT_WIDTH_PX)
    }

    pub(crate) fn height_px(&self) -> f32 {
        self.height.unwrap_or(DEFAULT_HEIGHT_PX)
    }

    /// How the size is given.
    pub(crate) fn size_mode(&self) -> SizeMode {
        self.size
            .unwrap_or(if self.width.is_some() || self.height.is_some() {
                SizeMode::Pixels
            } else {
                SizeMode::Percent
            })
    }

    /// The size to create the window with, before the display is known.
    pub(crate) fn initial_size(&self) -> Size {
        match self.size_mode() {
            SizeMode::Pixels => Size::new(self.width_px(), self.height_px()),
            SizeMode::Percent => PLACEHOLDER_WINDOW,
        }
    }

    /// The configured window size as KWin resolves it against its own display.
    pub(crate) fn kde_dims(&self) -> (Dim, Dim) {
        match self.size_mode() {
            SizeMode::Pixels => (
                Dim::Px(self.width_px().round() as u32),
                Dim::Px(self.height_px().round() as u32),
            ),
            SizeMode::Percent => (
                Dim::Percent {
                    percent: self.width_percent,
                    min: MIN_WINDOW.width as u32,
                },
                Dim::Percent {
                    percent: self.height_percent,
                    min: MIN_WINDOW.height as u32,
                },
            ),
        }
    }

    /// The window size on a display of `monitor` size.
    pub(crate) fn window_size(&self, monitor: Size) -> Size {
        match self.size_mode() {
            SizeMode::Pixels => Size::new(self.width_px(), self.height_px()),
            SizeMode::Percent => Size::new(
                (monitor.width * self.width_percent / 100.0)
                    .round()
                    .max(MIN_WINDOW.width.min(monitor.width)),
                (monitor.height * self.height_percent / 100.0)
                    .round()
                    .max(MIN_WINDOW.height.min(monitor.height)),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, OnBlur, SizeMode};
    use iced::Size;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn parse(text: &str) -> Result<Config, toml::de::Error> {
        toml::from_str(text)
    }

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "spooky-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn a_file_that_sets_a_pixel_size_keeps_meaning_pixels() {
        assert_eq!(parse("").expect("empty").size_mode(), SizeMode::Percent);
        let old = parse("width = 1100.0\nheight = 700.0").expect("old style");
        assert_eq!(old.size_mode(), SizeMode::Pixels);
        assert_eq!(
            old.window_size(Size::new(3000.0, 2000.0)),
            Size::new(1100.0, 700.0)
        );
        let chosen = parse("size = \"percent\"\nwidth = 1100.0").expect("explicit");
        assert_eq!(chosen.size_mode(), SizeMode::Percent);
    }

    #[test]
    fn a_percentage_size_follows_the_display_but_never_drops_below_the_minimum() {
        let config = Config::default();
        assert_eq!(
            config.window_size(Size::new(1512.0, 982.0)),
            Size::new(1512.0, 491.0)
        );
        assert_eq!(
            config.window_size(Size::new(800.0, 500.0)),
            Size::new(800.0, 440.0),
            "the width cannot exceed the display, the height keeps its floor"
        );
    }

    #[test]
    fn mistakes_in_the_file_are_errors_not_silent_defaults() {
        assert!(parse("shortcutt = \"F10\"").is_err(), "unknown key");
        assert!(parse("osc52 = \"read\"").is_err(), "unknown value");
        for bad in [
            "font_size = 4.0",
            "width = 100.0",
            "height_percent = 5.0",
            "width_percent = 140.0",
        ] {
            let config = parse(bad).expect("parses");
            assert!(config.validate().is_err(), "{bad}");
        }
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn a_saved_file_loads_back_unchanged_and_names_only_what_was_set() {
        let dir = scratch();
        let path = dir.join("spooky/config.toml");
        let mut config = Config {
            font_size: 14.0,
            shortcut: "F10".into(),
            on_blur: OnBlur::Hide,
            cursor_blink: false,
            size: Some(SizeMode::Percent),
            ..Config::default()
        };
        config.save_to(&path).expect("saved");
        assert_eq!(Config::load_from(&path).expect("loaded"), config);
        let text = std::fs::read_to_string(&path).expect("file");
        assert!(!text.contains("width ="), "unset pixel width stays out");
        config.width = Some(1200.0);
        config.save_to(&path).expect("saved again");
        assert_eq!(
            Config::load_from(&path).expect("loaded").width,
            Some(1200.0)
        );
        assert!(
            !dir.join("spooky")
                .read_dir()
                .expect("dir")
                .any(|entry| entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")),
            "no temporary file is left behind"
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_missing_file_means_the_defaults() {
        let dir = scratch();
        assert_eq!(
            Config::load_from(&dir.join("nothing.toml")).expect("defaults"),
            Config::default()
        );
        std::fs::remove_dir_all(dir).expect("cleanup");
    }
}
