//! Shared session protocol and authenticated local/SSH transport for Spooky.
//!
//! One bounded JSON value per line, one response per request. IDs and revisions
//! are scoped to a running server, not durable across operating-system restarts.
/// Local/SSH session connections and explicit file transfer.
pub mod client;
/// Owner-only runtime directories and Unix socket selection.
pub mod paths;
use serde::{Deserialize, Serialize};

/// Wire version checked before using a server.
pub const PROTOCOL_VERSION: u32 = 3;
/// Maximum encoded request or response, including its newline delimiter.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Default terminal background, shared with the native window chrome.
pub const DEFAULT_BACKGROUND: [u8; 3] = [34, 37, 46];

/// Operations on the server-owned workspace tree and terminal state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    /// Read full state when `since` is absent; otherwise wait for changes (up to
    /// one second) and return only changed panes with the complete workspace tree.
    Snapshot {
        /// Last applied revision from this server; reset after reconnecting.
        since: Option<u64>,
    },
    /// Start a workspace with an initial tab and shell.
    CreateWorkspace {
        /// User-visible label.
        name: String,
        /// Working directory on the server; absent uses its default directory.
        cwd: Option<String>,
    },
    /// Change a workspace's display label.
    RenameWorkspace {
        /// Workspace identity.
        workspace: u64,
        /// New display label.
        name: String,
    },
    /// Start another tab in a workspace's working directory.
    CreateTab {
        /// Workspace identity.
        workspace: u64,
        /// New tab's display label.
        title: String,
    },
    /// Replace a pane leaf with a split containing it and a new shell.
    Split {
        /// Existing pane identity.
        pane: u64,
        /// Direction of the divider.
        axis: Axis,
    },
    /// Write bytes to a shell's pseudoterminal without interpreting commands.
    Input {
        /// Destination pane.
        pane: u64,
        /// Raw input, including any escape sequences or UTF-8.
        bytes: Vec<u8>,
    },
    /// Resize both the emulator grid and the underlying pseudoterminal.
    Resize {
        /// Destination pane.
        pane: u64,
        /// Grid width in terminal columns.
        cols: u16,
        /// Grid height in terminal rows.
        rows: u16,
    },
    /// Move the pane's scrollback viewport.
    Scroll {
        /// Destination pane.
        pane: u64,
        /// Positive scrolls toward earlier output; negative toward the bottom.
        lines: i32,
    },
    /// Terminate a pane's process and remove its layout leaf.
    ClosePane {
        /// Pane to terminate.
        pane: u64,
    },
    /// Terminate all processes belonging to a workspace and remove it.
    CloseWorkspace {
        /// Workspace to terminate.
        workspace: u64,
    },
    /// Query protocol compatibility without mutating sessions.
    Ping,
}

/// One reply to a request; application errors do not imply transport failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    /// Workspace tree and full or changed terminal viewports, as requested.
    Snapshot(Snapshot),
    /// No visible changes since the client's revision.
    Unchanged {
        /// Current revision.
        revision: u64,
    },
    /// Mutation accepted.
    Ok,
    /// Human-readable application error.
    Error(String),
    /// Protocol handshake.
    Pong {
        /// Server wire version.
        version: u32,
    },
}

/// Divider orientation, independent of pixel dimensions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Axis {
    /// Horizontal divider: first above second.
    Horizontal,
    /// Vertical divider: first to the left of second.
    Vertical,
}

/// Binary pane layout owned by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Layout {
    /// One terminal pane.
    Pane(u64),
    /// Two equally sized sub-layouts separated by a divider.
    Split {
        /// Divider orientation.
        axis: Axis,
        /// Top or left subtree.
        first: Box<Layout>,
        /// Bottom or right subtree.
        second: Box<Layout>,
    },
}
impl Layout {
    /// Append leaf pane identities in display order without clearing `out`.
    pub fn panes(&self, out: &mut Vec<u64>) {
        match self {
            Self::Pane(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.panes(out);
                second.panes(out);
            }
        }
    }

    /// Whether this layout contains the given pane.
    pub fn contains(&self, pane: u64) -> bool {
        match self {
            Self::Pane(id) => *id == pane,
            Self::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }
}

/// A project-level group of terminal tabs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// Server-scoped identity.
    pub id: u64,
    /// Display label.
    pub name: String,
    /// Server-side directory used when spawning shells.
    pub cwd: String,
    /// Ordered tabs.
    pub tabs: Vec<Tab>,
}

/// A named pane layout within a workspace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    /// Server-scoped identity.
    pub id: u64,
    /// Display label.
    pub title: String,
    /// Authoritative split tree.
    pub layout: Layout,
}

/// Versioned workspace tree and terminal viewports from one server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Wire protocol version.
    pub version: u32,
    /// Revision used for conditional snapshot requests.
    pub revision: u64,
    /// Ordered workspace tree.
    pub workspaces: Vec<Workspace>,
    /// Every viewport for a full snapshot. An update carries only the panes that
    /// changed, and each of those only the rows that changed.
    pub panes: Vec<Pane>,
}

