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
pub const PROTOCOL_VERSION: u32 = 10;
/// Maximum encoded request or response, including its newline delimiter.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
/// Default terminal background, shared with the native window chrome.
pub const DEFAULT_BACKGROUND: [u8; 3] = [34, 37, 46];
/// Share of a split given to its first side when the split is created, in thousandths.
pub const EVEN_SPLIT: u16 = 500;
/// Smallest share either side of a split may keep, in thousandths.
pub const MIN_SPLIT_SHARE: u16 = 100;
/// Title of a terminal whose program has not set one.
pub const DEFAULT_TITLE: &str = "Terminal";

/// Operations on the server-owned workspace tree and terminal state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    /// Change a tab's display label.
    RenameTab {
        /// Tab identity.
        tab: u64,
        /// New display label.
        title: String,
    },
    /// Terminate every pane of a tab and remove it; closing a workspace's last tab removes the workspace.
    CloseTab {
        /// Tab to terminate.
        tab: u64,
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
    /// Move a split divider.
    SetSplitRatio {
        /// Split identity from the layout.
        split: u64,
        /// Share of the first side in thousandths, within
        /// [`MIN_SPLIT_SHARE`] and `1000 - MIN_SPLIT_SHARE`.
        ratio: u16,
    },
    /// Search a pane's whole scrollback and move its viewport to the match. A
    /// new `query` starts at the viewport; repeating it continues from the last
    /// match, wrapping around. An empty query clears the search.
    Find {
        /// Pane to search.
        pane: u64,
        /// Text to find, ignoring case.
        query: String,
        /// Search order.
        direction: FindDirection,
    },
    /// Read the latest text the pane's application stored with OSC 52.
    Clipboard {
        /// Pane whose clipboard write to read.
        pane: u64,
    },
    /// Start a tab in a workspace whose only pane is an agent (oh-my-pi) instead of a
    /// shell. The server runs the agent in the workspace's directory.
    CreateAgent {
        /// Workspace identity.
        workspace: u64,
        /// Which agent program to run (`omp`, and others as the server supports them).
        backend: String,
    },
    /// Send a message to an agent pane. While the agent is working it is queued
    /// after the current turn.
    AgentPrompt {
        /// Destination agent pane.
        pane: u64,
        /// Message text, at most [`MAX_AGENT_PROMPT_BYTES`].
        text: String,
    },
    /// Interrupt an agent pane's current turn.
    AgentAbort {
        /// Destination agent pane.
        pane: u64,
    },
    /// Open an agent tab in the workspace of an existing pane, in a given directory.
    /// The `omp` shell shim sends this, naming its own pane.
    OpenAgent {
        /// A pane of the workspace to open the agent in.
        pane: u64,
        /// Which agent program to run.
        backend: String,
        /// Absolute working directory on the server.
        cwd: String,
        /// The command-line arguments the user typed after the program name. The
        /// backend accepts a closed set and refuses the rest; at most
        /// [`MAX_AGENT_ARGS`] of at most [`MAX_AGENT_ARG_BYTES`] bytes each.
        args: Vec<String>,
    },
    /// Choose the model an agent uses (`provider/id` as listed in [`Agent::models`]).
    AgentSetModel {
        /// Destination agent pane.
        pane: u64,
        /// The model, as listed by the agent.
        model: String,
    },
    /// Choose how much an agent thinks (one of [`Agent::levels`]).
    AgentSetThinking {
        /// Destination agent pane.
        pane: u64,
        /// The level, as listed by the agent.
        level: String,
    },
    /// Ask the agent's program what its providers report about the remaining usage; the
    /// answer arrives as [`Agent::usage`]. Refused while the agent is working.
    AgentUsage {
        /// Destination agent pane.
        pane: u64,
    },
    /// Answer a question or approval an agent is waiting on ([`Agent::dialogs`]).
    AgentAnswer {
        /// Destination agent pane.
        pane: u64,
        /// The dialog being answered.
        dialog: String,
        /// The answer.
        answer: DialogAnswer,
    },
    /// Identify the server and count the shells it holds.
    Status,
    /// Stop the server and end every shell. The workspaces are saved and are
    /// restored, with new shells, by the next server start.
    Shutdown,
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
    /// Outcome of [`Request::Find`] when something matched.
    Found {
        /// One-based position of the match the viewport moved to.
        current: u32,
        /// Matches in the whole scrollback.
        total: u32,
    },
    /// [`Request::Find`] matched nothing.
    NotFound,
    /// Text stored by OSC 52.
    Clipboard {
        /// The write this is; compare with [`Pane::clipboard_seq`].
        seq: u64,
        /// Decoded text, at most [`MAX_CLIPBOARD_BYTES`].
        text: String,
    },
    /// Server identity and load.
    Status(ServerStatus),
}

