//! User-authored, data-only color themes discovered beside `config.toml`.

use super::{Color, ColorPalette, TerminalColors, color_palette};
use crate::config::Appearance;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fmt, fs,
    path::{Path, PathBuf},
};

const MAX_THEME_FILES: usize = 64;
const MAX_THEME_BYTES: u64 = 16 * 1024;
const MAX_ID_BYTES: usize = 64;
const MAX_NAME_CHARS: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ThemeDefinition {
    pub id: String,
    pub name: String,
    pub colors: ColorPalette,
    pub terminal: TerminalColors,
}

impl fmt::Display for ThemeDefinition {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ThemeCatalog {
    themes: Vec<ThemeDefinition>,
    issues: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFile {
    id: String,
    name: String,
    colors: ThemeFileColors,
    /// Optional overrides of the terminal's colours, by name; the rest are derived from
    /// `colors`.
    terminal: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFileColors {
    background: String,
    text: String,
    accent: String,
    success: String,
    warning: String,
    danger: String,
}

impl ThemeCatalog {
    pub(crate) fn load(directory: &Path) -> Self {
        let mut catalog = Self::builtins();
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return catalog,
            Err(error) => {
                catalog.issue(format!("{}: {error}", directory.display()));
                return catalog;
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) if entry.path().extension().is_some_and(|ext| ext == "toml") => {
                    match entry.file_type() {
                        Ok(kind) if kind.is_file() => paths.push(entry.path()),
                        Ok(_) => {}
                        Err(error) => catalog.issue(format!("{}: {error}", entry.path().display())),
                    }
                }
                Ok(_) => {}
                Err(error) => catalog.issue(format!("{}: {error}", directory.display())),
            }
        }
        paths.sort();
        if paths.len() > MAX_THEME_FILES {
            catalog.issue(format!(
                "Only the first {MAX_THEME_FILES} theme files are loaded from {}",
                directory.display()
            ));
            paths.truncate(MAX_THEME_FILES);
        }
        for path in paths {
            catalog.load_file(&path);
        }
        catalog
    }

    pub(crate) fn themes(&self) -> &[ThemeDefinition] {
        &self.themes
    }

    pub(crate) fn get(&self, id: &str) -> Option<&ThemeDefinition> {
        self.themes.iter().find(|theme| theme.id == id)
    }

    pub(crate) fn issues(&self) -> &[String] {
        &self.issues
    }

    fn builtins() -> Self {
        let mut themes: Vec<_> = [
            ("midnight", "Midnight", Appearance::Midnight),
            ("spooky", "Spooky", Appearance::Spooky),
            ("nord", "Nord", Appearance::Nord),
            ("dracula", "Dracula", Appearance::Dracula),
        ]
        .into_iter()
        .map(|(id, name, appearance)| ThemeDefinition {
            id: id.to_owned(),
            name: name.to_owned(),
            colors: color_palette(appearance),
            terminal: match appearance {
                Appearance::Nord => TerminalColors::nord(),
                Appearance::Dracula => TerminalColors::dracula(),
                Appearance::Midnight | Appearance::Spooky => TerminalColors::default(),
            },
        })
        .collect();
        for (id, name, colors) in [
            (
                "ember",
                "Ember",
                [
                    (25, 18, 26),
                    (243, 232, 245),
                    (240, 135, 85),
                    (112, 200, 154),
                    (240, 198, 116),
                    (240, 108, 117),
                ],
            ),
            (
                "ocean",
                "Ocean",
                [
                    (16, 27, 38),
                    (213, 231, 240),
                    (112, 186, 221),
                    (128, 201, 162),
                    (232, 190, 115),
                    (233, 130, 130),
                ],
            ),
        ] {
            let [background, text, accent, success, warning, danger] =
                colors.map(|(r, g, b)| Color::from_rgb8(r, g, b));
            let colors =
                ColorPalette::from_base(background, text, accent, success, warning, danger);
            themes.push(ThemeDefinition {
                id: id.to_owned(),
                name: name.to_owned(),
                terminal: TerminalColors::derive(&colors),
                colors,
            });
        }
        Self {
            themes,
            issues: Vec::new(),
        }
    }

