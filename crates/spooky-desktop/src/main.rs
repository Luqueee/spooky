//! Native GPU frontend for persistent Spooky sessions.

mod chrome;
mod divider;
mod link;
mod links;
#[cfg(target_os = "macos")]
mod macos_app;
#[cfg(target_os = "macos")]
mod macos_keyboard;
mod pointer;
mod shortcuts;
mod tailscale;
mod terminal;
mod view;

use iced::{Point, Rectangle, Size, Subscription, Task, clipboard, event, keyboard, window};
use link::{Channel, Command, Event, Link};
use shortcuts::Shortcut;
use spooky_core::{
    Axis, EVEN_SPLIT, FindDirection, Layout, Pane, Request, Snapshot, Tab, Workspace,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::{Duration, Instant},
};

/// What programs in a terminal may do to the clipboard with OSC 52.
#[derive(Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Osc52 {
    /// Programs may set the clipboard. Reading it is never allowed.
    Write,
    /// Clipboard requests from programs are ignored.
    Off,
}
#[derive(Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    font_size: f32,
    width: f32,
    height: f32,
    shortcut: String,
    /// Keep a Dock icon on macOS. By default Spooky runs without one, like other
    /// dropdown terminals, whenever the global shortcut is available to bring it back.
    dock_icon: bool,
    /// Whether a program's OSC 52 request may set the clipboard: `"write"` (the
    /// default; the footer says when it happens) or `"off"`.
    osc52: Osc52,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            width: 1440.0,
            height: 900.0,
            shortcut: "Ctrl+Shift+Space".into(),
            dock_icon: false,
            osc52: Osc52::Write,
        }
    }
}
impl Config {
    fn load() -> anyhow::Result<Self> {
        let home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .ok_or_else(|| anyhow::anyhow!("Cannot determine configuration directory"))?;
        let path = home.join("spooky/config.toml");
        let config: Self = match std::fs::read_to_string(&path) {
            Ok(s) => toml::from_str(&s).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(anyhow::anyhow!("{}: {e}", path.display())),
        };
        if !(8.0..=48.0).contains(&config.font_size)
            || !(400.0..=4096.0).contains(&config.width)
            || !(200.0..=2160.0).contains(&config.height)
        {
            anyhow::bail!(
                "Invalid font_size or window dimensions in {}",
                path.display()
            );
        }
        Ok(config)
    }
}
#[derive(Clone, Debug)]
enum Message {
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
    Drop(PathBuf),
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
    OpenTailscale,
    CloseTailscale,
    RefreshTailscale,
    TailscaleLoaded(Result<tailscale::Network, String>),
    TailscaleQuery(String),
    TailscaleUser(String),
    SelectTailscale(String),
    Upload,
    Picked(Option<PathBuf>),
    Capture,
    Captured(window::Screenshot, Rectangle),
    Saved(Result<String, String>),
    Bounds(u64, Rectangle),
    FocusPane(u64, Vec<u8>),
    PaneInput(u64, Vec<u8>),
    Select(u64, (u16, u16), (u16, u16)),
    ScrollPane(u64, i32),
    Paste(Option<String>),
    Blink,
    LinkHover(Option<String>),
    OpenLink(String),
    /// A split divider is at this window position.
    DragSplit(u64, Point),
    EndSplitDrag,
    ResetSplit(u64),
    SearchOpen,
    SearchInput(String),
    SearchStep(FindDirection),
    SearchClose,
    Modifiers(keyboard::Modifiers),
    /// The pointer entered (`true`) or left (`false`) this pane.
    HoverPane(u64, bool),
    Copy,
    PasteClipboard,
}
#[derive(Clone, Copy)]
enum Confirm {
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
enum Pending {
    Workspace(HashSet<u64>),
    Tab(u64, HashSet<u64>),
    Pane(HashSet<u64>),
}
#[derive(Clone, Copy)]
struct Selection {
    pane: u64,
    anchor: (u16, u16),
    head: (u16, u16),
}
/// The scrollback search strip.
#[derive(Default)]
struct Search {
    query: String,
    /// The pane the last search ran in, which holds the highlights.
    pane: Option<u64>,
    /// `None` until a search answered; then the match `(current, total)` or none.
    result: Option<Option<(u32, u32)>>,
}
struct App {
    config: Config,
    link: Link,
    generation: u64,
    snapshot: Option<Snapshot>,
    workspace: Option<u64>,
    tab: Option<u64>,
    pane: Option<u64>,
    terminal_focused: bool,
    selection: Option<Selection>,
    bounds: HashMap<u64, Rectangle>,
    grids: HashMap<u64, (u16, u16)>,
    target: String,
    connected_target: String,
    sidebar_open: bool,
    workspaces_open: bool,
    connection_open: bool,
    menu_open: bool,
    context_menu: Option<(ContextTarget, Point)>,
    rename_workspace: Option<u64>,
    rename_tab: Option<u64>,
    tailscale_open: bool,
    tailscale: tailscale::Discovery,
    tailscale_query: String,
    tailscale_user: String,
    rename: String,
    renaming: bool,
    status: String,
    confirm: Option<Confirm>,
    pending: Option<Pending>,
    window: Option<window::Id>,
    /// Hidden by Spooky itself (shortcut or close button), not minimized by the user.
    hidden: bool,
    focused: bool,
    initialized: bool,
    hotkey_id: Option<u32>,
    commands_online: bool,
    display_online: bool,
    /// Why the command connection cannot be opened, and whether it is being retried.
    unreachable: Option<(String, bool)>,
    /// Whether a blinking cursor is in its visible phase.
    blink_on: bool,
    /// Target of the link under the pointer, shown in the footer.
    hover_link: Option<String>,
    /// A divider's ratio as dragged, shown until the server confirms it.
    split_drag: Option<(u64, u16)>,
    split_dragging: bool,
    /// When a divider position was last sent to the server.
    split_sent: Option<Instant>,
    search: Option<Search>,
    /// Last OSC 52 write number seen per pane.
    clipboard_seen: HashMap<u64, u64>,
    /// The pane last told it has the keyboard focus.
    focus_reported: Option<u64>,
    /// Keyboard modifiers as last reported by the window.
    modifiers: keyboard::Modifiers,
    /// The pane under the pointer, which shows its quick actions.
    hover_pane: Option<u64>,
}
impl App {
    fn new(config: Config, status: String, hotkey_id: Option<u32>) -> (Self, Task<Message>) {
        (
            Self {
                config,
                link: Link::start(),
                generation: 0,
                snapshot: None,
                workspace: None,
                tab: None,
                pane: None,
                terminal_focused: true,
                selection: None,
                bounds: HashMap::new(),
                grids: HashMap::new(),
                target: String::new(),
                connected_target: String::new(),
                sidebar_open: true,
                workspaces_open: true,
                connection_open: false,
                menu_open: false,
                context_menu: None,
                rename_workspace: None,
                rename_tab: None,
                tailscale_open: false,
                tailscale: tailscale::Discovery::Idle,
                tailscale_query: String::new(),
                tailscale_user: String::new(),
                rename: String::new(),
                renaming: false,
                status,
                confirm: None,
                pending: None,
                window: None,
                hidden: false,
                focused: false,
                initialized: false,
                hotkey_id,
                commands_online: false,
                display_online: false,
                unreachable: None,
                blink_on: true,
                hover_link: None,
                split_drag: None,
                split_dragging: false,
                split_sent: None,
                search: None,
                clipboard_seen: HashMap::new(),
                focus_reported: None,
                modifiers: keyboard::Modifiers::default(),
                hover_pane: None,
            },
            window::latest().map(|id| id.map(Message::Opened).unwrap_or(Message::Tick)),
        )
    }
    fn send(&mut self, request: Request) {
        if self
            .link
            .tx
            .try_send(Command::Request(self.generation, request))
            .is_err()
        {
            self.status = "Connection queue full; command not sent".into();
        }
    }
    fn create_workspace(&mut self, name: String) {
        let cwd = if self.connected_target.is_empty() {
            let Some(home) =
                std::env::home_dir().and_then(|path| path.into_os_string().into_string().ok())
            else {
                self.pending = None;
                self.status = "Could not resolve the local home directory".into();
                return;
            };
            Some(home)
        } else {
            None
        };
        self.send(Request::CreateWorkspace { name, cwd });
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
    fn reconcile(&mut self, s: &Snapshot) {
        if self
            .context_menu
            .is_some_and(|(target, _)| !context_target_exists(s, target))
        {
            self.context_menu = None;
        }
        if self
            .hover_pane
            .is_some_and(|id| !s.panes.iter().any(|p| p.id == id))
        {
            self.hover_pane = None;
        }
        if self
            .rename_workspace
            .is_some_and(|id| !s.workspaces.iter().any(|w| w.id == id))
        {
            self.rename_workspace = None;
            self.renaming = false;
            self.terminal_focused = true;
        }
        if self.rename_tab.is_some_and(|id| {
            !s.workspaces
                .iter()
                .flat_map(|w| &w.tabs)
                .any(|t| t.id == id)
        }) {
            self.rename_tab = None;
            self.renaming = false;
            self.terminal_focused = true;
        }
        if matches!(self.confirm, Some(Confirm::Tab(id)) if !s.workspaces.iter().flat_map(|w| &w.tabs).any(|t| t.id == id))
        {
            self.confirm = None;
        }
        if matches!(self.confirm, Some(Confirm::Workspace(id)) if !s.workspaces.iter().any(|w| w.id == id))
        {
            self.confirm = None;
        }
        if let Some(pending) = self.pending.take() {
            match pending {
                Pending::Workspace(old) => {
                    if let Some(w) = s.workspaces.iter().find(|w| !old.contains(&w.id)) {
                        self.workspace = Some(w.id);
                        self.tab = w.tabs.first().map(|t| t.id);
                        self.pane = None;
                        self.terminal_focused = true;
                    } else {
                        self.pending = Some(Pending::Workspace(old));
                    }
                }
                Pending::Tab(wid, old) => {
                    if let Some(t) = s
                        .workspaces
                        .iter()
                        .find(|w| w.id == wid)
                        .and_then(|w| w.tabs.iter().find(|t| !old.contains(&t.id)))
                    {
                        self.workspace = Some(wid);
                        self.tab = Some(t.id);
                        self.pane = None;
                        self.terminal_focused = true;
                    } else {
                        self.pending = Some(Pending::Tab(wid, old));
                    }
                }
                Pending::Pane(old) => {
                    if let Some(p) = s.panes.iter().find(|p| !old.contains(&p.id)) {
                        self.pane = Some(p.id);
                        self.terminal_focused = true;
                    } else {
                        self.pending = Some(Pending::Pane(old));
                    }
                }
            }
        }
        let w = s
            .workspaces
            .iter()
            .find(|w| Some(w.id) == self.workspace)
            .or_else(|| s.workspaces.first());
        self.workspace = w.map(|w| w.id);
        let t = w.and_then(|w| {
            w.tabs
                .iter()
                .find(|t| Some(t.id) == self.tab)
                .or_else(|| w.tabs.first())
        });
        self.tab = t.map(|t| t.id);
        let mut ids = Vec::new();
        if let Some(t) = t {
            t.layout.panes(&mut ids);
        }
        if !self.pane.is_some_and(|p| ids.contains(&p)) {
            self.pane = ids.first().copied();
            self.selection = None;
        }
    }
    /// Carries out an application shortcut by sending the message its button would.
    fn run_shortcut(&self, shortcut: Shortcut) -> Task<Message> {
        let workspaces: Vec<u64> = self
            .snapshot
            .as_ref()
            .map(|s| s.workspaces.iter().map(|w| w.id).collect())
            .unwrap_or_default();
        let tabs: Vec<u64> = self
            .active()
            .map(|(w, _)| w.tabs.iter().map(|t| t.id).collect())
            .unwrap_or_default();
        let message = match shortcut {
            Shortcut::NewTab => Some(Message::NewTab),
            Shortcut::NewWorkspace => Some(Message::NewWorkspace),
            Shortcut::Split(axis) => Some(Message::Split(axis)),
            Shortcut::ClosePane => Some(Message::AskClosePane),
            Shortcut::CloseWorkspace => Some(Message::AskCloseWorkspace),
            Shortcut::Tab(position) => shortcuts::nth(&tabs, position).map(Message::Tab),
            Shortcut::NextTab => shortcuts::cycle(&tabs, self.tab, true).map(Message::Tab),
            Shortcut::PreviousTab => shortcuts::cycle(&tabs, self.tab, false).map(Message::Tab),
            Shortcut::Workspace(position) => {
                shortcuts::nth(&workspaces, position).map(Message::Workspace)
            }
            Shortcut::NextWorkspace => {
                shortcuts::cycle(&workspaces, self.workspace, true).map(Message::Workspace)
            }
            Shortcut::PreviousWorkspace => {
                shortcuts::cycle(&workspaces, self.workspace, false).map(Message::Workspace)
            }
            Shortcut::RenameTab => self.tab.map(Message::RenameTab),
            Shortcut::RenameWorkspace => Some(Message::Rename),
            Shortcut::ToggleSidebar => Some(Message::ToggleSidebar),
            Shortcut::Find => Some(Message::SearchOpen),
            Shortcut::FindNext => self
                .search
                .as_ref()
                .map(|_| Message::SearchStep(FindDirection::Next)),
            Shortcut::FindPrevious => self
                .search
                .as_ref()
                .map(|_| Message::SearchStep(FindDirection::Previous)),
            Shortcut::Quit => Some(Message::Quit),
        };
        message.map_or_else(Task::none, Task::done)
    }
    /// Takes the window off screen. Only offered while the global shortcut can bring it back.
    fn hide(&mut self, id: window::Id) -> Task<Message> {
        self.hidden = true;
        self.focused = false;
        window::set_mode(id, window::Mode::Hidden)
    }
    /// Measures the display and window, then shows the window via [`Message::Present`].
    fn reveal(&self, id: window::Id) -> Task<Message> {
        window::monitor_size(id).then(move |monitor| {
            window::size(id).map(move |size| Message::Present(id, monitor, size))
        })
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.sync_focus_report();
        task
    }
    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {}
            Message::Hotkey(key) => {
                if Some(key) == self.hotkey_id
                    && let Some(id) = self.window
                {
                    // The shortcut hides a window that is on screen and focused. A
                    // window that is hidden, minimized or behind another app is
                    // brought forward instead, so its real state decides, not a toggle.
                    return if !self.hidden && self.focused {
                        self.hide(id)
                    } else {
                        self.reveal(id)
                    };
                }
            }
            Message::WindowFocus(focused) => self.focused = focused,
            Message::Present(id, monitor, size) => {
                self.hidden = false;
                self.terminal_focused = true;
                let mut tasks = vec![
                    window::set_mode(id, window::Mode::Windowed),
                    window::minimize(id, false),
                    window::gain_focus(id),
                ];
                if let Some(at) = dropdown_origin(monitor, size) {
                    tasks.insert(0, window::move_to(id, at));
                }
                return Task::batch(tasks);
            }
            Message::Quit => return iced::exit(),
            Message::Link(e) => match e {
                Event::Snapshot(g, mut s) if g == self.generation => {
                    if let Some(mut current) = self.snapshot.take() {
                        current.apply_update(s);
                        s = current;
                    }
                    if s.workspaces.is_empty() && !self.initialized {
                        self.initialized = true;
                        self.create_workspace("Workspace 1".into());
                    }
                    self.reconcile(&s);
                    let fetch = self.clipboard_writes(&s);
                    self.snapshot = Some(s);
                    self.settle_split();
                    for pane in fetch {
                        self.send(Request::Clipboard { pane });
                    }
                }
                // While a target is unreachable the failure card owns the explanation;
                // the display thread's echo of it must not replace the footer text.
                Event::Status(g, s) if g == self.generation && self.unreachable.is_none() => {
                    self.status = s;
                }
                Event::Unreachable(g, reason, retrying) if g == self.generation => {
                    self.status = "Cannot connect".into();
                    self.unreachable = Some((reason, retrying));
                }
                Event::Connection(g, channel, online) if g == self.generation => {
                    match channel {
                        Channel::Commands => {
                            self.commands_online = online;
                            if online {
                                self.unreachable = None;
                            }
                        }
                        Channel::Display => {
                            self.display_online = online;
                            if online {
                                // A restarted server reuses pane IDs; sizes already
                                // sent to its predecessor say nothing about its panes.
                                self.grids.clear();
                                self.clipboard_seen.clear();
                            }
                        }
                    }
                    if online && self.online() {
                        self.status = "Connected".into();
                    }
                }
                Event::Failed(g, s) if g == self.generation => {
                    self.pending = None;
                    if !self.split_dragging {
                        self.split_drag = None;
                    }
                    self.status = s;
                }
                Event::Uploaded(g, p, path) if g == self.generation => {
                    self.send(Request::Input {
                        pane: p,
                        bytes: shell_quote(&path).into_bytes(),
                    });
                    self.status = format!("Inserted file path: {path}");
                }
                Event::Found(g, result) if g == self.generation => {
                    if let Some(search) = self.search.as_mut() {
                        search.result = Some(result);
                    }
                }
                Event::Clipboard(g, text)
                    if g == self.generation && self.config.osc52 == Osc52::Write =>
                {
                    self.status = format!(
                        "A terminal program set the clipboard ({} bytes)",
                        text.len()
                    );
                    return clipboard::write(text);
                }
                _ => {}
            },
            Message::Opened(id) => {
                self.window = Some(id);
                if self.hotkey_id.is_some() {
                    #[cfg(target_os = "macos")]
                    {
                        macos_app::show_windows_instantly();
                        if !self.config.dock_icon {
                            macos_app::hide_dock_icon();
                        }
                    }
                    return self.reveal(id);
                }
            }
            // The close button hides the window while the shortcut can bring it back;
            // without one, hiding would strand the app, so closing quits. Sessions
            // run on the server either way.
            Message::Close(id) => {
                return if self.hotkey_id.is_some() {
                    self.hide(id)
                } else {
                    window::close(id)
                };
            }
            Message::DragWindow => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            Message::NewWorkspace => {
                self.workspaces_open = true;
                let ids = self
                    .snapshot
                    .as_ref()
                    .map(|s| s.workspaces.iter().map(|w| w.id).collect())
                    .unwrap_or_default();
                let n = self.snapshot.as_ref().map_or(1, |s| s.workspaces.len() + 1);
                self.pending = Some(Pending::Workspace(ids));
                self.create_workspace(format!("Workspace {n}"));
            }
            Message::NewTab => {
                self.context_menu = None;
                if let Some((w, _)) = self.active() {
                    let id = w.id;
                    let ids = w.tabs.iter().map(|t| t.id).collect();
                    self.pending = Some(Pending::Tab(id, ids));
                    self.send(Request::CreateTab {
                        workspace: id,
                        title: "Shell".into(),
                    });
                }
            }
            Message::Split(axis) => {
                self.menu_open = false;
                self.context_menu = None;
                if let Some(pane) = self.pane {
                    return Task::done(Message::SplitPane(pane, axis));
                }
            }
            Message::SplitPane(pane, axis) => {
                self.menu_open = false;
                self.context_menu = None;
                if let Some(ids) = self
                    .snapshot
                    .as_ref()
                    .filter(|s| s.panes.iter().any(|p| p.id == pane))
                    .map(|s| s.panes.iter().map(|p| p.id).collect::<HashSet<u64>>())
                {
                    self.pending = Some(Pending::Pane(ids));
                    self.send(Request::Split { pane, axis });
                }
            }
            Message::Workspace(id) => {
                self.context_menu = None;
                self.connection_open = false;
                self.terminal_focused = true;
                self.workspace = Some(id);
                self.tab = None;
                self.pane = None;
                if let Some(s) = self.snapshot.take() {
                    self.reconcile(&s);
                    self.snapshot = Some(s);
                }
            }
            Message::Tab(id) => {
                self.context_menu = None;
                self.tab = Some(id);
                self.pane = None;
                if let Some(s) = self.snapshot.take() {
                    self.reconcile(&s);
                    self.snapshot = Some(s);
                }
            }
            Message::ContextMenu(target, position) => {
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| context_target_exists(s, target))
                {
                    if let ContextTarget::Pane(id) = target {
                        self.pane = Some(id);
                        self.terminal_focused = true;
                    }
                    self.context_menu = Some((target, position));
                    self.menu_open = false;
                }
            }
            Message::DismissContextMenu => self.context_menu = None,
            Message::Rename => {
                if let Some(id) = self.workspace {
                    return Task::done(Message::RenameWorkspace(id));
                }
            }
            Message::RenameWorkspace(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if let Some(w) = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.workspaces.iter().find(|w| w.id == id))
                {
                    self.rename = w.name.clone();
                    self.rename_workspace = Some(id);
                    self.renaming = true;
                    self.terminal_focused = false;
                    return iced::widget::operation::focus("workspace-name")
                        .chain(iced::widget::operation::select_all("workspace-name"));
                }
            }
            Message::RenameValue(v) => {
                self.rename = v;
                self.terminal_focused = false;
            }
            Message::RenameTab(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if let Some(title) = self
                    .snapshot
                    .as_ref()
                    .and_then(|s| {
                        s.workspaces
                            .iter()
                            .flat_map(|w| &w.tabs)
                            .find(|t| t.id == id)
                    })
                    .map(|t| self.tab_label(t))
                {
                    self.rename = title;
                    self.rename_tab = Some(id);
                    self.rename_workspace = None;
                    self.renaming = true;
                    self.terminal_focused = false;
                    return iced::widget::operation::focus("workspace-name")
                        .chain(iced::widget::operation::select_all("workspace-name"));
                }
            }
            Message::RenameSubmit => {
                if let Some(id) = self.rename_workspace.take() {
                    if !self.rename.trim().is_empty() {
                        self.send(Request::RenameWorkspace {
                            workspace: id,
                            name: self.rename.trim().to_owned(),
                        });
                    } else {
                        self.status = "Workspace name cannot be empty".into();
                    }
                } else if let Some(id) = self.rename_tab.take() {
                    if !self.rename.trim().is_empty() {
                        self.send(Request::RenameTab {
                            tab: id,
                            title: self.rename.trim().to_owned(),
                        });
                    } else {
                        self.status = "Tab name cannot be empty".into();
                    }
                }
                self.renaming = false;
                self.terminal_focused = true;
            }
            Message::CancelRename => {
                self.renaming = false;
                self.rename_workspace = None;
                self.rename_tab = None;
                self.terminal_focused = true;
            }
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
            Message::AskClosePane => {
                self.menu_open = false;
                self.context_menu = None;
                self.confirm = self.pane.map(Confirm::Pane);
            }
            Message::AskClosePaneOf(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.panes.iter().any(|p| p.id == id))
                {
                    self.confirm = Some(Confirm::Pane(id));
                }
            }
            Message::AskCloseTab(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if self.snapshot.as_ref().is_some_and(|s| {
                    s.workspaces
                        .iter()
                        .flat_map(|w| &w.tabs)
                        .any(|t| t.id == id)
                }) {
                    self.confirm = Some(Confirm::Tab(id));
                }
            }
            Message::AskCloseWorkspace => {
                self.menu_open = false;
                if let Some(id) = self.workspace {
                    return Task::done(Message::CloseWorkspace(id));
                }
            }
            Message::CloseWorkspace(id) => {
                self.context_menu = None;
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.workspaces.iter().any(|w| w.id == id))
                {
                    self.confirm = Some(Confirm::Workspace(id));
                }
            }
            Message::ConfirmClose => match self.confirm.take() {
                Some(Confirm::Pane(p)) => self.send(Request::ClosePane { pane: p }),
                Some(Confirm::Workspace(w)) => self.send(Request::CloseWorkspace { workspace: w }),
                Some(Confirm::Tab(t)) => self.send(Request::CloseTab { tab: t }),
                None => {}
            },
            Message::CancelClose => self.confirm = None,
            Message::TargetValue(v) => {
                self.target = v;
                self.terminal_focused = false;
            }
            Message::Local => {
                self.target.clear();
                return Task::done(Message::Connect);
            }
            Message::Retry => {
                self.target = self.connected_target.clone();
                return Task::done(Message::Connect);
            }
            Message::OpenTailscale => {
                self.tailscale_open = true;
                self.terminal_focused = false;
                if !matches!(self.tailscale, tailscale::Discovery::Loading) {
                    return Task::done(Message::RefreshTailscale);
                }
            }
            Message::CloseTailscale => {
                self.tailscale_open = false;
                self.terminal_focused = !self.connection_open;
            }
            Message::RefreshTailscale => {
                if !matches!(self.tailscale, tailscale::Discovery::Loading) {
                    self.tailscale = tailscale::Discovery::Loading;
                    return Task::perform(tailscale::discover(), Message::TailscaleLoaded);
                }
            }
            Message::TailscaleLoaded(result) => {
                self.tailscale = match result {
                    Ok(network) => tailscale::Discovery::Ready(network),
                    Err(error) => tailscale::Discovery::Failed(error),
                };
            }
            Message::TailscaleQuery(query) => self.tailscale_query = query,
            Message::TailscaleUser(user) => self.tailscale_user = user,
            Message::SelectTailscale(host) => {
                let valid = matches!(&self.tailscale, tailscale::Discovery::Ready(network)
                    if network.peers.iter().any(|peer| peer.host == host && peer.online));
                if valid {
                    let user = self.tailscale_user.trim();
                    if !user.is_empty()
                        && (user.starts_with('-')
                            || !user
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)))
                    {
                        self.status = "SSH user must contain only letters, numbers, dot, underscore or hyphen.".into();
                    } else {
                        self.target = if user.is_empty() {
                            host
                        } else {
                            format!("{user}@{host}")
                        };
                        self.tailscale_open = false;
                        self.connection_open = true;
                    }
                }
            }
            Message::Connect => {
                self.connection_open = false;
                self.tailscale_open = false;
                self.context_menu = None;
                self.rename_workspace = None;
                self.rename_tab = None;
                self.workspaces_open = true;
                self.menu_open = false;
                self.confirm = None;
                self.renaming = false;
                self.connected_target = self.target.trim().to_owned();
                self.generation += 1;
                self.snapshot = None;
                self.workspace = None;
                self.tab = None;
                self.pane = None;
                self.pending = None;
                self.initialized = false;
                self.commands_online = false;
                self.display_online = false;
                self.unreachable = None;
                self.grids.clear();
                self.bounds.clear();
                self.selection = None;
                self.clipboard_seen.clear();
                self.focus_reported = None;
                self.search = None;
                self.split_drag = None;
                self.split_dragging = false;
                self.hover_link = None;
                self.terminal_focused = true;
                if self.link.retarget(self.generation, self.target.trim()) {
                    self.status = "Connecting…".into();
                } else {
                    self.status = "Connection queue full".into();
                }
            }
            Message::Upload => {
                let pane = self.pane;
                if pane.is_some() {
                    return Task::perform(
                        async {
                            rfd::AsyncFileDialog::new()
                                .pick_file()
                                .await
                                .map(|f| f.path().to_path_buf())
                        },
                        Message::Picked,
                    );
                }
            }
            Message::Drop(path) | Message::Picked(Some(path)) => {
                if let Some(p) = self.pane {
                    if self
                        .link
                        .tx
                        .try_send(Command::Upload(self.generation, p, path))
                        .is_err()
                    {
                        self.status = "Upload queue full".into();
                    } else {
                        self.status = "Uploading file…".into();
                    }
                }
            }
            Message::Picked(None) => {}
            Message::Bounds(p, b) => {
                self.bounds.insert(p, b);
                let (cw, ch) = terminal::metrics(self.config.font_size);
                let cols = ((b.width / cw).floor() as u16).max(1);
                let rows = ((b.height / ch).floor() as u16).max(1);
                if self.grids.get(&p) != Some(&(cols, rows)) {
                    self.grids.insert(p, (cols, rows));
                    self.send(Request::Resize {
                        pane: p,
                        cols,
                        rows,
                    });
                }
            }
            Message::FocusPane(pane, report) => {
                self.pane = Some(pane);
                self.terminal_focused = true;
                self.selection = None;
                if !report.is_empty() {
                    self.send(Request::Input {
                        pane,
                        bytes: report,
                    });
                }
            }
            Message::PaneInput(pane, bytes) => {
                self.send(Request::Input { pane, bytes });
            }
            Message::Select(p, a, b) => {
                self.pane = Some(p);
                self.terminal_focused = true;
                self.selection = (a != b).then_some(Selection {
                    pane: p,
                    anchor: a,
                    head: b,
                });
            }
            Message::ScrollPane(p, lines) => {
                if lines != 0 {
                    self.send(Request::Scroll { pane: p, lines });
                }
            }
            Message::Capture => {
                if let (Some(id), Some(bounds)) = (
                    self.window,
                    self.pane.and_then(|p| self.bounds.get(&p).copied()),
                ) {
                    return window::screenshot(id).map(move |shot| Message::Captured(shot, bounds));
                } else {
                    self.status = "No visible pane to capture".into();
                }
            }
            Message::Captured(shot, b) => {
                return Task::perform(
                    async move {
                        let scale = shot.scale_factor;
                        let rect = Rectangle {
                            x: (b.x * scale).round().max(0.0) as u32,
                            y: (b.y * scale).round().max(0.0) as u32,
                            width: (b.width * scale).floor().max(1.0) as u32,
                            height: (b.height * scale).floor().max(1.0) as u32,
                        };
                        let cropped = shot.crop(rect).map_err(|e| e.to_string())?;
                        let file = rfd::AsyncFileDialog::new()
                            .set_file_name("spooky-pane.png")
                            .add_filter("PNG", &["png"])
                            .save_file()
                            .await
                            .ok_or_else(|| "Capture cancelled".to_owned())?;
                        image::save_buffer_with_format(
                            file.path(),
                            cropped.as_ref(),
                            cropped.size.width,
                            cropped.size.height,
                            image::ColorType::Rgba8,
                            image::ImageFormat::Png,
                        )
                        .map_err(|e| e.to_string())?;
                        Ok(file.path().display().to_string())
                    },
                    Message::Saved,
                );
            }
            Message::Saved(result) => {
                self.status = match result {
                    Ok(p) => format!("Saved capture: {p}"),
                    Err(e) => format!("Capture: {e}"),
                }
            }
            Message::Paste(Some(s)) => {
                if self.terminal_focused
                    && let Some(p) = self.current_pane()
                {
                    let id = p.id;
                    let bytes = paste_bytes(&s, p.bracketed_paste);
                    self.send(Request::Input { pane: id, bytes });
                }
            }
            Message::Paste(None) => {}
            Message::Modifiers(modifiers) => self.modifiers = modifiers,
            Message::HoverPane(id, inside) => {
                if inside {
                    self.hover_pane = Some(id);
                } else if self.hover_pane == Some(id) {
                    self.hover_pane = None;
                }
            }
            Message::Copy => {
                self.context_menu = None;
                if let Some(text) = self.selected_text() {
                    return clipboard::write(text);
                }
            }
            Message::PasteClipboard => {
                self.context_menu = None;
                return clipboard::read().map(Message::Paste);
            }
            Message::Blink => self.blink_on = !self.blink_on,
            Message::LinkHover(target) => self.hover_link = target,
            Message::OpenLink(target) => {
                self.hover_link = None;
                self.status = match links::open(&target) {
                    Ok(()) => format!("Opening {target}"),
                    Err(error) => error,
                };
            }
            Message::DragSplit(split, at) => self.drag_split(split, at),
            Message::EndSplitDrag => {
                self.split_dragging = false;
                if let Some((split, ratio)) = self.split_drag {
                    self.split_sent = Some(Instant::now());
                    self.send(Request::SetSplitRatio { split, ratio });
                }
            }
            Message::ResetSplit(split) => {
                self.split_dragging = false;
                self.split_drag = Some((split, EVEN_SPLIT));
                self.split_sent = Some(Instant::now());
                self.send(Request::SetSplitRatio {
                    split,
                    ratio: EVEN_SPLIT,
                });
            }
            Message::SearchOpen => {
                if self.pane.is_some() {
                    self.search.get_or_insert_with(Search::default);
                    self.terminal_focused = false;
                    return iced::widget::operation::focus("search-input")
                        .chain(iced::widget::operation::select_all("search-input"));
                }
            }
            Message::SearchInput(query) => {
                // A text field inserts the letter of Cmd+F and Cmd+G; that edit
                // is the chord, handled as a key press, not text.
                if shortcuts::primary(self.modifiers)
                    && self
                        .search
                        .as_ref()
                        .is_some_and(|search| is_chord_echo(&search.query, &query))
                {
                    return Task::none();
                }
                self.terminal_focused = false;
                self.find(&query, FindDirection::Previous);
            }
            Message::SearchStep(direction) => {
                if let Some(query) = self.search.as_ref().map(|search| search.query.clone()) {
                    self.find(&query, direction);
                }
            }
            Message::SearchClose => {
                if let Some(search) = self.search.take() {
                    if let Some(pane) = search.pane {
                        self.send(Request::Find {
                            pane,
                            query: String::new(),
                            direction: FindDirection::Next,
                        });
                    }
                    self.terminal_focused = true;
                }
            }
            Message::Key(key, physical, mods, typed, status) => {
                self.blink_on = true;
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && self.search.is_some()
                    && !(self.menu_open
                        || self.context_menu.is_some()
                        || self.tailscale_open
                        || self.renaming
                        || self.confirm.is_some()
                        || self.connection_open)
                {
                    return Task::done(Message::SearchClose);
                }
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && (self.menu_open
                        || self.context_menu.is_some()
                        || self.tailscale_open
                        || self.renaming
                        || self.confirm.is_some()
                        || self.connection_open)
                {
                    self.menu_open = false;
                    self.context_menu = None;
                    self.rename_workspace = None;
                    self.rename_tab = None;
                    if self.tailscale_open {
                        self.tailscale_open = false;
                        self.terminal_focused = !self.connection_open;
                        return Task::none();
                    }
                    self.renaming = false;
                    self.connection_open = false;
                    self.confirm = None;
                    self.terminal_focused = true;
                    return Task::none();
                }
                let chord = shortcuts::shortcut(&key, physical, mods);
                if chord == Some(Shortcut::Quit) {
                    return iced::exit();
                }
                // The search chords also work while the search field has the focus,
                // whether or not the field consumed the key.
                if !self.terminal_focused
                    && self.search.is_some()
                    && self.confirm.is_none()
                    && !(self.menu_open
                        || self.context_menu.is_some()
                        || self.tailscale_open
                        || self.renaming
                        || self.connection_open)
                    && let Some(
                        shortcut @ (Shortcut::Find | Shortcut::FindNext | Shortcut::FindPrevious),
                    ) = chord
                {
                    return self.run_shortcut(shortcut);
                }
                if status == event::Status::Ignored
                    && self.terminal_focused
                    && self.confirm.is_none()
                    && !self.menu_open
                    && !self.renaming
                    && self.context_menu.is_none()
                    && !self.tailscale_open
                {
                    if let Some(shortcut) = chord {
                        return self.run_shortcut(shortcut);
                    }
                    match clipboard_action(&key, mods) {
                        Some(ClipboardAction::Copy) => {
                            if let Some(s) = self.selected_text() {
                                return clipboard::write(s);
                            }
                        }
                        Some(ClipboardAction::Paste) => {
                            return clipboard::read().map(Message::Paste);
                        }
                        None => {
                            if let Some(p) = self.current_pane() {
                                let id = p.id;
                                let application_cursor = p.application_cursor;
                                let typed = terminal_text(&key, physical, mods, typed);
                                if let Some(bytes) =
                                    key_bytes(&key, mods, typed.as_deref(), application_cursor)
                                {
                                    self.send(Request::Input { pane: id, bytes });
                                }
                            }
                        }
                    }
                }
            }
        }
        Task::none()
    }
    fn selected_text(&self) -> Option<String> {
        let Selection {
            pane: id,
            anchor,
            head,
        } = self.selection?;
        if anchor == head {
            return None;
        }
        let pane = self.snapshot.as_ref()?.panes.iter().find(|p| p.id == id)?;
        Some(selection_text(pane, anchor, head))
    }
    /// Typing and live output need both the command and the display connection.
    fn online(&self) -> bool {
        self.commands_online && self.display_online
    }
    /// What a tab shows: its name, or its terminal's title while unnamed. The
    /// selected pane speaks for the selected tab.
    fn tab_label(&self, tab: &Tab) -> String {
        let pane = if Some(tab.id) == self.tab && self.pane.is_some_and(|p| tab.layout.contains(p))
        {
            self.pane
        } else {
            first_pane(&tab.layout)
        };
        let title = pane
            .and_then(|id| self.snapshot.as_ref()?.panes.iter().find(|p| p.id == id))
            .map(|pane| pane.title.as_str());
        label_for(tab, title)
    }
    /// Whether the selected pane's blinking cursor should be animating.
    fn blink_wanted(&self) -> bool {
        self.focused
            && self.terminal_focused
            && !self.hidden
            && self
                .current_pane()
                .is_some_and(|p| p.cursor.blinking && p.cursor.visible && !p.exited)
    }
    /// Tells programs that asked for it (DECSET 1004) when the keyboard focus
    /// reaches or leaves their pane.
    fn sync_focus_report(&mut self) {
        let desired = self
            .pane
            .filter(|_| self.focused && self.terminal_focused && !self.hidden);
        if desired == self.focus_reported {
            return;
        }
        let reports = {
            let panes = self
                .snapshot
                .as_ref()
                .map(|s| s.panes.as_slice())
                .unwrap_or_default();
            focus_reports(self.focus_reported, desired, |id| {
                panes
                    .iter()
                    .any(|p| p.id == id && p.focus_reporting && !p.exited)
            })
        };
        self.focus_reported = desired;
        for (pane, bytes) in reports {
            self.send(Request::Input {
                pane,
                bytes: bytes.to_vec(),
            });
        }
    }
    /// Panes whose program set the clipboard since this client last looked.
    fn clipboard_writes(&mut self, snapshot: &Snapshot) -> Vec<u64> {
        if self.config.osc52 == Osc52::Off {
            return Vec::new();
        }
        new_clipboard_writes(&mut self.clipboard_seen, &snapshot.panes)
    }
    /// The split's axis and the window area it occupies, both sides included.
    fn split_geometry(&self, split: u64) -> Option<(Axis, iced::Rectangle)> {
        let (_, tab) = self.active()?;
        let (axis, node) = find_split(&tab.layout, split)?;
        let mut ids = Vec::new();
        node.panes(&mut ids);
        let mut region: Option<iced::Rectangle> = None;
        for id in ids {
            let inner = self.bounds.get(&id)?;
            let outer = iced::Rectangle {
                x: inner.x - view::PANE_PADDING_X,
                y: inner.y - view::PANE_PADDING_Y,
                width: inner.width + 2.0 * view::PANE_PADDING_X,
                height: inner.height + 2.0 * view::PANE_PADDING_Y,
            };
            region = Some(region.map_or(outer, |region| region.union(&outer)));
        }
        Some((axis, region?))
    }
    /// Moves a divider to the pointer: shown at once, sent to the server at a
    /// gentler pace than the pointer moves.
    fn drag_split(&mut self, split: u64, at: Point) {
        let Some((axis, region)) = self.split_geometry(split) else {
            return;
        };
        let ratio = divider::ratio_at(region, axis, at);
        self.split_dragging = true;
        if self.split_drag == Some((split, ratio)) {
            return;
        }
        self.split_drag = Some((split, ratio));
        if self
            .split_sent
            .is_none_or(|sent| sent.elapsed() >= Duration::from_millis(40))
        {
            self.split_sent = Some(Instant::now());
            self.send(Request::SetSplitRatio { split, ratio });
        }
    }
    /// Drops the dragged ratio once the server shows it (or the split), or has
    /// had long enough to.
    fn settle_split(&mut self) {
        let Some((split, ratio)) = self.split_drag else {
            return;
        };
        if self.split_dragging {
            return;
        }
        let current = self
            .active()
            .and_then(|(_, tab)| find_split(&tab.layout, split))
            .and_then(|(_, node)| match node {
                Layout::Split { ratio, .. } => Some(*ratio),
                Layout::Pane(_) => None,
            });
        let overdue = self
            .split_sent
            .is_none_or(|sent| sent.elapsed() > Duration::from_secs(1));
        if current.is_none_or(|current| current == ratio) || overdue {
            self.split_drag = None;
        }
    }
    /// Searches the selected pane's scrollback and records what was asked.
    fn find(&mut self, query: &str, direction: FindDirection) {
        let Some(pane) = self.pane else {
            return;
        };
        let Some(search) = self.search.as_mut() else {
            return;
        };
        search.query = query.to_owned();
        search.result = None;
        let moved_from = search.pane.replace(pane).filter(|old| *old != pane);
        if let Some(old) = moved_from {
            self.send(Request::Find {
                pane: old,
                query: String::new(),
                direction,
            });
        }
        self.send(Request::Find {
            pane,
            query: query.to_owned(),
            direction,
        });
    }
}