/// Longest OSC 52 write the server keeps, in bytes.
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// Search order for [`Request::Find`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FindDirection {
    /// Toward newer output.
    Next,
    /// Toward older output.
    Previous,
}

/// What `spooky-server status` and `stop` report.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerStatus {
    /// Process identity.
    pub pid: u32,
    /// Wire version the server speaks.
    pub version: u32,
    /// Seconds since the server started.
    pub uptime_secs: u64,
    /// Workspaces held.
    pub workspaces: u32,
    /// Tabs held.
    pub tabs: u32,
    /// Shells held, running or exited.
    pub panes: u32,
    /// Shells still running.
    pub running: u32,
    /// Agent panes whose process is alive.
    pub agents: u32,
}

/// A search match inside the viewport.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Highlight {
    /// Viewport row.
    pub row: u16,
    /// First matching column.
    pub start: u16,
    /// Last matching column, inclusive.
    pub end: u16,
    /// Whether this is the match [`Request::Find`] last moved to.
    pub current: bool,
}

/// Longest message [`Request::AgentPrompt`] carries, in bytes.
pub const MAX_AGENT_PROMPT_BYTES: usize = 256 * 1024;
/// Most arguments [`Request::OpenAgent`] carries.
pub const MAX_AGENT_ARGS: usize = 16;
/// Longest single argument of [`Request::OpenAgent`], in bytes.
pub const MAX_AGENT_ARG_BYTES: usize = 8 * 1024;

/// Who wrote an entry of an agent's conversation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentRole {
    /// The user.
    User,
    /// The model.
    Assistant,
    /// The model's reasoning, which the interface may fold away.
    Thinking,
    /// A tool the agent ran; see [`AgentMessage::tool`].
    Tool,
    /// Something the server reports: an error, the agent ending.
    Notice,
}

/// How far a tool call got.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ToolState {
    /// Still running.
    Running,
    /// Finished.
    Done,
    /// Finished with an error.
    Failed,
}

/// A tool call in a conversation. All of it came from a model or a tool and is
/// untrusted: draw it, never act on it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTool {
    /// The tool's name, such as `read` or `bash`.
    pub name: String,
    /// The main argument: a path, a command, a URL.
    pub detail: String,
    /// How far it got.
    pub state: ToolState,
    /// What it printed, bounded.
    pub output: String,
}

/// One entry of an agent's conversation. Everything in it came from a model, a tool
/// or a file and is untrusted: draw it, never act on it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMessage {
    /// Identity within the agent, stable while the entry is kept.
    pub id: u64,
    /// Who wrote it.
    pub role: AgentRole,
    /// The text, possibly still growing while the agent streams it; for a tool, what
    /// it is for.
    pub text: String,
    /// Set for [`AgentRole::Tool`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<AgentTool>,
}

/// What a dialog asks of the user.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DialogKind {
    /// Allow or refuse.
    Confirm,
    /// Pick one of the options.
    Select,
    /// Type an answer.
    Input,
}

