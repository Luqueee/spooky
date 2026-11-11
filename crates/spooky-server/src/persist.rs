//! Saved workspaces.
//!
//! The layout of every workspace (names, directories, tabs, splits and where the
//! dividers sit) is written beside the server log so that a restarted server can
//! rebuild it. Shells are new processes: running programs and scrollback are not
//! saved.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use spooky_core::{Layout, MIN_SPLIT_SHARE, Workspace};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const NAME: &str = "workspaces.json";
const FORMAT: u32 = 1;
/// A saved file larger than this is treated as damaged rather than parsed.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// An agent pane's identity and what is needed to resume it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedAgent {
    /// The pane identity, a leaf of a saved layout.
    pub pane: u64,
    /// The agent program, as named when it was created.
    pub backend: String,
    /// The directory it ran in.
    pub cwd: String,
    /// The program's own session to resume, when it has one.
    pub session: Option<String>,
    /// The agent is shown over a terminal of the same pane, which comes back when it closes.
    #[serde(default)]
    pub covers: bool,
}

/// What a server saves and the next one restores.
#[derive(Debug, Serialize, Deserialize)]
pub struct Saved {
    format: u32,
    /// First identity the restored server may hand out.
    pub next_id: u64,
    /// Grid size new shells start at, so a restored prompt is not drawn at a
    /// default size and then reflowed.
    cols: u16,
    rows: u16,
    /// Workspaces in display order.
    pub workspaces: Vec<Workspace>,
    /// The panes of those workspaces that are agents rather than shells.
    #[serde(default)]
    pub agents: Vec<SavedAgent>,
}

impl Saved {
    pub fn new(next_id: u64, workspaces: Vec<Workspace>) -> Self {
        Self {
            format: FORMAT,
            next_id,
            cols: 80,
            rows: 24,
            workspaces,
            agents: Vec::new(),
        }
    }

    /// Records the grid size restored shells should start at.
    pub fn sized(mut self, cols: u16, rows: u16) -> Self {
        (self.cols, self.rows) = (cols, rows);
        self
    }

    /// Records which panes are agents and how to resume them.
    pub fn with_agents(mut self, agents: Vec<SavedAgent>) -> Self {
        self.agents = agents;
        self
    }

    /// The saved grid size, or 80 by 24 when it is out of range.
    pub fn size(&self) -> (u16, u16) {
        if (2..=500).contains(&self.cols) && (1..=300).contains(&self.rows) {
            (self.cols, self.rows)
        } else {
            (80, 24)
        }
    }

    /// Rejects state that could not have been saved by a server: duplicate or
    /// unissued identities, tabs without panes, and dividers outside their range.
    fn validate(&mut self) -> Result<()> {
        ensure!(
            self.format == FORMAT,
            "unsupported format {} (expected {FORMAT})",
            self.format
        );
        let mut seen = HashSet::new();
        for workspace in &mut self.workspaces {
            claim(&mut seen, workspace.id, self.next_id)?;
            ensure!(!workspace.tabs.is_empty(), "workspace without tabs");
            for tab in &mut workspace.tabs {
                claim(&mut seen, tab.id, self.next_id)?;
                claim_layout(&mut seen, &mut tab.layout, self.next_id)?;
            }
        }
        let mut agents = HashSet::new();
        for agent in &self.agents {
            ensure!(
                seen.contains(&agent.pane),
                "agent {} is not in any layout",
                agent.pane
            );
            ensure!(
                agents.insert(agent.pane),
                "agent {} appears twice",
                agent.pane
            );
        }
        Ok(())
    }
}

fn claim(seen: &mut HashSet<u64>, id: u64, next_id: u64) -> Result<()> {
    ensure!(id != 0 && id < next_id, "identity {id} was never issued");
    ensure!(seen.insert(id), "identity {id} appears twice");
    Ok(())
}

fn claim_layout(seen: &mut HashSet<u64>, layout: &mut Layout, next_id: u64) -> Result<()> {
    match layout {
        Layout::Pane(id) => claim(seen, *id, next_id),
        Layout::Split {
            id,
            ratio,
            first,
            second,
            ..
        } => {
            claim(seen, *id, next_id)?;
            *ratio = (*ratio).clamp(MIN_SPLIT_SHARE, 1000 - MIN_SPLIT_SHARE);
            claim_layout(seen, first, next_id)?;
            claim_layout(seen, second, next_id)
        }
    }
}