/// Whether the thing a context menu was opened for is still in the snapshot.
fn context_target_exists(snapshot: &Snapshot, target: ContextTarget) -> bool {
    match target {
        ContextTarget::Workspace(id) => snapshot.workspaces.iter().any(|w| w.id == id),
        ContextTarget::Tab(id) => snapshot
            .workspaces
            .iter()
            .flat_map(|w| &w.tabs)
            .any(|t| t.id == id),
        ContextTarget::Pane(id) => snapshot.panes.iter().any(|p| p.id == id),
    }
}
/// Where a dropdown window of `window` size goes on a display of `monitor` size:
/// horizontally centred against the top edge. `None` when the display is unknown,
/// leaving placement to the system.
fn dropdown_origin(monitor: Option<Size>, window: Size) -> Option<Point> {
    let monitor = monitor?;
    Some(Point::new(
        ((monitor.width - window.width) / 2.0).max(0.0),
        0.0,
    ))
}
/// [`dropdown_origin`] as the window's starting position, so it is created where it
/// stays instead of appearing elsewhere and moving. Before the display is known it
/// falls back to the top-left corner.
fn initial_position(window: Size, monitor: Size) -> Point {
    dropdown_origin(Some(monitor), window).unwrap_or(Point::ORIGIN)
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Longest tab label, in characters.
const MAX_TAB_LABEL: usize = 28;

/// `text` cut to `max` characters, with an ellipsis when it was cut.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max - 1).chain(['…']).collect()
    }
}