/// A question or approval an agent waits on. Its text is untrusted; only a user
/// action may answer it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentDialog {
    /// Identity, echoed in [`Request::AgentAnswer`].
    pub id: String,
    /// What it asks.
    pub kind: DialogKind,
    /// The question.
    pub title: String,
    /// More about it, possibly empty.
    pub message: String,
    /// The choices of a [`DialogKind::Select`].
    pub options: Vec<String>,
}

/// A user's answer to an [`AgentDialog`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DialogAnswer {
    /// Allowed (`true`) or refused.
    Confirm(bool),
    /// The chosen option or the typed text.
    Value(String),
    /// Dismissed.
    Cancel,
}

/// A job the agent's program assigns a model to (`default`, `plan`, `smol`...), as its
/// own configuration says. Names and models are untrusted text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelRole {
    /// The role's name.
    pub role: String,
    /// The model, `provider/id`.
    pub model: String,
    /// The thinking level the role asks for, when it names one.
    pub thinking: Option<String>,
}

/// How much of one allowance a provider's account has left, as the agent's program reports
/// it. The text is untrusted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageWindow {
    /// The allowance, such as `5 hours` or `7 days`.
    pub name: String,
    /// The account it belongs to.
    pub account: String,
    /// What is left, in whole percent.
    pub left: u8,
    /// When it renews, such as `4h51m`.
    pub resets: String,
}

/// What one provider reports about its accounts' allowances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageProvider {
    /// The provider's name.
    pub name: String,
    /// Remarks about the accounts, such as saved resets.
    pub notes: Vec<String>,
    /// The allowances.
    pub windows: Vec<UsageWindow>,
}