    fn load_file(&mut self, path: &Path) {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.issue(format!("{}: {error}", path.display()));
                return;
            }
        };
        if metadata.len() > MAX_THEME_BYTES {
            self.issue(format!(
                "{}: theme file exceeds {MAX_THEME_BYTES} bytes",
                path.display()
            ));
            return;
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                self.issue(format!("{}: {error}", path.display()));
                return;
            }
        };
        let parsed: ThemeFile = match toml::from_str(&text) {
            Ok(theme) => theme,
            Err(error) => {
                self.issue(format!("{}: {error}", path.display()));
                return;
            }
        };
        if !valid_theme_id(&parsed.id) {
            self.issue(format!(
                "{}: id must use lowercase letters, digits, and hyphens",
                path.display()
            ));
            return;
        }
        if parsed.name.trim().is_empty()
            || parsed.name.chars().count() > MAX_NAME_CHARS
            || parsed.name.chars().any(char::is_control)
        {
            self.issue(format!(
                "{}: name must be 1 to {MAX_NAME_CHARS} printable characters",
                path.display()
            ));
            return;
        }
        if self.themes.iter().any(|theme| theme.id == parsed.id) {
            self.issue(format!(
                "{}: duplicate or reserved theme id {:?}",
                path.display(),
                parsed.id
            ));
            return;
        }
        let colors = match parsed.colors.palette() {
            Ok(colors) => colors,
            Err(problem) => {
                self.issue(format!("{}: {problem}", path.display()));
                return;
            }
        };
        let terminal = match terminal_colors(&colors, parsed.terminal.as_ref()) {
            Ok(terminal) => terminal,
            Err(problem) => {
                self.issue(format!("{}: {problem}", path.display()));
                return;
            }
        };
        self.themes.push(ThemeDefinition {
            id: parsed.id,
            name: parsed.name,
            colors,
            terminal,
        });
    }

    fn issue(&mut self, issue: String) {
        if self.issues.len() < 10 {
            self.issues.push(issue);
        }
    }
}

impl ThemeFileColors {
    fn palette(self) -> Result<ColorPalette, String> {
        let parse = |name: &str, value: &str| {
            parse_hex(value).ok_or_else(|| format!("colors.{name} must be a #RRGGBB color"))
        };
        Ok(ColorPalette::from_base(
            parse("background", &self.background)?,
            parse("text", &self.text)?,
            parse("accent", &self.accent)?,
            parse("success", &self.success)?,
            parse("warning", &self.warning)?,
            parse("danger", &self.danger)?,
        ))
    }
}

/// The terminal colours of a theme file: derived from its palette, with the file's own
/// choices on top.
fn terminal_colors(
    palette: &ColorPalette,
    overrides: Option<&BTreeMap<String, String>>,
) -> Result<TerminalColors, String> {
    let mut colors = TerminalColors::derive(palette);
    for (name, value) in overrides.into_iter().flatten() {
        let rgb = parse_hex(value)
            .ok_or_else(|| format!("terminal.{name:?} must be a #RRGGBB color"))?
            .rgb8();
        if !colors.set(name, rgb) {
            return Err(format!("terminal.{name:?} is not a colour a theme may set"));
        }
    }
    Ok(colors)
}

fn parse_hex(value: &str) -> Option<Color> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let red = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let green = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let blue = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::from_rgb8(red, green, blue))
}
pub(crate) fn valid_theme_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || (index > 0 && byte == b'-')
        })
        && id.as_bytes()[0].is_ascii_lowercase()
}

pub(crate) fn themes_directory(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("themes")
}

