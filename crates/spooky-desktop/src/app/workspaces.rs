use super::{App, Confirm, ContextTarget, Message, Pending};
use crate::features::agent;
use crate::{
    layout::{context_target_exists, first_pane},
    tabs::label_for,
};
use iced::Task;
use spooky_core::{Request, Snapshot, Tab};
use std::collections::HashSet;

impl App {
    pub(super) fn workspaces_message(&mut self, message: Message) -> Task<Message> {
        match message {
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
            Message::NewAgent => {
                self.close_settings();
                self.context_menu = None;
                self.menu_open = false;
                if let Some((w, _)) = self.active() {
                    let id = w.id;
                    let ids = w.tabs.iter().map(|t| t.id).collect();
                    self.pending = Some(Pending::Tab(id, ids));
                    self.send(Request::CreateAgent {
                        workspace: id,
                        backend: agent::BACKEND.into(),
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
            _ => {}
        }
        Task::none()
    }
    pub(super) fn create_workspace(&mut self, name: String) {
        let cwd = if !self.connection.is_remote() {
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
    pub(super) fn reconcile(&mut self, s: &Snapshot) {
        self.agents.sync(
            s.panes
                .iter()
                .filter_map(|pane| Some((pane.id, pane.agent.as_ref()?))),
        );
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
        // An agent opened from a shell (typing `omp` there) comes to the front.
        let agents_now: HashSet<u64> = s
            .panes
            .iter()
            .filter(|p| p.agent.is_some())
            .map(|p| p.id)
            .collect();
        if let Some(seen) = &self.agents_seen
            && let Some(new) = agents_now.iter().find(|id| !seen.contains(id)).copied()
            && let Some((workspace, tab)) = s.workspaces.iter().find_map(|w| {
                w.tabs
                    .iter()
                    .find(|t| t.layout.contains(new))
                    .map(|t| (w.id, t.id))
            })
            && Some(workspace) == self.workspace
        {
            self.tab = Some(tab);
            self.pane = Some(new);
        }
        self.agents_seen = Some(agents_now);
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
    /// What a tab shows: its name, or its terminal's title while unnamed. The
    /// selected pane speaks for the selected tab.
    pub(super) fn tab_label(&self, tab: &Tab) -> String {
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
}