/// A tab's label: the name the user gave it, or else its terminal's title
/// unless that is still the default.
fn label_for(tab: &Tab, terminal_title: Option<&str>) -> String {
    let title = match terminal_title {
        Some(title) if !tab.renamed && !title.is_empty() && title != spooky_core::DEFAULT_TITLE => {
            title
        }
        _ => &tab.title,
    };
    shorten(title, MAX_TAB_LABEL)
}

fn first_pane(layout: &Layout) -> Option<u64> {
    let mut ids = Vec::new();
    layout.panes(&mut ids);
    ids.first().copied()
}

/// Whether a text field's new content is its old content plus the one letter of
/// a search chord (Cmd+F or Cmd+G). The field inserts that letter even while the
/// chord modifier is held, so such an edit is the chord and not typing.
fn is_chord_echo(old: &str, new: &str) -> bool {
    new.strip_prefix(old).is_some_and(|extra| {
        let mut chars = extra.chars();
        matches!(
            (chars.next(), chars.next()),
            (Some(c), None) if matches!(c.to_ascii_lowercase(), 'f' | 'g')
        )
    })
}

/// The split with this identity, anywhere in `layout`, with its axis.
fn find_split(layout: &Layout, split: u64) -> Option<(Axis, &Layout)> {
    match layout {
        Layout::Pane(_) => None,
        Layout::Split {
            id,
            axis,
            first,
            second,
            ..
        } => {
            if *id == split {
                Some((*axis, layout))
            } else {
                find_split(first, split).or_else(|| find_split(second, split))
            }
        }
    }
}