#[cfg(test)]
mod tests {
    use super::{Color, ThemeCatalog};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU32, Ordering},
    };

    fn scratch() -> PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "spooky-themes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("theme scratch directory");
        path
    }

    #[test]
    fn discovers_custom_theme_and_keeps_builtin_themes() {
        let dir = scratch();
        fs::write(
            dir.join("custom.toml"),
            r##"id = "custom"
name = "Custom"
[colors]
background = "#19121A"
text = "#F3E8F5"
accent = "#F08755"
success = "#70C89A"
warning = "#F0C674"
danger = "#F06C75"
"##,
        )
        .expect("theme file");

        let catalog = ThemeCatalog::load(&dir);
        let custom = catalog.get("custom").expect("custom theme loaded");
        assert_eq!(custom.name, "Custom");
        assert_eq!(custom.colors.background, Color::from_rgb8(25, 18, 26));
        assert_eq!(custom.colors.accent, Color::from_rgb8(240, 135, 85));
        assert_ne!(custom.colors.sidebar, custom.colors.background);
        assert!(catalog.get("midnight").is_some());
        assert!(catalog.issues().is_empty());
        fs::remove_dir_all(dir).expect("remove scratch directory");
    }

    #[test]
    fn invalid_and_reserved_themes_are_ignored_with_diagnostics() {
        let dir = scratch();
        fs::write(dir.join("bad.toml"), "id = 'not valid'").expect("bad theme");
        fs::write(
            dir.join("reserved.toml"),
            r##"id = "dracula"
name = "Override"
[colors]
background = "#000000"
text = "#FFFFFF"
accent = "#FFFFFF"
success = "#00FF00"
warning = "#FFFF00"
danger = "#FF0000"
"##,
        )
        .expect("reserved theme");
        let catalog = ThemeCatalog::load(&dir);
        assert!(catalog.get("dracula").is_some());
        assert!(catalog.get("not valid").is_none());
        assert_eq!(catalog.issues().len(), 2);
        fs::remove_dir_all(dir).expect("remove scratch directory");
    }
    #[test]
    fn a_theme_file_may_choose_terminal_colours_and_the_rest_follow_its_palette() {
        let dir = scratch();
        fs::write(
            dir.join("mine.toml"),
            r##"id = "mine"
name = "Mine"
[colors]
background = "#19121A"
text = "#F3E8F5"
accent = "#F08755"
success = "#70C89A"
warning = "#F0C674"
danger = "#F06C75"
[terminal]
red = "#112233"
foreground = "#445566"
"##,
        )
        .expect("theme file");
        let catalog = ThemeCatalog::load(&dir);
        assert!(catalog.issues().is_empty(), "{:?}", catalog.issues());
        let terminal = catalog.get("mine").expect("theme").terminal;
        assert_eq!(
            terminal.resolve(spooky_core::ANSI_PALETTE[1]),
            [0x11, 0x22, 0x33]
        );
        assert_eq!(
            terminal.resolve(spooky_core::DEFAULT_FOREGROUND),
            [0x44, 0x55, 0x66]
        );
        // Not chosen: derived from the palette's success colour.
        assert_eq!(
            terminal.resolve(spooky_core::ANSI_PALETTE[2]),
            [0x70, 0xc8, 0x9a]
        );
        fs::remove_dir_all(dir).expect("remove scratch directory");
    }

    #[test]
    fn a_bad_terminal_colour_rejects_the_whole_theme() {
        let dir = scratch();
        let theme = |id: &str, line: &str| {
            format!(
                "id = \"{id}\"\nname = \"X\"\n[colors]\nbackground = \"#000001\"\ntext = \"#FFFFFE\"\n\
                 accent = \"#FFFFFE\"\nsuccess = \"#00FF00\"\nwarning = \"#FFFF00\"\ndanger = \"#FF0000\"\n\
                 [terminal]\n{line}\n"
            )
        };
        fs::write(dir.join("a.toml"), theme("a", "red = \"red\"")).expect("theme");
        fs::write(dir.join("b.toml"), theme("b", "black = \"#000000\"")).expect("theme");
        fs::write(dir.join("c.toml"), theme("c", "purple = \"#000000\"")).expect("theme");
        let catalog = ThemeCatalog::load(&dir);
        for id in ["a", "b", "c"] {
            assert!(catalog.get(id).is_none(), "{id}");
        }
        assert_eq!(catalog.issues().len(), 3, "{:?}", catalog.issues());
        fs::remove_dir_all(dir).expect("remove scratch directory");
    }

    #[test]
    fn the_nord_and_dracula_themes_carry_their_own_schemes_and_midnight_none() {
        let catalog = ThemeCatalog::load(&scratch());
        let red = spooky_core::ANSI_PALETTE[1];
        assert_eq!(
            catalog.get("nord").expect("nord").terminal.resolve(red),
            [0xbf, 0x61, 0x6a]
        );
        assert_eq!(
            catalog
                .get("dracula")
                .expect("dracula")
                .terminal
                .resolve(red),
            [0xff, 0x55, 0x55]
        );
        assert_eq!(
            catalog
                .get("midnight")
                .expect("midnight")
                .terminal
                .resolve(red),
            red
        );
    }
}