impl Snapshot {
    /// Apply an ordered update from the same server without copying unchanged rows.
    /// The workspace tree is authoritative: panes removed from it are discarded.
    pub fn apply_update(&mut self, update: Self) {
        self.version = update.version;
        self.revision = update.revision;
        self.workspaces = update.workspaces;
        self.panes.retain(|pane| {
            self.workspaces
                .iter()
                .flat_map(|workspace| &workspace.tabs)
                .any(|tab| tab.layout.contains(pane.id))
        });
        for pane in update.panes {
            match self.panes.iter_mut().find(|current| current.id == pane.id) {
                Some(current) => current.apply_update(pane),
                None => {
                    let mut fresh = pane;
                    let lines = std::mem::take(&mut fresh.lines);
                    fresh.set_lines(lines);
                    self.panes.push(fresh);
                }
            }
        }
    }
}

/// Renderable terminal viewport and input modes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pane {
    /// Server-scoped identity.
    pub id: u64,
    /// Terminal title.
    pub title: String,
    /// Grid width.
    pub cols: u16,
    /// Grid height.
    pub rows: u16,
    /// Viewport rows. A full snapshot carries every row; an update only the rows
    /// that changed since the requested revision. After [`Pane::apply_update`] a
    /// client's pane holds exactly one line per row, ordered by row.
    pub lines: Vec<Line>,
    /// Viewport-relative cursor.
    pub cursor: Cursor,
    /// Whether the shell has terminated; its screen remains inspectable.
    pub exited: bool,
    /// Whether paste should be wrapped in bracketed-paste escape sequences.
    pub bracketed_paste: bool,
    /// Whether navigation uses application cursor escape sequences.
    pub application_cursor: bool,
}

impl Pane {
    /// Merge an update for this pane: scalar state is replaced and only the rows
    /// the update carries are overwritten. Grid size changes always arrive with
    /// every row, so rows beyond the new height are dropped and new ones filled.
    pub fn apply_update(&mut self, update: Self) {
        let Self {
            title,
            cols,
            rows,
            lines,
            cursor,
            exited,
            bracketed_paste,
            application_cursor,
            ..
        } = update;
        self.title = title;
        self.cols = cols;
        self.rows = rows;
        self.cursor = cursor;
        self.exited = exited;
        self.bracketed_paste = bracketed_paste;
        self.application_cursor = application_cursor;
        self.set_lines(lines);
    }

    /// Visible text of one row without trailing blanks; empty for an unknown row.
    pub fn row_text(&self, row: u16) -> String {
        self.lines
            .iter()
            .find(|line| line.row == row)
            .map(|line| line.text_between(0, self.cols.saturating_sub(1)))
            .unwrap_or_default()
    }

    /// Make `lines` dense (one per row), overwriting the rows listed in `changed`.
    fn set_lines(&mut self, changed: Vec<Line>) {
        let rows = usize::from(self.rows);
        self.lines.truncate(rows);
        while self.lines.len() < rows {
            self.lines.push(Line {
                row: self.lines.len() as u16,
                runs: Vec::new(),
            });
        }
        for line in changed {
            if let Some(slot) = self.lines.get_mut(usize::from(line.row)) {
                *slot = line;
            }
        }
    }
}

/// One viewport row. Columns not covered by a run are blank with the default
/// background, so an empty row costs nothing on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    /// Zero-based viewport row.
    pub row: u16,
    /// Styled text in increasing column order.
    pub runs: Vec<Run>,
}

impl Line {
    /// Text of the cells starting in columns `start..=end`, without trailing
    /// blanks. Columns no run covers read as spaces.
    pub fn text_between(&self, start: u16, end: u16) -> String {
        let mut text = String::new();
        let mut column = start;
        for run in &self.runs {
            for (col, cell, width) in run.cells() {
                if col < start || col > end {
                    continue;
                }
                while column < col {
                    text.push(' ');
                    column += 1;
                }
                text.push_str(cell);
                column = col.saturating_add(width);
            }
        }
        text.truncate(text.trim_end_matches(' ').len());
        text
    }
}

/// Adjacent cells sharing one style, starting at `col`.
///
/// With `width == 0` every Unicode scalar value in `text` is one single-column
/// cell. A cell that is wide or carries combining characters is instead a run of
/// its own whose whole `text` is that one cell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// Zero-based terminal column of the first cell.
    pub col: u16,
    /// Cell contents; see the type documentation for how it maps to cells.
    pub text: String,
    /// Resolved RGB foreground.
    pub fg: [u8; 3],
    /// Resolved RGB background.
    pub bg: [u8; 3],
    /// Bold styling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub bold: bool,
    /// Italic styling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub italic: bool,
    /// Underline styling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub underline: bool,
    /// Zero for one cell per scalar value; otherwise `text` is a single cell
    /// occupying this many columns (1 with combining characters, 2 when wide).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: u8,
}

