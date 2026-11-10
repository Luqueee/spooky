use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    env,
    fs::{self},
    io::{self},
    path::PathBuf,
};

/// Per-machine server settings, read from `spooky/server.toml` in the
/// configuration directory.
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Settings {
    /// Scrollback lines kept per pane.
    pub(crate) scrollback_lines: usize,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            scrollback_lines: 10_000,
        }
    }
}
impl Settings {
    const MAX_SCROLLBACK: usize = 1_000_000;

    pub(crate) fn load() -> Result<Self> {
        let Some(base) = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        else {
            return Ok(Self::default());
        };
        let path = base.join("spooky/server.toml");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        let settings: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        ensure!(
            settings.scrollback_lines <= Self::MAX_SCROLLBACK,
            "{}: scrollback_lines must be at most {}",
            path.display(),
            Self::MAX_SCROLLBACK
        );
        Ok(settings)
    }
}