/// Writes the saved state atomically with owner-only permissions.
pub fn store(dir: &Path, saved: &Saved) -> Result<()> {
    let target = dir.join(NAME);
    let temporary = dir.join(format!("{NAME}.tmp"));
    let bytes = serde_json::to_vec(saved).context("encoding saved workspaces")?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, &target).with_context(|| format!("replacing {}", target.display()))
}

/// Reads the saved state, if any. A file that cannot be trusted is moved aside
/// as `workspaces.json.corrupt` (so it can be inspected) and reported.
pub fn load(dir: &Path) -> Result<Option<Saved>> {
    let path = dir.join(NAME);
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("opening {}", path.display())),
    };
    match parse(&mut file) {
        Ok(saved) => Ok(Some(saved)),
        Err(error) => {
            let aside = dir.join(format!("{NAME}.corrupt"));
            let moved = fs::rename(&path, &aside).is_ok();
            bail!(
                "saved workspaces unusable ({error:#}); {}",
                if moved {
                    format!("kept as {}", aside.display())
                } else {
                    "could not be moved aside".into()
                }
            )
        }
    }
}

fn parse(file: &mut File) -> Result<Saved> {
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "not a regular file");
    ensure!(metadata.len() <= MAX_BYTES, "larger than {MAX_BYTES} bytes");
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    let mut saved: Saved = serde_json::from_slice(&bytes)?;
    saved.validate()?;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::{NAME, Saved, load, store};
    use spooky_core::{Axis, Layout, Tab, Workspace};
    use std::{
        fs,
        sync::atomic::{AtomicU32, Ordering},
    };

    fn scratch() -> std::path::PathBuf {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "spooky-persist-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    fn workspace(id: u64, tab: u64, layout: Layout) -> Workspace {
        Workspace {
            id,
            name: "Work".into(),
            cwd: "/tmp".into(),
            tabs: vec![Tab {
                id: tab,
                title: "Shell".into(),
                renamed: false,
                layout,
            }],
        }
    }

    fn split(id: u64, ratio: u16, first: u64, second: u64) -> Layout {
        Layout::Split {
            id,
            axis: Axis::Vertical,
            ratio,
            first: Box::new(Layout::Pane(first)),
            second: Box::new(Layout::Pane(second)),
        }
    }

    #[test]
    fn saved_workspaces_come_back_with_their_layout_and_identities() {
        let dir = scratch();
        let saved = Saved::new(9, vec![workspace(1, 2, split(5, 650, 3, 4))]);
        store(&dir, &saved).expect("store");
        let restored = load(&dir).expect("load").expect("saved state exists");
        assert_eq!(restored.next_id, 9);
        let Layout::Split { id, ratio, .. } = &restored.workspaces[0].tabs[0].layout else {
            panic!("the split was lost");
        };
        assert_eq!((*id, *ratio), (5, 650));
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn nothing_saved_is_not_an_error() {
        let dir = scratch();
        assert!(load(&dir).expect("load").is_none());
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_damaged_file_is_set_aside_and_never_half_restored() {
        let dir = scratch();
        for damaged in [
            "{not json".to_string(),
            // Two panes claiming one identity.
            serde_json::to_string(&Saved::new(9, vec![workspace(1, 2, split(5, 500, 3, 3))]))
                .expect("encode"),
            // An identity the server never issued.
            serde_json::to_string(&Saved::new(4, vec![workspace(1, 2, split(5, 500, 3, 4))]))
                .expect("encode"),
        ] {
            fs::write(dir.join(NAME), damaged).expect("write");
            let error = load(&dir).expect_err("damaged state must be rejected");
            assert!(error.to_string().contains("workspaces.json.corrupt"));
            assert!(!dir.join(NAME).exists());
            assert!(dir.join(format!("{NAME}.corrupt")).exists());
        }
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn a_saved_divider_cannot_hide_a_pane() {
        let dir = scratch();
        let saved = Saved::new(9, vec![workspace(1, 2, split(5, 0, 3, 4))]);
        store(&dir, &saved).expect("store");
        let restored = load(&dir).expect("load").expect("saved state exists");
        let Layout::Split { ratio, .. } = &restored.workspaces[0].tabs[0].layout else {
            panic!("the split was lost");
        };
        assert_eq!(*ratio, spooky_core::MIN_SPLIT_SHARE);
        fs::remove_dir_all(dir).expect("cleanup");
    }
}
