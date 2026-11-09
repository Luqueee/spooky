//! Native GPU frontend for persistent Spooky sessions.

mod chrome;
mod link;
#[cfg(target_os = "macos")]
mod macos_keyboard;
mod terminal;
mod view;

use iced::{Point, Rectangle, Size, Subscription, Task, clipboard, event, keyboard, window};
use link::{Command, Event, Link};
use spooky_core::{Axis, Pane, Request, Snapshot, Tab, Workspace};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

#[derive(Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    font_size: f32,
    width: f32,
    height: f32,
    shortcut: String,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 12.0,
            width: 1440.0,
            height: 900.0,
            shortcut: "Ctrl+Shift+Space".into(),
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
    DragWindow,
    Drop(PathBuf),
    NewWorkspace,
    NewTab,
    Split(Axis),
    Workspace(u64),
    Tab(u64),
    Rename,
    RenameValue(String),
    RenameSubmit,
    CancelRename,
    ToggleSidebar,
    ToggleConnection,
    ToggleMenu,
    AskClosePane,
    AskCloseWorkspace,
    ConfirmClose,
    CancelClose,
    TargetValue(String),
    Connect,
    Local,
    Upload,
    Picked(Option<PathBuf>),
    Capture,
    Captured(window::Screenshot, Rectangle),
    Saved(Result<String, String>),
    Bounds(u64, Rectangle),
    FocusPane(u64),
    Select(u64, (u16, u16), (u16, u16)),
    ScrollPane(u64, i32),
    Paste(Option<String>),
}
#[derive(Clone, Copy)]
enum Confirm {
    Pane(u64),
    Workspace(u64),
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
    connection_open: bool,
    menu_open: bool,
    rename: String,
    renaming: bool,
    status: String,
    confirm: Option<Confirm>,
    pending: Option<Pending>,
    window: Option<window::Id>,
    visible: bool,
    initialized: bool,
    hotkey_id: Option<u32>,
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
                terminal_focused: false,
                selection: None,
                bounds: HashMap::new(),
                grids: HashMap::new(),
                target: String::new(),
                connected_target: String::new(),
                sidebar_open: true,
                connection_open: false,
                menu_open: false,
                rename: String::new(),
                renaming: false,
                status,
                confirm: None,
                pending: None,
                window: None,
                visible: true,
                initialized: false,
                hotkey_id,
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
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {}
            Message::Hotkey(key) => {
                if Some(key) == self.hotkey_id {
                    self.visible = !self.visible;
                    if let Some(id) = self.window {
                        return if self.visible {
                            Task::batch([
                                window::minimize(id, false),
                                window::move_to(id, Point::new(40.0, 0.0)),
                                window::gain_focus(id),
                            ])
                        } else {
                            window::minimize(id, true)
                        };
                    }
                }
            }
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
                    self.snapshot = Some(s);
                }
                Event::Status(g, s) if g == self.generation => self.status = s,
                Event::Failed(g, s) if g == self.generation => {
                    self.pending = None;
                    self.status = s;
                }
                Event::Uploaded(g, p, path) if g == self.generation => {
                    self.send(Request::Input {
                        pane: p,
                        bytes: shell_quote(&path).into_bytes(),
                    });
                    self.status = format!("Inserted file path: {path}");
                }
                _ => {}
            },
            Message::Opened(id) => self.window = Some(id),
            Message::Close(id) => return window::close(id),
            Message::DragWindow => {
                if let Some(id) = self.window {
                    return window::drag(id);
                }
            }
            Message::NewWorkspace => {
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
                if let Some(p) = self.pane {
                    let ids = self
                        .snapshot
                        .as_ref()
                        .map(|s| s.panes.iter().map(|p| p.id).collect())
                        .unwrap_or_default();
                    self.pending = Some(Pending::Pane(ids));
                    self.send(Request::Split { pane: p, axis });
                }
            }
            Message::Workspace(id) => {
                self.workspace = Some(id);
                self.tab = None;
                self.pane = None;
                if let Some(s) = self.snapshot.take() {
                    self.reconcile(&s);
                    self.snapshot = Some(s);
                }
            }
            Message::Tab(id) => {
                self.tab = Some(id);
                self.pane = None;
                if let Some(s) = self.snapshot.take() {
                    self.reconcile(&s);
                    self.snapshot = Some(s);
                }
            }
            Message::Rename => {
                self.menu_open = false;
                if let Some((w, _)) = self.active() {
                    self.rename = w.name.clone();
                    self.renaming = true;
                    self.terminal_focused = false;
                }
            }
            Message::RenameValue(v) => {
                self.rename = v;
                self.terminal_focused = false;
            }
            Message::RenameSubmit => {
                if let Some(id) = self.workspace {
                    if !self.rename.trim().is_empty() {
                        self.send(Request::RenameWorkspace {
                            workspace: id,
                            name: self.rename.trim().to_owned(),
                        });
                    } else {
                        self.status = "Workspace name cannot be empty".into();
                    }
                }
                self.renaming = false;
                self.terminal_focused = true;
            }
            Message::CancelRename => {
                self.renaming = false;
                self.terminal_focused = true;
            }
            Message::ToggleSidebar => self.sidebar_open = !self.sidebar_open,
            Message::ToggleConnection => {
                self.connection_open = !self.connection_open;
                self.terminal_focused = !self.connection_open;
            }
            Message::ToggleMenu => self.menu_open = !self.menu_open,
            Message::AskClosePane => {
                self.menu_open = false;
                self.confirm = self.pane.map(Confirm::Pane);
            }
            Message::AskCloseWorkspace => {
                self.menu_open = false;
                self.confirm = self.workspace.map(Confirm::Workspace);
            }
            Message::ConfirmClose => match self.confirm.take() {
                Some(Confirm::Pane(p)) => self.send(Request::ClosePane { pane: p }),
                Some(Confirm::Workspace(w)) => self.send(Request::CloseWorkspace { workspace: w }),
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
            Message::Connect => {
                self.connection_open = false;
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
                self.grids.clear();
                self.bounds.clear();
                self.selection = None;
                self.terminal_focused = false;
                if self
                    .link
                    .tx
                    .try_send(Command::Target(self.generation, self.target.trim().into()))
                    .is_err()
                {
                    self.status = "Connection queue full".into();
                } else {
                    self.status = "Connecting…".into();
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
            Message::FocusPane(pane) => {
                self.pane = Some(pane);
                self.terminal_focused = true;
                self.selection = None;
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
                    let bytes = if p.bracketed_paste {
                        format!("\x1b[200~{s}\x1b[201~").into_bytes()
                    } else {
                        s.into_bytes()
                    };
                    self.send(Request::Input { pane: id, bytes });
                }
            }
            Message::Paste(None) => {}
            Message::Key(key, physical, mods, typed, status) => {
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && (self.menu_open || self.renaming || self.confirm.is_some())
                {
                    self.menu_open = false;
                    self.renaming = false;
                    self.confirm = None;
                    self.terminal_focused = true;
                    return Task::none();
                }
                if status == event::Status::Ignored
                    && self.terminal_focused
                    && self.confirm.is_none()
                    && !self.menu_open
                    && !self.renaming
                {
                    let copy = mods.shift()
                        && (mods.control() || mods.logo())
                        && matches!(
                            key.as_ref(),
                            keyboard::Key::Character("c") | keyboard::Key::Character("C")
                        );
                    let paste = mods.shift()
                        && (mods.control() || mods.logo())
                        && matches!(
                            key.as_ref(),
                            keyboard::Key::Character("v") | keyboard::Key::Character("V")
                        )
                        || mods.logo()
                            && matches!(
                                key.as_ref(),
                                keyboard::Key::Character("v") | keyboard::Key::Character("V")
                            );
                    if copy {
                        if let Some(s) = self.selected_text() {
                            return clipboard::write(s);
                        }
                    } else if paste {
                        return clipboard::read().map(Message::Paste);
                    } else if let Some(p) = self.current_pane() {
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
        Task::none()
    }
    fn selected_text(&self) -> Option<String> {
        let Selection {
            pane: id,
            anchor: a,
            head: b,
        } = self.selection?;
        if a == b {
            return None;
        }
        let pane = self.snapshot.as_ref()?.panes.iter().find(|p| p.id == id)?;
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for r in lo.0..=hi.0 {
            if r > lo.0 {
                out.push('\n');
            }
            let start = if r == lo.0 { lo.1 } else { 0 };
            let end = if r == hi.0 {
                hi.1
            } else {
                pane.cols.saturating_sub(1)
            };
            let mut col = start;
            for c in pane
                .cells
                .iter()
                .filter(|c| c.row == r && c.col >= start && c.col <= end)
            {
                while col < c.col {
                    out.push(' ');
                    col += 1;
                }
                out.push_str(&c.text);
                col = c.col.saturating_add(if c.wide { 2 } else { 1 });
            }
        }
        Some(out)
    }
}
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
fn subscription(_: &App) -> Subscription<Message> {
    Subscription::batch([
        Subscription::run(background_events),
        event::listen_with(|event, status, id| match event {
            iced::Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modifiers,
                physical_key,
                text,
                ..
            }) if status == event::Status::Ignored
                || key == keyboard::Key::Named(keyboard::key::Named::Escape) =>
            {
                Some(Message::Key(
                    key,
                    physical_key,
                    modifiers,
                    text.map(|s| s.to_string()),
                    status,
                ))
            }
            iced::Event::Window(window::Event::Opened { .. }) => Some(Message::Opened(id)),
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
        position: window::Position::Specific(Point::new(40.0, 0.0)),
        exit_on_close_request: false,
        ..Default::default()
    })
    .run();
    drop(manager);
    result.map_err(|e| anyhow::anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::key_bytes;
    use iced::keyboard::{Key, Modifiers, key::Named};

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
}
