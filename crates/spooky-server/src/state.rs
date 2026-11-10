use crate::pane::{Pane, Size};
use anyhow::{Context, Result, bail};
use spooky_core::{Layout, Workspace};
use std::{collections::BTreeMap, env, path::PathBuf, sync::Arc};

pub(crate) struct State {
    pub(crate) next_id: u64,
    pub(crate) workspaces: Vec<Workspace>,
    pub(crate) panes: BTreeMap<u64, Arc<Pane>>,
}

/// The size of the largest pane, which is the window-sized one when nothing
/// is split, or `fallback` when no pane exists.
pub(crate) fn largest_pane(state: &State, fallback: Size) -> Size {
    state
        .panes
        .values()
        .map(|pane| pane.size())
        .max_by_key(|size| u32::from(size.cols) * u32::from(size.rows))
        .unwrap_or(fallback)
}

pub(crate) fn replace_leaf(layout: &mut Layout, pane: u64, replacement: Layout) -> Result<()> {
    match layout {
        Layout::Pane(id) if *id == pane => {
            *layout = replacement;
            Ok(())
        }
        Layout::Split { first, .. } if first.contains(pane) => {
            replace_leaf(first, pane, replacement)
        }
        Layout::Split { second, .. } => replace_leaf(second, pane, replacement),
        _ => bail!("pane not in layout"),
    }
}
pub(crate) fn remove_leaf(layout: &mut Layout, pane: u64) -> Result<()> {
    match layout {
        Layout::Split { first, second, .. } if matches!(**first, Layout::Pane(id) if id == pane) => {
            *layout = (**second).clone();
            Ok(())
        }
        Layout::Split { first, second, .. } if matches!(**second, Layout::Pane(id) if id == pane) =>
        {
            *layout = (**first).clone();
            Ok(())
        }
        Layout::Split { first, .. } if first.contains(pane) => remove_leaf(first, pane),
        Layout::Split { second, .. } => remove_leaf(second, pane),
        _ => bail!("pane not in split"),
    }
}
pub(crate) fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        bail!("name must be nonempty, at most 256 bytes, and contain no control characters");
    }
    Ok(())
}
pub(crate) fn validate_size(cols: u16, rows: u16) -> Result<()> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        bail!("terminal size out of bounds");
    }
    Ok(())
}
pub(crate) fn validated_cwd(cwd: Option<&str>) -> Result<PathBuf> {
    let path = match cwd {
        Some(s) if !s.is_empty() => PathBuf::from(s),
        Some(_) => bail!("working directory is empty"),
        None => env::current_dir()?,
    };
    let path = path.canonicalize().context("invalid working directory")?;
    if !path.is_dir() {
        bail!("working directory is not a directory");
    }
    if path.to_str().is_none() {
        bail!("working directory is not valid UTF-8");
    }
    Ok(path)
}

/// The directory a restored workspace's shells start in: the saved one if it
/// still exists, otherwise the home directory.
pub(crate) fn restore_cwd(saved: &str) -> PathBuf {
    validated_cwd(Some(saved))
        .or_else(|_| validated_cwd(env::var("HOME").ok().as_deref()))
        .or_else(|_| validated_cwd(None))
        .unwrap_or_else(|_| PathBuf::from("/"))
}