/// Panes whose program stored something in the clipboard since `seen` was
/// last updated. A pane seen for the first time only sets the baseline, so
/// writes made before this client looked are not replayed into the clipboard.
fn new_clipboard_writes(seen: &mut HashMap<u64, u64>, panes: &[Pane]) -> Vec<u64> {
    seen.retain(|id, _| panes.iter().any(|pane| pane.id == *id));
    panes
        .iter()
        .filter(|pane| {
            matches!(seen.insert(pane.id, pane.clipboard_seq), Some(before) if pane.clipboard_seq > before)
        })
        .map(|pane| pane.id)
        .collect()
}

const FOCUS_IN: &[u8] = b"\x1b[I";
const FOCUS_OUT: &[u8] = b"\x1b[O";

/// What to send when the keyboard focus moves from pane `old` to pane `new`:
/// focus-out and focus-in reports, only to programs that asked for them.
fn focus_reports(
    old: Option<u64>,
    new: Option<u64>,
    reporting: impl Fn(u64) -> bool,
) -> Vec<(u64, &'static [u8])> {
    let mut reports = Vec::new();
    if old == new {
        return reports;
    }
    if let Some(id) = old
        && reporting(id)
    {
        reports.push((id, FOCUS_OUT));
    }
    if let Some(id) = new
        && reporting(id)
    {
        reports.push((id, FOCUS_IN));
    }
    reports
}

