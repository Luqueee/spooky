use super::{App, Message};
use crate::{
    config::Osc52,
    features::attachments::{self},
    link::Event,
    terminal::new_clipboard_writes,
};
use iced::{Task, clipboard};
use spooky_core::{Request, Snapshot};

impl App {
    pub(super) fn connection_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Link(e) => match e {
                Event::Snapshot(g, mut s) if g == self.connection.generation() => {
                    if let Some(mut current) = self.snapshot.take() {
                        current.apply_update(s);
                        s = current;
                    }
                    if self
                        .connection
                        .needs_first_workspace(s.workspaces.is_empty())
                    {
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
                Event::Status(g, line) if g == self.connection.generation() => {
                    if let Some(line) = self.connection.status(line) {
                        self.status = line;
                    }
                }
                Event::Unreachable(g, reason, retrying) if g == self.connection.generation() => {
                    self.status = self.connection.failed_to_connect(reason, retrying);
                }
                Event::Connection(g, channel, online) if g == self.connection.generation() => {
                    let update = self.connection.channel(channel, online);
                    if update.forget_panes {
                        self.grids.clear();
                        self.clipboard_seen.clear();
                    }
                    if let Some(line) = update.status {
                        self.status = line;
                    }
                }
                Event::Failed(g, s) if g == self.connection.generation() => {
                    self.pending = None;
                    self.splits.failed();
                    self.status = s;
                }
                Event::Uploaded(g, pane, path) if g == self.connection.generation() => {
                    self.send(attachments::insert_path(pane, &path));
                    self.status = format!("Inserted file path: {path}");
                }
                Event::Found(g, result) if g == self.connection.generation() => {
                    self.search.found(result)
                }
                Event::Clipboard(g, text)
                    if g == self.connection.generation() && self.config.osc52 == Osc52::Write =>
                {
                    self.status = format!(
                        "A terminal program set the clipboard ({} bytes)",
                        text.len()
                    );
                    return clipboard::write(text);
                }
                _ => {}
            },
            Message::TargetValue(v) => {
                self.connection.set_target(v);
                self.terminal_focused = false;
            }
            Message::Local => {
                self.connection.use_local();
                return Task::done(Message::Connect);
            }
            Message::Retry => {
                self.connection.use_current();
                return Task::done(Message::Connect);
            }
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
                let (generation, target) = self.connection.reconnect();
                self.snapshot = None;
                self.workspace = None;
                self.tab = None;
                self.pane = None;
                self.agents_seen = None;
                self.pending = None;
                self.grids.clear();
                self.bounds.clear();
                self.selection = None;
                self.clipboard_seen.clear();
                self.focus_reported = None;
                self.search.reset();
                self.splits.reset();
                self.hyperlinks.set_hover(None);
                self.terminal_focused = true;
                if self.link.retarget(generation, &target) {
                    self.status = "Connecting…".into();
                } else {
                    self.status = "Connection queue full".into();
                }
            }
            _ => {}
        }
        Task::none()
    }
    /// Typing and live output need both the command and the display connection.
    pub(super) fn online(&self) -> bool {
        self.connection.online()
    }
    /// Panes whose program set the clipboard since this client last looked.
    pub(super) fn clipboard_writes(&mut self, snapshot: &Snapshot) -> Vec<u64> {
        if self.config.osc52 == Osc52::Off {
            return Vec::new();
        }
        new_clipboard_writes(&mut self.clipboard_seen, &snapshot.panes)
    }
}
