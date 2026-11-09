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
pub const PROTOCOL_VERSION: u32 = 2;
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
    /// All viewports for a full snapshot; only changed viewports for an update.
    pub panes: Vec<Pane>,
}

impl Snapshot {
    /// Apply an ordered update from the same server without copying unchanged grids.
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
            if let Some(current) = self.panes.iter_mut().find(|current| current.id == pane.id) {
                *current = pane;
            } else {
                self.panes.push(pane);
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
    /// Visible cells, excluding wide-character spacer cells.
    pub cells: Vec<Cell>,
    /// Viewport-relative cursor.
    pub cursor: Cursor,
    /// Whether the shell has terminated; its screen remains inspectable.
    pub exited: bool,
    /// Whether paste should be wrapped in bracketed-paste escape sequences.
    pub bracketed_paste: bool,
    /// Whether navigation uses application cursor escape sequences.
    pub application_cursor: bool,
}

/// One display cell, including combining characters attached to its base glyph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cell {
    /// Zero-based viewport row.
    pub row: u16,
    /// Zero-based terminal column.
    pub col: u16,
    /// Base character followed by any combining characters.
    pub text: String,
    /// Resolved RGB foreground.
    pub fg: [u8; 3],
    /// Resolved RGB background.
    pub bg: [u8; 3],
    /// Bold styling.
    pub bold: bool,
    /// Italic styling.
    pub italic: bool,
    /// Underline styling.
    pub underline: bool,
    /// The glyph occupies two columns; it must still be rendered.
    pub wide: bool,
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
