use super::{App, Message, Selection};
use crate::{
    commands::Command,
    features::{attachments, palette},
    features::{
        search::{self},
        settings::{self},
    },
    shortcuts,
    terminal::{
        ClipboardAction, clipboard_action, focus_reports, key_bytes, paste_bytes, selection_text,
        terminal_text,
    },
};
use iced::{Task, clipboard, event, keyboard};
use spooky_core::{FindDirection, Request};

impl App {
    pub(super) fn input_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Bounds(p, b) => {
                self.bounds.insert(p, b);
                self.fit(p, b);
            }
            Message::FocusPane(pane, report) => {
                self.pane = Some(pane);
                self.terminal_focused = true;
                self.copy_pending = None;
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
                self.copy_pending = None;
                self.selection = (a != b).then_some(Selection {
                    pane: p,
                    anchor: a,
                    head: b,
                    all: false,
                });
            }
            Message::ScrollPane(p, lines) => {
                if lines != 0 {
                    self.send(Request::Scroll { pane: p, lines });
                }
            }
            Message::Paste(Some(s)) => {
                if self.terminal_focused
                    && let Some(p) = self.current_pane().filter(|p| p.agent.is_none())
                {
                    let id = p.id;
                    let bytes = paste_bytes(&s, p.bracketed_paste);
                    self.send(Request::Input { pane: id, bytes });
                }
            }
            Message::Paste(None) => {}
            Message::Modifiers(modifiers) => self.modifiers = modifiers,
            Message::Copy => {
                self.context_menu = None;
                if self
                    .selection
                    .is_some_and(|selection| selection.all && Some(selection.pane) == self.pane)
                {
                    self.copy_all();
                } else if let Some(text) = self.selected_text() {
                    self.copy_pending = None;
                    return clipboard::write(text);
                }
            }
            Message::SelectAll => {
                self.context_menu = None;
                if let Some(pane) = self.current_pane().filter(|pane| pane.agent.is_none()) {
                    self.selection = Some(Selection {
                        pane: pane.id,
                        anchor: (0, 0),
                        head: (0, 0),
                        all: true,
                    });
                }
            }
            Message::CopyAll => {
                self.context_menu = None;
                self.copy_all();
            }
            Message::ClearViewport => {
                self.context_menu = None;
                if let Some(pane) = self.current_pane().filter(|pane| pane.agent.is_none()) {
                    let id = pane.id;
                    self.selection = None;
                    self.copy_pending = None;
                    self.send(Request::ClearViewport { pane: id });
                }
            }
            Message::PasteClipboard => {
                self.context_menu = None;
                return clipboard::read().map(Message::Paste);
            }
            Message::Key(key, physical, mods, typed, status) => {
                self.blink_on = true;
                if self.directory_fallback.is_some() {
                    if key == keyboard::Key::Named(keyboard::key::Named::Escape) {
                        self.directory_fallback = None;
                    } else if shortcuts::shortcut(&key, physical, mods) == Some(Command::Quit) {
                        return iced::exit();
                    }
                    return Task::none();
                }
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
                if self.palette.is_open() {
                    use keyboard::{Key, key::Named};
                    let message = match key {
                        Key::Named(Named::Escape) => Some(palette::Message::Close),
                        Key::Named(Named::ArrowDown) => Some(palette::Message::Move(1)),
                        Key::Named(Named::ArrowUp) => Some(palette::Message::Move(-1)),
                        _ => match shortcuts::shortcut(&key, physical, mods) {
                            Some(Command::Palette) => Some(palette::Message::Toggle),
                            Some(Command::Quit) => return iced::exit(),
                            _ => None,
                        },
                    };
                    return message
                        .map_or_else(Task::none, |message| Task::done(Message::Palette(message)));
                }
                if key == keyboard::Key::Named(keyboard::key::Named::Escape)
                    && self.search.is_open()
                    && !(self.menu_open
                        || self.context_menu.is_some()
                        || self.directory_menu.is_some()
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
                        || self.directory_menu.is_some()
                        || self.tailscale.is_open()
                        || self.renaming
                        || self.confirm.is_some()
                        || self.connection_open)
                {
                    self.menu_open = false;
                    self.context_menu = None;
                    self.directory_menu = None;
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
                    && self.settings.is_shown()
                {
                    self.close_settings();
                    return Task::none();
                }
                let chord = shortcuts::shortcut(&key, physical, mods);
                if chord == Some(Command::Quit) {
                    return iced::exit();
                }
                // The search chords also work while the search field has the focus,
                // whether or not the field consumed the key.
                if !self.terminal_focused
                    && self.search.is_open()
                    && self.confirm.is_none()
                    && !(self.menu_open
                        || self.context_menu.is_some()
                        || self.directory_menu.is_some()
                        || self.tailscale.is_open()
                        || self.renaming
                        || self.connection_open)
                    && let Some(
                        command @ (Command::Find | Command::FindNext | Command::FindPrevious),
                    ) = chord
                {
                    return self.run_command(command);
                }
                if status == event::Status::Ignored
                    && self.terminal_focused
                    && self.confirm.is_none()
                    && !self.menu_open
                    && !self.renaming
                    && self.context_menu.is_none()
                    && self.directory_menu.is_none()
                    && !self.tailscale.is_open()
                {
                    if let Some(command) = chord {
                        return self.run_command(command);
                    }
                    match clipboard_action(&key, mods) {
                        Some(ClipboardAction::Copy) => return Task::done(Message::Copy),
                        Some(ClipboardAction::Paste) => {
                            return clipboard::read().map(Message::Paste);
                        }
                        None => {
                            if let Some(p) = self.current_pane().filter(|p| p.agent.is_none()) {
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
            _ => {}
        }
        Task::none()
    }
    /// Carries out a command by sending the message its button would.
    pub(super) fn run_command(&self, command: Command) -> Task<Message> {
        let workspaces: Vec<u64> = self
            .snapshot
            .as_ref()
            .map(|s| s.workspaces.iter().map(|w| w.id).collect())
            .unwrap_or_default();
        let tabs: Vec<u64> = self
            .active()
            .map(|(w, _)| w.tabs.iter().map(|t| t.id).collect())
            .unwrap_or_default();
        let message = match command {
            Command::NewTab => Some(Message::NewTab),
            Command::NewAgent => Some(Message::NewAgent),
            Command::NewAgentHere => Some(Message::NewAgentHere),
            Command::NewWorkspace => Some(Message::NewWorkspace),
            Command::Split(axis) => Some(Message::Split(axis)),
            Command::TogglePaneZoom => self.pane.map(Message::TogglePaneZoom),
            Command::ClosePane => Some(Message::AskClosePane),
            Command::CloseWorkspace => Some(Message::AskCloseWorkspace),
            Command::Tab(position) => shortcuts::nth(&tabs, position).map(Message::Tab),
            Command::NextTab => shortcuts::cycle(&tabs, self.tab, true).map(Message::Tab),
            Command::PreviousTab => shortcuts::cycle(&tabs, self.tab, false).map(Message::Tab),
            Command::Workspace(position) => {
                shortcuts::nth(&workspaces, position).map(Message::Workspace)
            }
            Command::NextWorkspace => {
                shortcuts::cycle(&workspaces, self.workspace, true).map(Message::Workspace)
            }
            Command::PreviousWorkspace => {
                shortcuts::cycle(&workspaces, self.workspace, false).map(Message::Workspace)
            }
            Command::RenameTab => self.tab.map(Message::RenameTab),
            Command::RenameWorkspace => Some(Message::Rename),
            Command::ToggleSidebar => Some(Message::ToggleSidebar),
            Command::Find => Some(Message::Search(search::Message::Open)),
            Command::FindNext => self
                .search
                .is_open()
                .then_some(Message::Search(search::Message::Step(FindDirection::Next))),
            Command::FindPrevious => {
                self.search
                    .is_open()
                    .then_some(Message::Search(search::Message::Step(
                        FindDirection::Previous,
                    )))
            }
            Command::Settings => Some(Message::Settings(settings::Message::Open)),
            Command::Quit => Some(Message::Quit),
            Command::Palette => Some(Message::Palette(palette::Message::Toggle)),
            Command::Attach => Some(Message::Attachments(attachments::Message::Attach)),
            Command::Capture => Some(Message::Attachments(attachments::Message::Capture)),
            Command::Copy => Some(Message::Copy),
            Command::SelectAll => Some(Message::SelectAll),
            Command::CopyAll => Some(Message::CopyAll),
            Command::ClearViewport => Some(Message::ClearViewport),
            Command::Paste => Some(Message::PasteClipboard),
            Command::Connect => {
                // The panel lives in the sidebar, so it has to be showing.
                return if self.sidebar_open {
                    Task::done(Message::ToggleConnection)
                } else {
                    Task::done(Message::ToggleSidebar).chain(Task::done(Message::ToggleConnection))
                };
            }
        };
        message.map_or_else(Task::none, Task::done)
    }
    fn copy_all(&mut self) {
        let Some(pane) = self
            .current_pane()
            .filter(|pane| pane.agent.is_none())
            .map(|pane| pane.id)
        else {
            return;
        };
        if !self.online() {
            self.status = "Not connected; copy not sent".into();
            return;
        }
        if self.copy_pending.is_some() {
            self.status = "A copy is already running".into();
            return;
        }
        self.copy_epoch = self.copy_epoch.wrapping_add(1);
        let generation = self.connection.generation();
        let epoch = self.copy_epoch;
        if self
            .link
            .tx
            .try_send(crate::link::Command::CopyAll(generation, pane, epoch))
            .is_ok()
        {
            self.copy_pending = Some((generation, pane, epoch));
        } else {
            self.status = "Connection queue full; copy not sent".into();
        }
    }
    pub(super) fn selected_text(&self) -> Option<String> {
        let Selection {
            pane: id,
            anchor,
            head,
            all,
        } = self.selection?;
        if all || anchor == head {
            return None;
        }
        let pane = self.snapshot.as_ref()?.panes.iter().find(|p| p.id == id)?;
        Some(selection_text(pane, anchor, head))
    }
    /// Tells programs that asked for it (DECSET 1004) when the keyboard focus
    /// reaches or leaves their pane.
    pub(super) fn sync_focus_report(&mut self) {
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
}