/// Text under a selection: one line per row, without the blank cells that pad
/// each row to the grid width.
fn selection_text(pane: &Pane, anchor: (u16, u16), head: (u16, u16)) -> String {
    let (lo, hi) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let mut out = String::new();
    for row in lo.0..=hi.0 {
        let start = if row == lo.0 { lo.1 } else { 0 };
        let end = if row == hi.0 {
            hi.1
        } else {
            pane.cols.saturating_sub(1)
        };
        if row > lo.0 {
            out.push('\n');
        }
        if let Some(line) = pane.lines.iter().find(|line| line.row == row) {
            out.push_str(&line.text_between(start, end));
        }
    }
    out
}

/// Bytes sent for pasted text. Bracketed paste drops ESC and ETX: pasted text
/// must not end the bracket early (running the rest as typed input) or interrupt
/// the foreground program.
fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if !bracketed {
        return text.as_bytes().to_vec();
    }
    let text = text.replace(['\x1b', '\x03'], "");
    format!("\x1b[200~{text}\x1b[201~").into_bytes()
}

#[derive(Debug, PartialEq, Eq)]
enum ClipboardAction {
    Copy,
    Paste,
}

/// Cmd+C/V and Ctrl+Shift+C/V. Plain Ctrl+C and Ctrl+V remain terminal input.
fn clipboard_action(key: &keyboard::Key, mods: keyboard::Modifiers) -> Option<ClipboardAction> {
    if !((mods.logo() && !mods.control()) || (mods.control() && mods.shift())) {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Character(c) if c.eq_ignore_ascii_case("c") => Some(ClipboardAction::Copy),
        keyboard::Key::Character(c) if c.eq_ignore_ascii_case("v") => Some(ClipboardAction::Paste),
        _ => None,
    }
}

