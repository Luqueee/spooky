use super::{App, Confirm, ContextTarget, Message, Pending};
use crate::features::agent;
use crate::{
    layout::{context_target_exists, first_pane},
    tabs::label_for,
};
use iced::Task;
use spooky_core::{Request, Snapshot, Tab, Workspace};
use std::collections::HashSet;

impl App {
    pub(super) fn workspaces_message(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::NewWorkspace => {
                self.hide_settings();
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
                let source = self.pane;
                self.hide_settings();
                self.context_menu = None;
                if let Some(w) = self.active_workspace() {
                    let id = w.id;
                    let ids = w.tabs.iter().map(|t| t.id).collect();
                    self.pending = Some(Pending::Tab(id, ids));
                    self.send(Request::CreateTab {
                        workspace: id,
                        source,
                        title: "Shell".into(),
                    });
                }
            }
            Message::NewAgent | Message::NewAgentHere => {
                let here = matches!(message, Message::NewAgentHere);
                let source = if here {
                    let Some(pane) = self.pane else {
                        return Task::none();
                    };
                    Some(pane)
                } else {
                    None
                };
                self.hide_settings();
                self.context_menu = None;
                self.menu_open = false;
                if let Some(w) = self.active_workspace() {
                    let id = w.id;
                    let ids = w.tabs.iter().map(|t| t.id).collect();
                    self.pending = Some(Pending::Tab(id, ids));
                    self.send(Request::CreateAgent {
                        workspace: id,
                        source,
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
                self.hide_settings();
                self.menu_open = false;
                self.context_menu = None;
                if let Some(ids) = self
                    .snapshot
                    .as_ref()
                    .filter(|s| s.panes.iter().any(|p| p.id == pane))
                    .map(|s| s.panes.iter().map(|p| p.id).collect::<HashSet<u64>>())
                {
                    self.pending = Some(Pending::Pane(ids));
                    self.send(Request::Split {
                        pane,
                        axis,
                        here: true,
                    });
                }
            }
            Message::TogglePaneZoom(pane) => {
                self.menu_open = false;
                self.context_menu = None;
                if let Some((_, tab)) = self.active()
                    && matches!(tab.layout, spooky_core::Layout::Split { .. })
                    && tab.layout.contains(pane)
                {
                    let target = (tab.id, pane);
                    self.zoomed = (self.zoomed != Some(target)).then_some(target);
                    self.pane = Some(pane);
                    self.terminal_focused = true;
                    self.selection = None;
                    self.splits.reset();
                    if self.current_pane().is_some_and(|p| p.agent.is_some()) {
                        return iced::widget::operation::focus(agent::FIELD);
                    }
                }
            }
            Message::Workspace(id) => {
                self.hide_settings();
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
                self.hide_settings();
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
                if let Some(id) = self.pane {
                    return Task::done(Message::AskClosePaneOf(id));
                }
            }
            Message::AskClosePaneOf(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if self.online() {
                    self.status = "Connected".into();
                }
                if self
                    .snapshot
                    .as_ref()
                    .is_some_and(|s| s.panes.iter().any(|p| p.id == id))
                {
                    self.send(Request::ClosePane { pane: id });
                }
            }
            Message::AskCloseTab(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if self.online() {
                    self.status = "Connected".into();
                }
                if self.snapshot.as_ref().is_some_and(|s| {
                    s.workspaces
                        .iter()
                        .flat_map(|w| &w.tabs)
                        .any(|t| t.id == id)
                }) {
                    self.send(Request::CloseTab { tab: id });
                }
            }
            Message::ToggleTabLock(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if self.online() {
                    self.status = "Connected".into();
                }
                if let Some(tab) = self.snapshot.as_ref().and_then(|s| {
                    s.workspaces
                        .iter()
                        .flat_map(|w| &w.tabs)
                        .find(|t| t.id == id)
                }) {
                    self.send(Request::SetTabLocked {
                        tab: id,
                        locked: !tab.locked,
                    });
                }
            }
            Message::TogglePaneLock(id) => {
                self.menu_open = false;
                self.context_menu = None;
                if self.online() {
                    self.status = "Connected".into();
                }
                if let Some(tab) = self.snapshot.as_ref().and_then(|s| {
                    s.workspaces
                        .iter()
                        .flat_map(|w| &w.tabs)
                        .find(|t| t.layout.contains(id))
                }) {
                    self.send(Request::SetPaneLocked {
                        pane: id,
                        locked: !tab.locked_panes.contains(&id),
                    });
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
            Message::ConfirmClose => {
                if let Some(Confirm::Workspace(w)) = self.confirm.take() {
                    self.send(Request::CloseWorkspace { workspace: w });
                }
            }
            Message::CancelClose => self.confirm = None,
            Message::UseWorkspaceDirectory => {
                if let Some((request, workspace, _)) = self.directory_fallback.take() {
                    if !self.online() || self.fallback_workspace(&request, workspace).is_none() {
                        self.status =
                            "The original pane or connection is no longer available".into();
                    } else {
                        self.status = "Using the workspace's starting directory".into();
                        match request {
                            Request::CreateTab {
                                workspace, title, ..
                            } => {
                                if let Some(w) = self
                                    .snapshot
                                    .as_ref()
                                    .and_then(|s| s.workspaces.iter().find(|w| w.id == workspace))
                                {
                                    self.pending = Some(Pending::Tab(
                                        workspace,
                                        w.tabs.iter().map(|t| t.id).collect(),
                                    ));
                                    self.send(Request::CreateTab {
                                        workspace,
                                        source: None,
                                        title,
                                    });
                                }
                            }
                            Request::Split { pane, axis, .. } => {
                                if let Some(s) = &self.snapshot {
                                    self.pending =
                                        Some(Pending::Pane(s.panes.iter().map(|p| p.id).collect()));
                                    self.send(Request::Split {
                                        pane,
                                        axis,
                                        here: false,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            Message::CancelWorkspaceDirectory => self.directory_fallback = None,
            _ => {}
        }
        Task::none()
    }
    /// The failed creation's original workspace, only while its source still belongs
    /// to it. A split of an agent cannot succeed even with a different directory.
    pub(super) fn fallback_workspace(
        &self,
        request: &Request,
        workspace: u64,
    ) -> Option<&Workspace> {
        fallback_workspace(self.snapshot.as_ref()?, request, workspace)
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

/// A retry is valid only for the original pane and workspace still in the snapshot.
fn fallback_workspace<'a>(
    snapshot: &'a Snapshot,
    request: &Request,
    workspace: u64,
) -> Option<&'a Workspace> {
    let (pane, split) = match request {
        Request::CreateTab {
            workspace: source_workspace,
            source: Some(pane),
            ..
        } if *source_workspace == workspace => (*pane, false),
        Request::Split {
            pane, here: true, ..
        } => (*pane, true),
        _ => return None,
    };
    let source = snapshot.panes.iter().find(|p| p.id == pane)?;
    if split && source.agent.is_some() {
        return None;
    }
    snapshot
        .workspaces
        .iter()
        .find(|w| w.id == workspace && w.tabs.iter().any(|tab| tab.layout.contains(pane)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use spooky_core::{Agent, Axis, Layout, PROTOCOL_VERSION};

    fn snapshot() -> Snapshot {
        Snapshot {
            version: PROTOCOL_VERSION,
            revision: 1,
            workspaces: vec![
                Workspace {
                    id: 10,
                    name: "one".into(),
                    cwd: "/one".into(),
                    tabs: vec![Tab {
                        id: 11,
                        title: "shell".into(),
                        renamed: false,
                        locked: false,
                        locked_panes: Vec::new(),
                        layout: Layout::Pane(1),
                    }],
                },
                Workspace {
                    id: 20,
                    name: "two".into(),
                    cwd: "/two".into(),
                    tabs: Vec::new(),
                },
            ],
            panes: vec![crate::test_support::pane(&[])],
        }
    }

    #[test]
    fn fallback_rejects_a_closed_or_different_workspace_source() {
        let mut snapshot = snapshot();
        let request = Request::CreateTab {
            workspace: 10,
            source: Some(1),
            title: "Shell".into(),
        };
        assert_eq!(
            fallback_workspace(&snapshot, &request, 10).map(|w| w.id),
            Some(10)
        );
        let tabs = std::mem::take(&mut snapshot.workspaces[0].tabs);
        snapshot.workspaces[1].tabs = tabs;
        assert!(fallback_workspace(&snapshot, &request, 10).is_none());
        let tabs = std::mem::take(&mut snapshot.workspaces[1].tabs);
        snapshot.workspaces[0].tabs = tabs;
        snapshot.panes.clear();
        assert!(fallback_workspace(&snapshot, &request, 10).is_none());
    }

    #[test]
    fn fallback_refuses_splitting_an_agent_even_when_its_workspace_exists() {
        let mut snapshot = snapshot();
        let request = Request::Split {
            pane: 1,
            axis: Axis::Vertical,
            here: true,
        };
        assert_eq!(
            fallback_workspace(&snapshot, &request, 10).map(|w| w.id),
            Some(10)
        );
        let tabs = std::mem::take(&mut snapshot.workspaces[0].tabs);
        snapshot.workspaces[1].tabs = tabs;
        assert!(fallback_workspace(&snapshot, &request, 10).is_none());
        let tabs = std::mem::take(&mut snapshot.workspaces[1].tabs);
        snapshot.workspaces[0].tabs = tabs;
        snapshot.panes[0].agent = Some(Agent::default());
        assert!(fallback_workspace(&snapshot, &request, 10).is_none());
    }
}