impl Run {
    /// Terminal columns this run covers.
    pub fn columns(&self) -> u16 {
        match self.width {
            0 => u16::try_from(self.text.chars().count()).unwrap_or(u16::MAX),
            width => u16::from(width),
        }
    }

    /// Each terminal cell as `(column, text, columns occupied)`.
    pub fn cells(&self) -> impl Iterator<Item = (u16, &str, u16)> {
        Cells {
            run: self,
            byte: 0,
            column: 0,
        }
    }
}

struct Cells<'a> {
    run: &'a Run,
    byte: usize,
    column: u16,
}

impl<'a> Iterator for Cells<'a> {
    type Item = (u16, &'a str, u16);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self
            .run
            .text
            .get(self.byte..)
            .filter(|rest| !rest.is_empty())?;
        let (length, width) = match self.run.width {
            0 => (rest.chars().next()?.len_utf8(), 1),
            width => (rest.len(), u16::from(width)),
        };
        let item = (
            self.run.col.saturating_add(self.column),
            &rest[..length],
            width,
        );
        self.byte += length;
        self.column = self.column.saturating_add(width);
        Some(item)
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

/// Terminal cursor position relative to the current viewport.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    /// Zero-based row.
    pub row: u16,
    /// Zero-based column.
    pub col: u16,
    /// False when hidden by the application or outside the viewport.
    pub visible: bool,
}

#[cfg(test)]
mod tests {
    use super::{Cursor, Line, Pane, Run};

    fn run(col: u16, text: &str, width: u8) -> Run {
        Run {
            col,
            text: text.into(),
            fg: [1, 2, 3],
            bg: [0, 0, 0],
            bold: false,
            italic: false,
            underline: false,
            width,
        }
    }

    fn pane(cols: u16, rows: u16, lines: Vec<Line>) -> Pane {
        Pane {
            id: 1,
            title: String::new(),
            cols,
            rows,
            lines,
            cursor: Cursor {
                row: 0,
                col: 0,
                visible: true,
            },
            exited: false,
            bracketed_paste: false,
            application_cursor: false,
        }
    }

    fn line(row: u16, runs: Vec<Run>) -> Line {
        Line { row, runs }
    }

    #[test]
    fn a_run_is_one_cell_per_scalar_unless_it_is_a_single_cell() {
        assert_eq!(
            run(3, "ab", 0).cells().collect::<Vec<_>>(),
            [(3, "a", 1), (4, "b", 1)]
        );
        assert_eq!(run(5, "界", 2).cells().collect::<Vec<_>>(), [(5, "界", 2)]);
        assert_eq!(
            run(0, "e\u{301}", 1).cells().collect::<Vec<_>>(),
            [(0, "e\u{301}", 1)]
        );
        assert_eq!(run(3, "ab", 0).columns(), 2);
        assert_eq!(run(5, "界", 2).columns(), 2);
    }

    #[test]
    fn row_text_pads_uncovered_columns_and_drops_trailing_blanks() {
        let pane = pane(
            10,
            1,
            vec![line(
                0,
                vec![run(1, "ab", 0), run(5, "界", 2), run(7, "z", 0)],
            )],
        );
        assert_eq!(pane.row_text(0), " ab  界z");
        assert_eq!(pane.lines[0].text_between(2, 5), "b  界");
        assert_eq!(pane.row_text(9), "");
    }

    #[test]
    fn an_update_overwrites_only_the_rows_it_carries() {
        let mut current = pane(
            4,
            3,
            vec![
                line(0, vec![run(0, "one", 0)]),
                line(1, vec![run(0, "two", 0)]),
                line(2, vec![run(0, "six", 0)]),
            ],
        );
        current.apply_update(pane(4, 3, vec![line(1, vec![run(0, "2nd", 0)])]));
        assert_eq!(
            (
                current.row_text(0),
                current.row_text(1),
                current.row_text(2)
            ),
            ("one".into(), "2nd".into(), "six".into())
        );

        // A row that became blank is an explicit empty line, not an omission.
        current.apply_update(pane(4, 3, vec![line(0, Vec::new())]));
        assert_eq!(current.row_text(0), "");
        assert_eq!(current.row_text(1), "2nd");
    }

    #[test]
    fn a_resize_reshapes_the_held_rows() {
        let mut current = pane(4, 3, vec![line(2, vec![run(0, "old", 0)])]);
        current.apply_update(pane(
            6,
            2,
            vec![line(0, vec![run(0, "a", 0)]), line(1, vec![run(0, "b", 0)])],
        ));
        assert_eq!(current.lines.len(), 2);
        assert_eq!(
            (current.row_text(0), current.row_text(1)),
            ("a".into(), "b".into())
        );

        current.apply_update(pane(
            6,
            4,
            (0..4).map(|row| line(row, Vec::new())).collect(),
        ));
        assert_eq!(current.lines.len(), 4);
        assert!(
            current
                .lines
                .iter()
                .enumerate()
                .all(|(row, l)| usize::from(l.row) == row)
        );
    }
}