fn terminal_text(
    key: &keyboard::Key,
    physical: keyboard::key::Physical,
    modifiers: keyboard::Modifiers,
    text: Option<String>,
) -> Option<String> {
    #[cfg(target_os = "macos")]
    if modifiers.alt()
        && !modifiers.control()
        && !modifiers.logo()
        && matches!(key, keyboard::Key::Character(_))
        && text.as_deref().is_none_or(str::is_empty)
    {
        return macos_keyboard::option_text(physical, modifiers);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (key, physical, modifiers);
    text
}

fn key_bytes(
    key: &keyboard::Key,
    mods: keyboard::Modifiers,
    typed: Option<&str>,
    app: bool,
) -> Option<Vec<u8>> {
    use keyboard::key::Named;
    if mods.logo() {
        return None;
    }
    let modifier =
        1 + u8::from(mods.shift()) + 2 * u8::from(mods.alt()) + 4 * u8::from(mods.control());
    let mut bytes = match key {
        keyboard::Key::Character(s) if mods.control() => {
            let c = s.chars().next()?.to_ascii_uppercase();
            match c {
                '@'..='_' => vec![(c as u8) & 0x1f],
                '?' => vec![0x7f],
                ' ' => vec![0],
                _ => return None,
            }
        }
        keyboard::Key::Character(s) => {
            // Dead keys without terminal text must not leak their unmodified base key.
            let text = if cfg!(target_os = "macos") {
                typed.filter(|text| !text.is_empty())?
            } else {
                typed.unwrap_or(s.as_str())
            };
            text.as_bytes().to_vec()
        }
        keyboard::Key::Named(named) => {
            let arrow = match named {
                Named::ArrowUp => Some('A'),
                Named::ArrowDown => Some('B'),
                Named::ArrowRight => Some('C'),
                Named::ArrowLeft => Some('D'),
                Named::Home => Some('H'),
                Named::End => Some('F'),
                _ => None,
            };
            if let Some(c) = arrow {
                if modifier > 1 {
                    format!("\x1b[1;{modifier}{c}").into_bytes()
                } else if app {
                    format!("\x1bO{c}").into_bytes()
                } else {
                    format!("\x1b[{c}").into_bytes()
                }
            } else {
                let code = match named {
                    Named::Insert => Some(2),
                    Named::Delete => Some(3),
                    Named::PageUp => Some(5),
                    Named::PageDown => Some(6),
                    Named::F5 => Some(15),
                    Named::F6 => Some(17),
                    Named::F7 => Some(18),
                    Named::F8 => Some(19),
                    Named::F9 => Some(20),
                    Named::F10 => Some(21),
                    Named::F11 => Some(23),
                    Named::F12 => Some(24),
                    _ => None,
                };
                if let Some(n) = code {
                    if modifier > 1 {
                        format!("\x1b[{n};{modifier}~").into_bytes()
                    } else {
                        format!("\x1b[{n}~").into_bytes()
                    }
                } else {
                    match named {
                        Named::Space if mods.control() => vec![0],
                        Named::Space => typed.unwrap_or(" ").as_bytes().to_vec(),
                        Named::Enter => b"\r".to_vec(),
                        Named::Backspace => vec![0x7f],
                        Named::Tab if mods.shift() => b"\x1b[Z".to_vec(),
                        Named::Tab => b"\t".to_vec(),
                        Named::Escape => vec![0x1b],
                        Named::F1 => b"\x1bOP".to_vec(),
                        Named::F2 => b"\x1bOQ".to_vec(),
                        Named::F3 => b"\x1bOR".to_vec(),
                        Named::F4 => b"\x1bOS".to_vec(),
                        _ => return None,
                    }
                }
            }
        }
        _ => return None,
    };
    if mods.alt()
        && (!cfg!(target_os = "macos") || mods.control())
        && matches!(
            key,
            keyboard::Key::Character(_) | keyboard::Key::Named(Named::Space)
        )
    {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}
fn background_events() -> impl iced::futures::Stream<Item = Message> {
    use iced::futures::SinkExt;
    iced::stream::channel(32, async |mut output| {
        while let Some(event) = link::next_event().await {
            let message = match event {
                Event::Hotkey(id) => Message::Hotkey(id),
                event => Message::Link(event),
            };
            if output.send(message).await.is_err() {
                return;
            }
        }
    })
}
fn subscription(app: &App) -> Subscription<Message> {
    let blink = if app.blink_wanted() {
        iced::time::every(Duration::from_millis(530)).map(|_| Message::Blink)
    } else {
        Subscription::none()
    };
    Subscription::batch([
        Subscription::run(background_events),
        blink,
        event::listen_with(|event, status, id| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                physical_key,
                text,
                ..
            }) if status == event::Status::Ignored
                || key == keyboard::Key::Named(keyboard::key::Named::Escape)
                || shortcuts::primary(modifiers) =>
            {
                Some(Message::Key(
                    key,
                    physical_key,
                    modifiers,
                    text.map(|s| s.to_string()),
                    status,
                ))
            }
            iced::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                Some(Message::Modifiers(modifiers))
            }
            iced::Event::Window(window::Event::Opened { .. }) => Some(Message::Opened(id)),
            iced::Event::Window(window::Event::Focused) => Some(Message::WindowFocus(true)),
            iced::Event::Window(window::Event::Unfocused) => Some(Message::WindowFocus(false)),
            iced::Event::Window(window::Event::CloseRequested) => Some(Message::Close(id)),
            iced::Event::Window(window::Event::FileDropped(path)) => Some(Message::Drop(path)),
            _ => None,
        }),
    ])
}
fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let shortcut: global_hotkey::hotkey::HotKey = config
        .shortcut
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid global shortcut: {e}"))?;
    let wayland = cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (manager, hotkey_id, status) = if wayland {
        (None,None,"Wayland: global shortcuts and positioning are unavailable; use your compositor's shortcut".to_owned())
    } else {
        match global_hotkey::GlobalHotKeyManager::new().and_then(|manager| {
            manager.register(shortcut)?;
            Ok(manager)
        }) {
            Ok(manager) => (
                Some(manager),
                Some(shortcut.id()),
                "Starting session…".into(),
            ),
            Err(e) => (None, None, format!("Global shortcut unavailable: {e}")),
        }
    };
    let size = Size::new(config.width, config.height);
    let result = iced::application(
        move || App::new(config.clone(), status.clone(), hotkey_id),
        App::update,
        App::view,
    )
    .title("Spooky")
    .subscription(subscription)
    .theme(|_: &App| chrome::theme())
    .default_font(iced::Font::MONOSPACE)
    .window(window::Settings {
        size,
        min_size: Some(Size::new(720.0, 440.0)),
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        // With the global shortcut the window starts hidden, and is shown (placed,
        // focused) by the same path as every later reveal.
        visible: hotkey_id.is_none(),
        position: window::Position::SpecificWith(initial_position),
        exit_on_close_request: false,
        ..Default::default()
    })
    .run();
    drop(manager);
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::{
        ClipboardAction, ContextTarget, MAX_TAB_LABEL, clipboard_action, context_target_exists,
        dropdown_origin, find_split, focus_reports, is_chord_echo, key_bytes, label_for,
        new_clipboard_writes, paste_bytes, selection_text,
    };
    use iced::{
        Point, Size,
        keyboard::{Key, Modifiers, key::Named},
    };
    use std::collections::HashMap;

    #[test]
    fn the_dropdown_is_centred_against_the_top_and_never_starts_off_screen() {
        let window = Size::new(1440.0, 900.0);
        assert_eq!(
            dropdown_origin(Some(Size::new(1920.0, 1080.0)), window),
            Some(Point::new(240.0, 0.0))
        );
        assert_eq!(
            dropdown_origin(Some(Size::new(1280.0, 800.0)), window),
            Some(Point::new(0.0, 0.0)),
            "a window wider than its display starts at the left edge"
        );
        assert_eq!(dropdown_origin(None, window), None);
    }

    #[test]
    fn space_reaches_the_shell_with_terminal_modifiers() {
        let cases: &[(Modifiers, Option<&[u8]>)] = &[
            (Modifiers::empty(), Some(b" ")),
            (Modifiers::SHIFT, Some(b" ")),
            (Modifiers::CTRL, Some(b"\0")),
            #[cfg(target_os = "macos")]
            (Modifiers::ALT, Some(b" ")),
            #[cfg(not(target_os = "macos"))]
            (Modifiers::ALT, Some(b"\x1b ")),
            (Modifiers::ALT | Modifiers::CTRL, Some(b"\x1b\0")),
            (Modifiers::LOGO, None),
        ];
        for &(modifiers, expected) in cases {
            for text in [None, Some(" ")] {
                assert_eq!(
                    key_bytes(&Key::Named(Named::Space), modifiers, text, false).as_deref(),
                    expected,
                    "space with {modifiers:?}, text {text:?}"
                );
            }
        }
    }

    #[test]
    fn composed_text_reaches_the_shell_unchanged() {
        assert_eq!(
            key_bytes(
                &Key::Named(Named::Space),
                Modifiers::empty(),
                Some("~"),
                false
            ),
            Some(b"~".to_vec())
        );
        assert_eq!(
            key_bytes(
                &Key::Character("n".into()),
                Modifiers::empty(),
                Some("ñ"),
                false
            ),
            Some("ñ".as_bytes().to_vec())
        );
        #[cfg(target_os = "macos")]
        {
            for base in ["ñ", "n", "e"] {
                assert_eq!(
                    key_bytes(&Key::Character(base.into()), Modifiers::ALT, None, false),
                    None,
                    "an uncommitted dead key must not reach the shell"
                );
            }
            assert_eq!(
                key_bytes(&Key::Named(Named::Space), Modifiers::ALT, Some("~"), false),
                Some(b"~".to_vec())
            );
            assert_eq!(
                key_bytes(
                    &Key::Character("2".into()),
                    Modifiers::ALT,
                    Some("@"),
                    false
                ),
                Some(b"@".to_vec())
            );
        }
    }

    #[test]
    fn a_tab_shows_its_terminal_title_until_the_user_names_it() {
        let tab = |renamed| spooky_core::Tab {
            id: 1,
            title: "Shell".into(),
            renamed,
            layout: spooky_core::Layout::Pane(1),
        };
        assert_eq!(label_for(&tab(false), Some("vim notes.md")), "vim notes.md");
        assert_eq!(label_for(&tab(false), Some("Terminal")), "Shell");
        assert_eq!(label_for(&tab(false), Some("")), "Shell");
        assert_eq!(label_for(&tab(false), None), "Shell");
        assert_eq!(label_for(&tab(true), Some("vim notes.md")), "Shell");
        let long = "x".repeat(60);
        let shown = label_for(&tab(false), Some(&long));
        assert_eq!(shown.chars().count(), MAX_TAB_LABEL);
        assert!(shown.ends_with('…'));
    }

    #[test]
    fn the_clipboard_is_set_only_by_writes_made_while_watching() {
        let panes = |seqs: &[(u64, u64)]| {
            seqs.iter()
                .map(|&(id, seq)| {
                    let mut pane = pane(&["x"]);
                    pane.id = id;
                    pane.clipboard_seq = seq;
                    pane
                })
                .collect::<Vec<_>>()
        };
        let mut seen = HashMap::new();
        // First sight is the baseline whatever its number: nothing is replayed.
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 5), (2, 0)])).is_empty());
        assert_eq!(
            new_clipboard_writes(&mut seen, &panes(&[(1, 5), (2, 1)])),
            [2]
        );
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 5), (2, 1)])).is_empty());
        assert_eq!(
            new_clipboard_writes(&mut seen, &panes(&[(1, 7), (2, 1)])),
            [1]
        );
        // A pane that went away is forgotten, so a new one with its number starts afresh.
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 7)])).is_empty());
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 7), (2, 1)])).is_empty());
    }

    #[test]
    fn focus_is_reported_only_to_programs_that_asked_for_it() {
        let asks = |id| id != 3;
        let (out, focus_in) = (&b"\x1b[O"[..], &b"\x1b[I"[..]);
        assert_eq!(
            focus_reports(Some(1), Some(2), asks),
            [(1, out), (2, focus_in)]
        );
        assert_eq!(focus_reports(None, Some(1), asks), [(1, focus_in)]);
        assert_eq!(focus_reports(Some(1), None, asks), [(1, out)]);
        assert_eq!(focus_reports(Some(1), Some(3), asks), [(1, out)]);
        assert!(focus_reports(Some(1), Some(1), asks).is_empty());
    }

    #[test]
    fn the_letter_a_search_chord_leaves_in_a_text_field_is_not_typing() {
        assert!(is_chord_echo("hit-", "hit-g"));
        assert!(is_chord_echo("hit-", "hit-F"));
        assert!(is_chord_echo("", "g"));
        assert!(!is_chord_echo("hit-", "hit-x"), "other letters are typing");
        assert!(!is_chord_echo("hit-", "hit-gg"), "pasting is not a chord");
        assert!(!is_chord_echo("hit-", "hit"), "deleting is not a chord");
        assert!(!is_chord_echo("a", "ba"), "only a trailing letter counts");
    }

    #[test]
    fn a_split_is_found_anywhere_in_the_layout_tree() {
        use spooky_core::{Axis, Layout};
        let inner = Layout::Split {
            id: 11,
            axis: Axis::Horizontal,
            ratio: 300,
            first: Box::new(Layout::Pane(2)),
            second: Box::new(Layout::Pane(3)),
        };
        let tree = Layout::Split {
            id: 10,
            axis: Axis::Vertical,
            ratio: 500,
            first: Box::new(Layout::Pane(1)),
            second: Box::new(inner),
        };
        assert_eq!(
            find_split(&tree, 10).map(|(axis, _)| axis),
            Some(Axis::Vertical)
        );
        let (axis, node) = find_split(&tree, 11).expect("nested split");
        assert_eq!(axis, Axis::Horizontal);
        assert!(matches!(node, Layout::Split { ratio: 300, .. }));
        assert!(find_split(&tree, 99).is_none());
        assert!(find_split(&tree, 1).is_none(), "a pane is not a split");
    }

    #[test]
    fn a_context_menu_needs_what_it_was_opened_for() {
        use spooky_core::{Layout, Snapshot, Tab, Workspace};
        let snapshot = Snapshot {
            version: 6,
            revision: 1,
            workspaces: vec![Workspace {
                id: 10,
                name: "work".into(),
                cwd: "/".into(),
                tabs: vec![Tab {
                    id: 20,
                    title: "Shell".into(),
                    renamed: false,
                    layout: Layout::Pane(1),
                }],
            }],
            panes: vec![pane(&["x"])],
        };
        for target in [
            ContextTarget::Workspace(10),
            ContextTarget::Tab(20),
            ContextTarget::Pane(1),
        ] {
            assert!(context_target_exists(&snapshot, target), "{target:?}");
        }
        for target in [
            ContextTarget::Workspace(20),
            ContextTarget::Tab(10),
            ContextTarget::Pane(2),
        ] {
            assert!(!context_target_exists(&snapshot, target), "{target:?}");
        }
    }

    fn pane(rows: &[&str]) -> spooky_core::Pane {
        let cols = rows
            .iter()
            .map(|row| row.chars().count())
            .max()
            .unwrap_or(0);
        let lines = rows
            .iter()
            .enumerate()
            .map(|(row, text)| spooky_core::Line {
                row: row as u16,
                runs: if text.trim_end().is_empty() {
                    Vec::new()
                } else {
                    vec![spooky_core::Run {
                        col: 0,
                        text: text.trim_end().to_owned(),
                        fg: [0; 3],
                        bg: [0; 3],
                        bold: false,
                        italic: false,
                        underline: false,
                        strikethrough: false,
                        width: 0,
                        link: None,
                    }]
                },
            })
            .collect();
        spooky_core::Pane {
            id: 1,
            title: String::new(),
            cols: cols as u16,
            rows: rows.len() as u16,
            lines,
            cursor: spooky_core::Cursor {
                row: 0,
                col: 0,
                visible: false,
                shape: spooky_core::CursorShape::Beam,
                blinking: false,
            },
            exited: false,
            bracketed_paste: false,
            application_cursor: false,
            mouse_tracking: spooky_core::MouseTracking::Off,
            mouse_encoding: spooky_core::MouseEncoding::Normal,
            alternate_scroll: false,
            history: 0,
            scrolled: 0,
            focus_reporting: false,
            clipboard_seq: 0,
            highlights: Vec::new(),
        }
    }

    #[test]
    fn copied_selection_drops_the_blank_padding_of_each_row() {
        let pane = pane(&["foo       ", "bar       "]);
        assert_eq!(selection_text(&pane, (0, 0), (1, 9)), "foo\nbar");
        assert_eq!(selection_text(&pane, (1, 2), (0, 0)), "foo\nbar");
        assert_eq!(selection_text(&pane, (0, 1), (0, 2)), "oo");
    }

    #[test]
    fn bracketed_paste_cannot_be_ended_or_interrupted_by_pasted_text() {
        assert_eq!(
            paste_bytes("a\x1b[201~rm -rf ~\n\x03", true),
            b"\x1b[200~a[201~rm -rf ~\n\x1b[201~"
        );
        assert_eq!(paste_bytes("a\x1b[b\n", false), b"a\x1b[b\n");
    }

    #[test]
    fn clipboard_chords_leave_plain_control_keys_for_the_terminal() {
        let c = Key::Character("c".into());
        let v = Key::Character("v".into());
        let cases = [
            (&c, Modifiers::LOGO, Some(ClipboardAction::Copy)),
            (
                &c,
                Modifiers::LOGO | Modifiers::SHIFT,
                Some(ClipboardAction::Copy),
            ),
            (
                &c,
                Modifiers::CTRL | Modifiers::SHIFT,
                Some(ClipboardAction::Copy),
            ),
            (&v, Modifiers::LOGO, Some(ClipboardAction::Paste)),
            (
                &v,
                Modifiers::CTRL | Modifiers::SHIFT,
                Some(ClipboardAction::Paste),
            ),
            (&c, Modifiers::CTRL, None),
            (&v, Modifiers::CTRL, None),
            (&c, Modifiers::empty(), None),
        ];
        for (key, modifiers, expected) in cases {
            assert_eq!(
                clipboard_action(key, modifiers),
                expected,
                "{key:?} with {modifiers:?}"
            );
        }
    }
}