/// What an agent pane shows instead of a terminal viewport.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Agent {
    /// Which agent program this is, as named in [`Request::CreateAgent`].
    pub backend: String,
    /// The agent is working on a turn.
    pub busy: bool,
    /// Why the agent process is gone, once it is.
    pub ended: Option<String>,
    /// Earlier entries dropped from the front to keep the conversation bounded.
    pub dropped: u32,
    /// The most recent entries, oldest first.
    pub messages: Vec<AgentMessage>,
    /// Questions and approvals waiting for the user, oldest first.
    pub dialogs: Vec<AgentDialog>,
    /// The model in use, `provider/id`.
    pub model: Option<String>,
    /// The thinking level in use.
    pub thinking: Option<String>,
    /// Models that can be chosen, bounded.
    pub models: Vec<String>,
    /// Thinking levels that can be chosen.
    pub levels: Vec<String>,
    /// How much of the model's context window is used, in percent.
    pub context: Option<u8>,
    /// The roles the program gives its models, bounded.
    pub roles: Vec<ModelRole>,
    /// What the providers report about the remaining usage, once asked for.
    pub usage: Option<Vec<UsageProvider>>,
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
    /// Two sub-layouts separated by a divider.
    Split {
        /// Server-scoped identity, used to move the divider.
        id: u64,
        /// Divider orientation.
        axis: Axis,
        /// Share of the first side in thousandths.
        ratio: u16,
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

    /// Give the split `split` its first side `ratio` thousandths of the space.
    /// Returns false when the layout holds no such split.
    pub fn set_ratio(&mut self, split: u64, ratio: u16) -> bool {
        match self {
            Self::Pane(_) => false,
            Self::Split {
                id,
                ratio: current,
                first,
                second,
                ..
            } => {
                if *id == split {
                    *current = ratio;
                    true
                } else {
                    first.set_ratio(split, ratio) || second.set_ratio(split, ratio)
                }
            }
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
    /// Whether the user named the tab. An unnamed tab is labelled with its
    /// terminal's title.
    #[serde(default)]
    pub renamed: bool,
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
    /// Mouse events the application asked to receive. With [`MouseTracking::Off`]
    /// the mouse belongs to the terminal (selection, scrollback).
    pub mouse_tracking: MouseTracking,
    /// Byte format of mouse reports for this application.
    pub mouse_encoding: MouseEncoding,
    /// Wheel movement should be sent as arrow keys: the application is on the
    /// alternate screen and left alternate scroll (DECSET 1007) enabled.
    pub alternate_scroll: bool,
    /// Scrollback lines above the viewport.
    pub history: u32,
    /// Lines the viewport is scrolled up from live output; zero at the bottom.
    pub scrolled: u32,
    /// Whether the application wants focus in and out reports (DECSET 1004).
    pub focus_reporting: bool,
    /// Number of OSC 52 clipboard writes so far; fetch the text with
    /// [`Request::Clipboard`] when it grows.
    pub clipboard_seq: u64,
    /// Search matches inside the viewport.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub highlights: Vec<Highlight>,
    /// Set for an agent pane, whose conversation replaces the terminal viewport
    /// (every field above is then empty or zero).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Agent>,
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
            mouse_tracking,
            mouse_encoding,
            alternate_scroll,
            history,
            scrolled,
            focus_reporting,
            clipboard_seq,
            highlights,
            agent,
            ..
        } = update;
        self.title = title;
        self.cols = cols;
        self.rows = rows;
        self.cursor = cursor;
        self.exited = exited;
        self.bracketed_paste = bracketed_paste;
        self.application_cursor = application_cursor;
        self.mouse_tracking = mouse_tracking;
        self.mouse_encoding = mouse_encoding;
        self.alternate_scroll = alternate_scroll;
        self.history = history;
        self.scrolled = scrolled;
        self.focus_reporting = focus_reporting;
        self.clipboard_seq = clipboard_seq;
        self.highlights = highlights;
        self.agent = agent;
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
    /// Strikethrough styling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub strikethrough: bool,
    /// Zero for one cell per scalar value; otherwise `text` is a single cell
    /// occupying this many columns (1 with combining characters, 2 when wide).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub width: u8,
    /// OSC 8 hyperlink target the text carries. Untrusted: the application
    /// chose it, and it may differ from what the text says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
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
    /// Shape requested by the application.
    pub shape: CursorShape,
    /// Whether the application asked for a blinking cursor.
    pub blinking: bool,
}

/// Which mouse events an application asked the terminal to report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseTracking {
    /// No reporting.
    Off,
    /// Button presses and releases (DECSET 1000).
    Clicks,
    /// Clicks plus motion while a button is held (DECSET 1002).
    Drag,
    /// Clicks plus all motion (DECSET 1003).
    Motion,
}

/// Byte format of mouse reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseEncoding {
    /// Legacy `CSI M` with one byte per value; positions past column 222 cannot be expressed.
    Normal,
    /// Legacy `CSI M` with UTF-8 coded values (DECSET 1005).
    Utf8,
    /// `CSI <` decimal reports with distinct release (DECSET 1006).
    Sgr,
}

/// Cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    /// Fills the cell.
    Block,
    /// A bar under the cell.
    Underline,
    /// A bar at the left edge of the cell.
    Beam,
}

#[cfg(test)]
mod tests {
    use super::{Cursor, CursorShape, Line, MouseEncoding, MouseTracking, Pane, Run};

    fn run(col: u16, text: &str, width: u8) -> Run {
        Run {
            col,
            text: text.into(),
            fg: [1, 2, 3],
            bg: [0, 0, 0],
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            width,
            link: None,
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
                shape: CursorShape::Beam,
                blinking: false,
            },
            exited: false,
            bracketed_paste: false,
            application_cursor: false,
            mouse_tracking: MouseTracking::Off,
            mouse_encoding: MouseEncoding::Normal,
            alternate_scroll: false,
            history: 0,
            scrolled: 0,
            focus_reporting: false,
            clipboard_seq: 0,
            highlights: Vec::new(),
            agent: None,
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
