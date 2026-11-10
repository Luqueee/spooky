//! Native GPU frontend for persistent Spooky sessions.

mod config;
mod features;
mod layout;
mod link;
mod platform;
mod shortcuts;
mod tabs;
mod terminal;
#[cfg(test)]
mod test_support;
mod ui;
mod view;

use config::{Config, Osc52, SizeMode};
use features::{
    attachments::{self, Attachments},
    hyperlinks::Hyperlinks,
    search::{self, Search},
    settings::{self, Settings},
    splits::{self, Splits},
    tailscale,
    window::{
        Hotkeys, StartError, dropdown_origin, hides_on_blur, initial_position, level_for,
        usable_area,
    },
};
use iced::{Point, Rectangle, Size, Subscription, Task, clipboard, event, keyboard, window};
use layout::{context_target_exists, find_split, first_pane};
use link::{Channel, Command, Event, Link};
use shortcuts::Shortcut;
use spooky_core::{Axis, FindDirection, Layout, Pane, Request, Snapshot, Tab, Workspace};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};
use tabs::label_for;
use terminal::{
    ClipboardAction, clipboard_action, focus_reports, key_bytes, new_clipboard_writes, paste_bytes,
    selection_text, terminal_text,
};

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
    Modifiers(keyboard::Modifiers),
    /// The pointer entered (`true`) or left (`false`) this pane.
    HoverPane(u64, bool),
    Copy,
    PasteClipboard,
    Settings(settings::Message),
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
    initialized: bool,
    commands_online: bool,
    display_online: bool,
    /// Why the command connection cannot be opened, and whether it is being retried.
    unreachable: Option<(String, bool)>,
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
}
impl App {
    fn new(config: Config, status: String, hotkeys: Option<Hotkeys>) -> (Self, Task<Message>) {
        let size_pending = config.size_mode() == SizeMode::Percent;
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
                tailscale: tailscale::Picker::default(),
                rename: String::new(),
                renaming: false,
                status,
                confirm: None,
                pending: None,
                window: None,
                hidden: false,
                focused: false,
                initialized: false,
                commands_online: false,
                display_online: false,
                unreachable: None,
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
            Shortcut::Find => Some(Message::Search(search::Message::Open)),
            Shortcut::FindNext => self
                .search
                .is_open()
                .then_some(Message::Search(search::Message::Step(FindDirection::Next))),
            Shortcut::FindPrevious => {
                self.search
                    .is_open()
                    .then_some(Message::Search(search::Message::Step(
                        FindDirection::Previous,
                    )))
            }
            Shortcut::Settings => Some(Message::Settings(settings::Message::Open)),
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
                if self.hotkeys.as_ref().is_some_and(|keys| keys.id() == key)
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
            Message::WindowFocus(focused) => {
                self.focused = focused;
                if !focused
                    && hides_on_blur(
                        self.config.on_blur,
                        self.hidden,
                        self.attachments.dialog_open(),
                        self.hotkeys.is_some(),
                    )
                    && let Some(id) = self.window
                {
                    return self.hide(id);
                }
            }
            Message::Present(id, monitor, size) => {
                self.hidden = false;
                if !self.settings.is_open() {
                    self.terminal_focused = true;
                }
                // The configured size is applied the first time the display is known
                // and after a setting changes it; a later reveal only re-centres, so a
                // window the user resized keeps its size.
                let target = match monitor {
                    Some(monitor) if self.size_pending => {
                        self.size_pending = false;
                        self.config.window_size(usable_area(monitor))
                    }
                    _ => size,
                };
                let mut tasks = vec![
                    window::set_mode(id, window::Mode::Windowed),
                    window::minimize(id, false),
                    window::gain_focus(id),
                ];
                if target != size {
                    tasks.insert(0, window::resize(id, target));
                }
                if let Some(at) = dropdown_origin(monitor, target) {
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
                    self.settle_splits();
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
                    self.splits.failed();
                    self.status = s;
                }
                Event::Uploaded(g, pane, path) if g == self.generation => {
                    self.send(attachments::insert_path(pane, &path));
                    self.status = format!("Inserted file path: {path}");
                }
                Event::Found(g, result) if g == self.generation => self.search.found(result),
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
                if self.hotkeys.is_some() {
                    #[cfg(target_os = "macos")]
                    {
                        platform::macos::app::show_windows_instantly();
                        platform::macos::app::set_dock_icon(self.config.dock_icon);
                    }
                    return self.reveal(id);
                }
            }
            // The close button hides the window while the shortcut can bring it back;
            // without one, hiding would strand the app, so closing quits. Sessions
            // run on the server either way.
            Message::Close(id) => {
                return if self.hotkeys.is_some() {
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
                self.close_settings();
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
                self.close_settings();
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
                self.close_settings();
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
                self.close_settings();
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
                self.close_settings();
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
            Message::Tailscale(message) => return self.tailscale_message(message),
            Message::Connect => {
                self.connection_open = false;
                self.tailscale.close();
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
                self.search.reset();
                self.splits.reset();
                self.hyperlinks.set_hover(None);
                self.terminal_focused = true;
                if self.link.retarget(self.generation, self.target.trim()) {
                    self.status = "Connecting…".into();
                } else {
                    self.status = "Connection queue full".into();
                }
            }
            Message::Attachments(message) => return self.attachments_message(message),
            Message::Bounds(p, b) => {
                self.bounds.insert(p, b);
                self.fit(p, b);
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
            Message::Settings(message) => return self.settings_message(message),
            Message::Blink => self.blink_on = !self.blink_on,
            Message::LinkHover(target) => self.hyperlinks.set_hover(target),
            Message::OpenLink(target) => self.status = self.hyperlinks.follow(&target),
            Message::Splits(message) => self.split_message(message),
            Message::Search(message) => return self.search_message(message),
            Message::Key(key, physical, mods, typed, status) => {
                self.blink_on = true;
                if self.settings.recording() {
                    let effects = self.settings.record(
                        &key,
                        physical,
                        mods,
                        &mut self.config,
                        &mut self.hotkeys,
                    );
                    return self.apply_effects(effects);
                }
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && self.search.is_open()
                    && !(self.menu_open
                        || self.context_menu.is_some()
                        || self.tailscale.is_open()
                        || self.renaming
                        || self.confirm.is_some()
                        || self.connection_open)
                {
                    return Task::done(Message::Search(search::Message::Close));
                }
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && (self.menu_open
                        || self.context_menu.is_some()
                        || self.tailscale.is_open()
                        || self.renaming
                        || self.confirm.is_some()
                        || self.connection_open)
                {
                    self.menu_open = false;
                    self.context_menu = None;
                    self.rename_workspace = None;
                    self.rename_tab = None;
                    if self.tailscale.is_open() {
                        self.tailscale.close();
                        self.terminal_focused = !self.connection_open;
                        return Task::none();
                    }
                    self.renaming = false;
                    self.connection_open = false;
                    self.confirm = None;
                    self.terminal_focused = true;
                    return Task::none();
                }
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && self.settings.is_open()
                {
                    self.close_settings();
                    return Task::none();
                }
                let chord = shortcuts::shortcut(&key, physical, mods);
                if chord == Some(Shortcut::Quit) {
                    return iced::exit();
                }
                // The search chords also work while the search field has the focus,
                // whether or not the field consumed the key.
                if !self.terminal_focused
                    && self.search.is_open()
                    && self.confirm.is_none()
                    && !(self.menu_open
                        || self.context_menu.is_some()
                        || self.tailscale.is_open()
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
                    && !self.tailscale.is_open()
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
        self.config.cursor_blink
            && self.focused
            && self.terminal_focused
            && !self.hidden
            && self
                .current_pane()
                .is_some_and(|p| p.cursor.blinking && p.cursor.visible && !p.exited)
    }
    /// Tells the server the grid a pane's area holds, when that changed.
    fn fit(&mut self, pane: u64, area: Rectangle) {
        let (cw, ch) = terminal::metrics(self.config.font_size);
        let cols = ((area.width / cw).floor() as u16).max(1);
        let rows = ((area.height / ch).floor() as u16).max(1);
        if self.grids.get(&pane) != Some(&(cols, rows)) {
            self.grids.insert(pane, (cols, rows));
            self.send(Request::Resize { pane, cols, rows });
        }
    }
    /// Refits every pane after the font changed: the areas stay, the grids do not.
    fn refit_grids(&mut self) {
        let areas: Vec<(u64, Rectangle)> = self
            .bounds
            .iter()
            .map(|(pane, area)| (*pane, *area))
            .collect();
        for (pane, area) in areas {
            self.fit(pane, area);
        }
    }
    fn close_settings(&mut self) {
        if self.settings.close() {
            self.terminal_focused = true;
        }
    }
    /// Carries out a message from the attach and capture controls.
    fn attachments_message(&mut self, message: attachments::Message) -> Task<Message> {
        let context = attachments::Context {
            pane: self.pane,
            window: self.window,
            pane_bounds: self.pane.and_then(|pane| self.bounds.get(&pane).copied()),
        };
        let outcome = self.attachments.update(message, context);
        if let Some(status) = outcome.status {
            self.status = status;
        }
        if let Some((pane, path)) = outcome.upload {
            let queued = self
                .link
                .tx
                .try_send(Command::Upload(self.generation, pane, path))
                .is_ok();
            self.status = if queued {
                "Uploading file…".into()
            } else {
                "Upload queue full".into()
            };
        }
        outcome.task.map(Message::Attachments)
    }
    /// Carries out a message from the Tailscale picker.
    fn tailscale_message(&mut self, message: tailscale::Message) -> Task<Message> {
        match message {
            tailscale::Message::Open => self.terminal_focused = false,
            tailscale::Message::Close => self.terminal_focused = !self.connection_open,
            _ => {}
        }
        let (task, outcome) = self.tailscale.update(message);
        match outcome {
            Some(tailscale::Outcome::Chosen(destination)) => {
                self.target = destination;
                self.connection_open = true;
            }
            Some(tailscale::Outcome::Refused(reason)) => self.status = reason,
            None => {}
        }
        task.map(Message::Tailscale)
    }
    /// Carries out a message from the settings feature.
    fn settings_message(&mut self, message: settings::Message) -> Task<Message> {
        match message {
            settings::Message::Open => {
                self.menu_open = false;
                self.context_menu = None;
                self.settings.open();
                self.terminal_focused = false;
                Task::none()
            }
            settings::Message::Close => {
                self.close_settings();
                Task::none()
            }
            settings::Message::Change(change) => {
                let effects = self
                    .settings
                    .update(change, &mut self.config, &mut self.hotkeys);
                self.apply_effects(effects)
            }
        }
    }
    /// Updates whatever shows a setting that changed.
    fn apply_effects(&mut self, effects: Vec<settings::Effect>) -> Task<Message> {
        let mut tasks = Vec::new();
        for effect in effects {
            match effect {
                settings::Effect::Refit => self.refit_grids(),
                settings::Effect::Resize => {
                    self.size_pending = true;
                    if let Some(id) = self.window {
                        tasks.push(self.reveal(id));
                    }
                }
                settings::Effect::OnBlur(on_blur) => {
                    if let Some(id) = self.window {
                        tasks.push(window::set_level(id, level_for(on_blur)));
                    }
                }
                settings::Effect::DockIcon(show) => {
                    #[cfg(target_os = "macos")]
                    platform::macos::app::set_dock_icon(show);
                    #[cfg(not(target_os = "macos"))]
                    let _ = show;
                }
            }
        }
        Task::batch(tasks)
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
    /// The ratio the selected tab's layout gives `split`.
    fn split_ratio(&self, split: u64) -> Option<u16> {
        let (_, tab) = self.active()?;
        match find_split(&tab.layout, split)?.1 {
            Layout::Split { ratio, .. } => Some(*ratio),
            Layout::Pane(_) => None,
        }
    }
    /// Carries out a message from a divider.
    fn split_message(&mut self, message: splits::Message) {
        let mut splits = std::mem::take(&mut self.splits);
        let request = splits.update(message, |split| self.split_geometry(split), Instant::now());
        self.splits = splits;
        if let Some(request) = request {
            self.send(request);
        }
    }
    /// Drops a dragged ratio the server now shows.
    fn settle_splits(&mut self) {
        let mut splits = std::mem::take(&mut self.splits);
        splits.settle(|split| self.split_ratio(split), Instant::now());
        self.splits = splits;
    }
    /// Carries out a message from the search strip.
    fn search_message(&mut self, message: search::Message) -> Task<Message> {
        let outcome = self
            .search
            .update(message, self.pane, shortcuts::primary(self.modifiers));
        for request in outcome.requests {
            self.send(request);
        }
        if let Some(focused) = outcome.terminal_focused {
            self.terminal_focused = focused;
        }
        if outcome.focus_field {
            return iced::widget::operation::focus("search-input")
                .chain(iced::widget::operation::select_all("search-input"));
        }
        Task::none()
    }
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
            iced::Event::Window(window::Event::FileDropped(path)) => {
                Some(Message::Attachments(attachments::Message::Dropped(path)))
            }
            _ => None,
        }),
    ])
}
impl settings::Shortcuts for Option<Hotkeys> {
    fn available(&self) -> bool {
        self.is_some()
    }
    fn rebind(&mut self, text: &str) -> Result<(), String> {
        self.as_mut()
            .ok_or_else(|| "The global shortcut is not available in this session".to_owned())?
            .rebind(text)
    }
}

fn main() -> anyhow::Result<()> {
    let config = Config::load()?;
    let wayland = cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some();
    let (hotkeys, status) = if wayland {
        (
            None,
            "Wayland: global shortcuts and positioning are unavailable; use your compositor's shortcut"
                .to_owned(),
        )
    } else {
        match Hotkeys::start(&config.shortcut) {
            Ok(hotkeys) => (Some(hotkeys), "Starting session…".to_owned()),
            Err(StartError::Invalid(error)) => {
                anyhow::bail!("Invalid global shortcut: {error}")
            }
            Err(StartError::Unavailable(error)) => {
                (None, format!("Global shortcut unavailable: {error}"))
            }
        }
    };
    let dropdown = hotkeys.is_some();
    let size = config.initial_size();
    let level = level_for(config.on_blur);
    // The application state owns the shortcut registration; the boot function hands
    // it over once.
    let hotkeys = std::cell::Cell::new(hotkeys);
    let result = iced::application(
        move || App::new(config.clone(), status.clone(), hotkeys.take()),
        App::update,
        App::view,
    )
    .title("Spooky")
    .subscription(subscription)
    .theme(|_: &App| ui::theme())
    .default_font(iced::Font::MONOSPACE)
    .window(window::Settings {
        size,
        min_size: Some(config::MIN_WINDOW),
        level,
        #[cfg(target_os = "macos")]
        platform_specific: window::settings::PlatformSpecific {
            title_hidden: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
        },
        // With the global shortcut the window starts hidden, and is shown (placed,
        // focused) by the same path as every later reveal.
        visible: !dropdown,
        position: window::Position::SpecificWith(initial_position),
        exit_on_close_request: false,
        ..Default::default()
    })
    .run();
    result.map_err(|e| anyhow::anyhow!("{e}"))
}
