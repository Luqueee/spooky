use super::{App, Message};
use crate::{
    features::{
        attachments::{self},
        palette::{self},
        search::{self},
        settings::{self},
        splits::{self},
        tailscale,
        window::{Hotkeys, level_for},
    },
    link::Command,
    shortcuts,
};
use iced::{Task, window};
use std::time::Instant;

impl App {
    pub(super) fn close_settings(&mut self) {
        if self.settings.close() {
            self.terminal_focused = true;
        }
    }
    /// Carries out a message from the attach and capture controls.
    pub(super) fn attachments_message(&mut self, message: attachments::Message) -> Task<Message> {
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
                .try_send(Command::Upload(self.connection.generation(), pane, path))
                .is_ok();
            self.status = if queued {
                "Uploading file…".into()
            } else {
                "Upload queue full".into()
            };
        }
        outcome.task.map(Message::Attachments)
    }
    /// Carries out a message from the command palette; a chosen command runs after it closes.
    pub(super) fn palette_message(&mut self, message: palette::Message) -> Task<Message> {
        let outcome = self.palette.update(message);
        if let Some(focused) = outcome.terminal_focused {
            self.terminal_focused = focused;
        }
        let mut tasks = Vec::new();
        if outcome.focus_field {
            self.menu_open = false;
            self.context_menu = None;
            tasks.push(
                iced::widget::operation::focus("palette-input")
                    .chain(iced::widget::operation::select_all("palette-input")),
            );
        }
        if let Some(command) = outcome.run {
            tasks.push(self.run_command(command));
        }
        Task::batch(tasks)
    }
    /// Carries out a message from the Tailscale picker.
    pub(super) fn tailscale_message(&mut self, message: tailscale::Message) -> Task<Message> {
        match message {
            tailscale::Message::Open => self.terminal_focused = false,
            tailscale::Message::Close => self.terminal_focused = !self.connection_open,
            _ => {}
        }
        let (task, outcome) = self.tailscale.update(message);
        match outcome {
            Some(tailscale::Outcome::Chosen(destination)) => {
                self.connection.set_target(destination);
                self.connection_open = true;
            }
            Some(tailscale::Outcome::Refused(reason)) => self.status = reason,
            None => {}
        }
        task.map(Message::Tailscale)
    }
    /// Carries out a message from the settings feature.
    pub(super) fn settings_message(&mut self, message: settings::Message) -> Task<Message> {
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
    pub(super) fn apply_effects(&mut self, effects: Vec<settings::Effect>) -> Task<Message> {
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
                    crate::platform::macos::app::set_dock_icon(show);
                    #[cfg(not(target_os = "macos"))]
                    let _ = show;
                }
            }
        }
        Task::batch(tasks)
    }
    /// Carries out a message from a divider.
    pub(super) fn split_message(&mut self, message: splits::Message) {
        let mut splits = std::mem::take(&mut self.splits);
        let request = splits.update(message, |split| self.split_geometry(split), Instant::now());
        self.splits = splits;
        if let Some(request) = request {
            self.send(request);
        }
    }
    /// Drops a dragged ratio the server now shows.
    pub(super) fn settle_splits(&mut self) {
        let mut splits = std::mem::take(&mut self.splits);
        splits.settle(|split| self.split_ratio(split), Instant::now());
        self.splits = splits;
    }
    /// Carries out a message from the search strip.
    pub(super) fn search_message(&mut self, message: search::Message) -> Task<Message> {
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
