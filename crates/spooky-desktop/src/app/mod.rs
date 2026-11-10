//! The application: its state, its messages, and the composition of the features.
//!
//! `App` owns the shared state. Each file here adds the part of `update` that belongs to
//! one concern: the window, the server connection, workspaces and tabs, terminal input,
//! pane sizing, and the adapters that apply a feature's messages and outcomes.

mod connection;
mod dropdown;
mod features;
mod grid;
mod input;
mod subscription;
mod view;
mod workspaces;

pub(crate) use subscription::subscription;

use crate::{
    commands::Command as AppCommand,
    config::{Config, SizeMode},
    features::{
        attachments::{self, Attachments},
        connection::Connection,
        hyperlinks::Hyperlinks,
        palette::{self, Palette},
        search::{self, Search},
        settings::{self, Settings},
        splits::{self, Splits},
        tailscale,
        window::Hotkeys,
    },
    link::{Command, Event, Link},
};
use iced::{Point, Rectangle, Size, Task, event, keyboard, window};
use spooky_core::{Axis, Pane, Request, Snapshot, Tab, Workspace};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub(crate) enum Message {
    Tick,
    Link(Event),
    Hotkey(u32),
    Key(
        keyboard::Key,
        keyboard::key::Physical,
        keyboard::Modifiers,
        Option<String>,
        event::Status,
    ),
    Opened(window::Id),
    Close(window::Id),
    WindowFocus(bool),
    /// Shows the window at the top-centre of its display: id, display size, window size.
    Present(window::Id, Option<Size>, Size),
    Quit,
    DragWindow,
    NewWorkspace,
    NewTab,
    Split(Axis),
    /// Split this pane, which need not be the selected one.
    SplitPane(u64, Axis),
    Workspace(u64),
    /// A context menu was requested at this window position.
    ContextMenu(ContextTarget, Point),
    DismissContextMenu,
    RenameWorkspace(u64),
    CloseWorkspace(u64),
    Tab(u64),
    Rename,
    RenameTab(u64),
    AskCloseTab(u64),
    RenameValue(String),
    RenameSubmit,
    CancelRename,
    ToggleSidebar,
    ToggleWorkspaces,
    ToggleConnection,
    ToggleMenu,
    AskClosePane,
    AskClosePaneOf(u64),
    AskCloseWorkspace,
    ConfirmClose,
    CancelClose,
    TargetValue(String),
    Connect,
    Local,
    Retry,
    Tailscale(tailscale::Message),
    Attachments(attachments::Message),
    Bounds(u64, Rectangle),
    FocusPane(u64, Vec<u8>),
    PaneInput(u64, Vec<u8>),
    Select(u64, (u16, u16), (u16, u16)),
    ScrollPane(u64, i32),
    Paste(Option<String>),
    Blink,
    LinkHover(Option<String>),
    OpenLink(String),
    Splits(splits::Message),
    Search(search::Message),
    Palette(palette::Message),
    /// Carry out a command, as its keyboard shortcut would.
    Run(AppCommand),
    Modifiers(keyboard::Modifiers),
    /// The pointer entered (`true`) or left (`false`) this pane.
    HoverPane(u64, bool),
    Copy,
    PasteClipboard,
    Settings(settings::Message),
}
#[derive(Clone, Copy)]
pub(super) enum Confirm {
    Pane(u64),
    Workspace(u64),
    Tab(u64),
}
/// What a context menu is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContextTarget {
    Workspace(u64),
    Tab(u64),
    Pane(u64),
}
pub(super) enum Pending {
    Workspace(HashSet<u64>),
    Tab(u64, HashSet<u64>),
    Pane(HashSet<u64>),
}
#[derive(Clone, Copy)]
pub(super) struct Selection {
    pane: u64,
    anchor: (u16, u16),
    head: (u16, u16),
}
pub(crate) struct App {
    config: Config,
    link: Link,
    snapshot: Option<Snapshot>,
    workspace: Option<u64>,
    tab: Option<u64>,
    pane: Option<u64>,
    terminal_focused: bool,
    selection: Option<Selection>,
    bounds: HashMap<u64, Rectangle>,
    grids: HashMap<u64, (u16, u16)>,
    sidebar_open: bool,
    workspaces_open: bool,
    connection_open: bool,
    menu_open: bool,
    context_menu: Option<(ContextTarget, Point)>,
    rename_workspace: Option<u64>,
    rename_tab: Option<u64>,
    tailscale: tailscale::Picker,
    rename: String,
    renaming: bool,
    status: String,
    confirm: Option<Confirm>,
    pending: Option<Pending>,
    window: Option<window::Id>,
    /// Hidden by Spooky itself (shortcut or close button), not minimized by the user.
    hidden: bool,
    focused: bool,
    /// The server in use and how the connection to it is doing.
    connection: Connection,
    /// Whether a blinking cursor is in its visible phase.
    blink_on: bool,
    /// The link under the pointer, and following one.
    hyperlinks: Hyperlinks,
    /// Dragging the dividers between panes.
    splits: Splits,
    /// The scrollback search strip.
    search: Search,
    /// Last OSC 52 write number seen per pane.
    clipboard_seen: HashMap<u64, u64>,
    /// The pane last told it has the keyboard focus.
    focus_reported: Option<u64>,
    /// Keyboard modifiers as last reported by the window.
    modifiers: keyboard::Modifiers,
    /// The pane under the pointer, which shows its quick actions.
    hover_pane: Option<u64>,
    /// The global shortcut, when the session allows one.
    hotkeys: Option<Hotkeys>,
    /// The settings page.
    settings: Settings,
    /// The configured window size has not been applied yet: it needs the display.
    size_pending: bool,
    /// Attached files and pane captures.
    attachments: Attachments,
    /// The command palette.
    palette: Palette,
}
impl App {
    pub(crate) fn new(
        config: Config,
        status: String,
        hotkeys: Option<Hotkeys>,
    ) -> (Self, Task<Message>) {
        let size_pending = config.size_mode() == SizeMode::Percent;
        (
            Self {
                config,
                link: Link::start(),
                snapshot: None,
                workspace: None,
                tab: None,
                pane: None,
                terminal_focused: true,
                selection: None,
                bounds: HashMap::new(),
                grids: HashMap::new(),
                sidebar_open: true,
                workspaces_open: true,
                connection_open: false,
                menu_open: false,
                context_menu: None,
                rename_workspace: None,
                rename_tab: None,
                tailscale: tailscale::Picker::default(),
                rename: String::new(),
                renaming: false,
                status,
                confirm: None,
                pending: None,
                window: None,
                hidden: false,
                focused: false,
                connection: Connection::default(),
                blink_on: true,
                hyperlinks: Hyperlinks::default(),
                splits: Splits::default(),
                search: Search::default(),
                clipboard_seen: HashMap::new(),
                focus_reported: None,
                modifiers: keyboard::Modifiers::default(),
                hover_pane: None,
                hotkeys,
                settings: Settings::default(),
                size_pending,
                attachments: Attachments::default(),
                palette: Palette::default(),
            },
            window::latest().map(|id| id.map(Message::Opened).unwrap_or(Message::Tick)),
        )
    }
    fn send(&mut self, request: Request) {
        if self
            .link
            .tx
            .try_send(Command::Request(self.connection.generation(), request))
            .is_err()
        {
            self.status = "Connection queue full; command not sent".into();
        }
    }
    fn active(&self) -> Option<(&Workspace, &Tab)> {
        let s = self.snapshot.as_ref()?;
        let w = s.workspaces.iter().find(|w| Some(w.id) == self.workspace)?;
        let t = w.tabs.iter().find(|t| Some(t.id) == self.tab)?;
        Some((w, t))
    }
    fn current_pane(&self) -> Option<&Pane> {
        self.snapshot
            .as_ref()?
            .panes
            .iter()
            .find(|p| Some(p.id) == self.pane)
    }
    pub(crate) fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.sync_focus_report();
        task
    }
    /// Whether the selected pane's blinking cursor should be animating.
    fn blink_wanted(&self) -> bool {
        self.config.cursor_blink
            && self.focused
            && self.terminal_focused
            && !self.hidden
            && self
                .current_pane()
                .is_some_and(|p| p.cursor.blinking && p.cursor.visible && !p.exited)
    }
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Hotkey(..)
            | Message::WindowFocus(..)
            | Message::Present(..)
            | Message::Opened(..)
            | Message::Close(..)
            | Message::DragWindow => return self.window_message(message),
            Message::Link(..)
            | Message::TargetValue(..)
            | Message::Local
            | Message::Retry
            | Message::Connect => return self.connection_message(message),
            Message::NewWorkspace
            | Message::NewTab
            | Message::Split(..)
            | Message::SplitPane(..)
            | Message::Workspace(..)
            | Message::Tab(..)
            | Message::ContextMenu(..)
            | Message::DismissContextMenu
            | Message::Rename
            | Message::RenameWorkspace(..)
            | Message::RenameValue(..)
            | Message::RenameTab(..)
            | Message::RenameSubmit
            | Message::CancelRename
            | Message::AskClosePane
            | Message::AskClosePaneOf(..)
            | Message::AskCloseTab(..)
            | Message::AskCloseWorkspace
            | Message::CloseWorkspace(..)
            | Message::ConfirmClose
            | Message::CancelClose => return self.workspaces_message(message),
            Message::Bounds(..)
            | Message::FocusPane(..)
            | Message::PaneInput(..)
            | Message::Select(..)
            | Message::ScrollPane(..)
            | Message::Paste(..)
            | Message::Modifiers(..)
            | Message::HoverPane(..)
            | Message::Copy
            | Message::PasteClipboard
            | Message::Key(..) => return self.input_message(message),
            Message::Tick => {}
            Message::Quit => return iced::exit(),
            Message::ToggleSidebar => {
                self.sidebar_open = !self.sidebar_open;
                self.context_menu = None;
                if !self.sidebar_open && self.connection_open {
                    self.connection_open = false;
                    self.terminal_focused = true;
                }
            }
            Message::ToggleWorkspaces => {
                self.workspaces_open = !self.workspaces_open;
                self.context_menu = None;
            }
            Message::ToggleConnection => {
                self.connection_open = !self.connection_open;
                self.terminal_focused = !self.connection_open;
            }
            Message::ToggleMenu => self.menu_open = !self.menu_open,
            Message::Tailscale(message) => return self.tailscale_message(message),
            Message::Attachments(message) => return self.attachments_message(message),
            Message::Settings(message) => return self.settings_message(message),
            Message::Blink => self.blink_on = !self.blink_on,
            Message::LinkHover(target) => self.hyperlinks.set_hover(target),
            Message::OpenLink(target) => self.status = self.hyperlinks.follow(&target),
            Message::Splits(message) => self.split_message(message),
            Message::Search(message) => return self.search_message(message),
            Message::Palette(message) => return self.palette_message(message),
            Message::Run(command) => return self.run_command(command),
        }
        Task::none()
    }
}
