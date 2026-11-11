use crate::agent;
use crate::agent::Agent;
use crate::pane::{Changes, Pane, Size};
use crate::persist::SavedAgent;
use crate::persist::{self, Saved};
use crate::settings::Settings;
use crate::shim::Shim;
use crate::state::{
    State, largest_pane, remove_leaf, replace_leaf, restore_cwd, validate_name, validate_size,
    validated_cwd,
};
use alacritty_terminal::grid::Scroll;
use anyhow::{Context, Result, anyhow, bail, ensure};
use spooky_core::{
    EVEN_SPLIT, Layout, MAX_AGENT_ARG_BYTES, MAX_AGENT_ARGS, MAX_MESSAGE_BYTES, MIN_SPLIT_SHARE,
    PROTOCOL_VERSION, Request, Response, ServerStatus, Snapshot, Tab, Workspace,
};
use std::{
    collections::BTreeMap,
    fs::{self},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

/// How long a burst of layout changes settles before it is written out.
pub(crate) const SAVE_DELAY: Duration = Duration::from_millis(300);

/// Wakes the thread that writes the saved workspaces.
#[derive(Default)]
pub(crate) struct Saver {
    dirty: Mutex<bool>,
    wake: Condvar,
}
impl Saver {
    pub(crate) fn mark(&self) {
        if let Ok(mut dirty) = self.dirty.lock() {
            *dirty = true;
            self.wake.notify_one();
        }
    }
    /// Blocks until something was marked, then takes the mark. False once
    /// the state is unusable.
    pub(crate) fn take(&self) -> bool {
        let Ok(mut dirty) = self.dirty.lock() else {
            return false;
        };
        while !*dirty {
            dirty = match self.wake.wait(dirty) {
                Ok(dirty) => dirty,
                Err(_) => return false,
            };
        }
        *dirty = false;
        true
    }
}

pub(crate) struct Server {
    pub(crate) state: Mutex<State>,
    pub(crate) changes: Arc<Changes>,
    pub(crate) layout_revision: AtomicU64,
    pub(crate) settings: Settings,
    pub(crate) started: Instant,
    /// Size for new panes while no pane exists to copy one from.
    pub(crate) fallback_size: Size,
    pub(crate) state_dir: PathBuf,
    pub(crate) saver: Arc<Saver>,
    /// Serializes writers of the saved workspaces.
    pub(crate) save_lock: Mutex<()>,
    /// Set by `Shutdown`; the connection that asked stops the server once it
    /// has been answered.
    pub(crate) stop_requested: AtomicBool,
    /// The `omp` shim given to new shells, when enabled and installed.
    pub(crate) shim: Option<Shim>,
}
impl Server {
    pub(crate) fn new(settings: Settings, state_dir: PathBuf, fallback_size: Size) -> Self {
        let shim = settings
            .agent_shim
            .then(|| Shim::install(&state_dir))
            .and_then(|installed| {
                installed
                    .map_err(|error| info!("the omp shim is off: {error:#}"))
                    .ok()
            });
        Self {
            shim,
            state: Mutex::new(State {
                next_id: 1,
                workspaces: Vec::new(),
                panes: BTreeMap::new(),
                agents: BTreeMap::new(),
            }),
            changes: Arc::new(Changes::default()),
            layout_revision: AtomicU64::new(0),
            settings,
            started: Instant::now(),
            fallback_size,
            state_dir,
            saver: Arc::new(Saver::default()),
            save_lock: Mutex::new(()),
            stop_requested: AtomicBool::new(false),
        }
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("server state poisoned"))
    }
    fn changed(&self) {
        self.changes.advance(&self.layout_revision);
        self.saver.mark();
    }
    fn reserve_id(&self) -> Result<u64> {
        let mut state = self.lock()?;
        let id = state.next_id;
        state.next_id = id.checked_add(1).context("identifier space exhausted")?;
        Ok(id)
    }
    fn pane(&self, id: u64) -> Result<Arc<Pane>> {
        let state = self.lock()?;
        if state.agents.contains_key(&id) {
            bail!("pane is an agent, not a terminal");
        }
        match state.panes.get(&id) {
            Some(pane) => Ok(pane.clone()),
            None => bail!("pane not found"),
        }
    }
    fn agent(&self, id: u64) -> Result<Arc<Agent>> {
        let state = self.lock()?;
        match state.agents.get(&id) {
            Some(agent) => Ok(agent.clone()),
            None if state.panes.contains_key(&id) => bail!("pane is a terminal, not an agent"),
            None => bail!("pane not found"),
        }
    }
    /// Grid size for a pane that is not a split half.
    fn pane_size(&self) -> Result<Size> {
        Ok(largest_pane(&*self.lock()?, self.fallback_size))
    }
    fn spawn_pane(&self, id: u64, cwd: &Path, size: Size) -> Result<Arc<Pane>> {
        Pane::spawn(
            id,
            cwd,
            size,
            self.changes.clone(),
            self.settings.scrollback_lines,
            self.shim.as_ref(),
        )
    }
    fn workspace_cwd(&self, workspace: u64) -> Result<PathBuf> {
        let state = self.lock()?;
        Ok(PathBuf::from(
            &state
                .workspaces
                .iter()
                .find(|w| w.id == workspace)
                .context("workspace not found")?
                .cwd,
        ))
    }
    /// Starts `backend` with the user's `args` in `cwd`: over the terminal pane `over`,
    /// which keeps running underneath and shows again when the agent is closed, or,
    /// without one, as the only pane of a new tab of `workspace`.
    fn create_agent(
        &self,
        workspace: u64,
        backend: &str,
        args: &[String],
        cwd: PathBuf,
        over: Option<u64>,
    ) -> Result<()> {
        if let Some(pane) = over {
            let state = self.lock()?;
            ensure!(state.panes.contains_key(&pane), "pane not found");
            ensure!(
                !state.agents.contains_key(&pane),
                "the pane already shows an agent"
            );
        }
        let id = match over {
            Some(pane) => pane,
            None => self.reserve_id()?,
        };
        let tab = self.reserve_id()?;
        let backend = agent::backend(backend, args, None)?;
        let agent = Agent::spawn(id, backend, &cwd, self.changes.clone(), self.saver.clone())?;
        let mut state = self.lock()?;
        if over.is_none() {
            let ws = state
                .workspaces
                .iter_mut()
                .find(|w| w.id == workspace)
                .context("workspace closed during agent creation")?;
            ws.tabs.push(Tab {
                id: tab,
                title: "Agent".into(),
                renamed: false,
                layout: Layout::Pane(id),
            });
        } else if !state.panes.contains_key(&id) {
            bail!("pane closed during agent creation");
        }
        state.agents.insert(id, agent);
        // The pane changes what it shows: clients need its whole screen again.
        self.changed();
        Ok(())
    }
    fn status(&self) -> Result<ServerStatus> {
        let state = self.lock()?;
        Ok(ServerStatus {
            pid: std::process::id(),
            version: PROTOCOL_VERSION,
            uptime_secs: self.started.elapsed().as_secs(),
            workspaces: state.workspaces.len() as u32,
            tabs: state.workspaces.iter().map(|w| w.tabs.len() as u32).sum(),
            panes: state.panes.len() as u32,
            running: state
                .panes
                .values()
                .filter(|pane| !pane.exited.load(Ordering::Acquire))
                .count() as u32,
            agents: state
                .agents
                .values()
                .filter(|agent| agent.is_alive())
                .count() as u32,
        })
    }
    /// Writes the workspaces so the next server can restore them.
    pub(crate) fn save(&self) -> Result<()> {
        let _serial = self
            .save_lock
            .lock()
            .map_err(|_| anyhow!("save state poisoned"))?;
        let (next_id, workspaces, agents, size) = {
            let state = self.lock()?;
            let agents = state
                .agents
                .values()
                .filter_map(|agent| agent.saved())
                .map(|mut saved| {
                    saved.covers = state.panes.contains_key(&saved.pane);
                    saved
                })
                .collect::<Vec<_>>();
            (
                state.next_id,
                state.workspaces.clone(),
                agents,
                largest_pane(&state, self.fallback_size),
            )
        };
        persist::store(
            &self.state_dir,
            &Saved::new(next_id, workspaces)
                .with_agents(agents)
                .sized(size.cols, size.rows),
        )
    }
    /// Rebuilds saved workspaces with fresh shells in their directories, and agents
    /// resumed from their saved sessions. A tab whose panes cannot all be started is
    /// left out.
    pub(crate) fn restore(&self, saved: Saved) -> Result<()> {
        let (cols, rows) = saved.size();
        let size = Size { cols, rows };
        let agents: BTreeMap<u64, &SavedAgent> = saved.agents.iter().map(|a| (a.pane, a)).collect();
        let mut state = self.lock()?;
        state.next_id = saved.next_id;
        for mut workspace in saved.workspaces {
            let cwd = restore_cwd(&workspace.cwd);
            workspace.cwd = cwd.to_string_lossy().into_owned();
            let mut tabs = Vec::new();
            for tab in std::mem::take(&mut workspace.tabs) {
                let mut ids = Vec::new();
                tab.layout.panes(&mut ids);
                let mut panes = Vec::new();
                let mut resumed = Vec::new();
                let spawned = ids.into_iter().try_for_each(|id| {
                    match agents.get(&id) {
                        Some(saved) => {
                            resumed.push((id, self.restore_agent(id, saved)?));
                            if saved.covers {
                                panes.push((id, self.spawn_pane(id, &cwd, size)?));
                            }
                        }
                        None => panes.push((id, self.spawn_pane(id, &cwd, size)?)),
                    }
                    Ok::<(), anyhow::Error>(())
                });
                match spawned {
                    Ok(()) => {
                        state.panes.extend(panes);
                        state.agents.extend(resumed);
                        tabs.push(tab);
                    }
                    Err(error) => info!(
                        "left out saved tab {} of workspace {}: {error:#}",
                        tab.id, workspace.id
                    ),
                }
            }
            if !tabs.is_empty() {
                workspace.tabs = tabs;
                state.workspaces.push(workspace);
            }
        }
        Ok(())
    }
    /// Starts a saved agent again, resuming its program's session.
    fn restore_agent(&self, id: u64, saved: &SavedAgent) -> Result<Arc<Agent>> {
        let cwd = restore_cwd(&saved.cwd);
        // `omp` writes its session file with the first message: a conversation that never
        // began has nothing to resume, and asking for it would end the agent at once.
        let session = saved
            .session
            .as_deref()
            .filter(|file| Path::new(file).is_file());
        let backend = agent::backend(&saved.backend, &[], session)?;
        Agent::spawn(id, backend, &cwd, self.changes.clone(), self.saver.clone())
    }
    /// Saves the workspaces, ends every shell and exits.
    pub(crate) fn stop(&self) -> ! {
        if let Err(error) = self.save() {
            info!("final save failed: {error:#}");
        }
        let (panes, agents) = self
            .lock()
            .map(|mut state| {
                (
                    std::mem::take(&mut state.panes),
                    std::mem::take(&mut state.agents),
                )
            })
            .unwrap_or_default();
        // Dropping the agents ends their processes; exit would skip it.
        drop(agents);
        for pane in panes.values() {
            pane.terminate();
        }
        if let Ok(path) = spooky_core::paths::socket_path() {
            let _ = fs::remove_file(path);
        }
        info!("stopped");
        std::process::exit(0)
    }
    pub(crate) fn dispatch(&self, request: Request) -> Response {
        match self.execute(request) {
            Ok(response) => response,
            Err(error) => Response::Error(format!("{error:#}")),
        }
    }
    fn execute(&self, request: Request) -> Result<Response> {
        match request {
            Request::Ping => {
                return Ok(Response::Pong {
                    version: PROTOCOL_VERSION,
                });
            }
            Request::Snapshot { since } => {
                let revision = self.changes.wait(since)?;
                if since == Some(revision) {
                    return Ok(Response::Unchanged { revision });
                }
                let (workspaces, panes, agents, covered) = {
                    let state = self.lock()?;
                    (
                        state.workspaces.clone(),
                        state.panes.values().cloned().collect::<Vec<_>>(),
                        state.agents.values().cloned().collect::<Vec<_>>(),
                        state.agents.keys().copied().collect::<Vec<u64>>(),
                    )
                };
                // A topology change needs every viewport row, including newly
                // inserted panes whose output may predate their insertion.
                let full = since.is_none_or(|since| {
                    since > revision || self.layout_revision.load(Ordering::Acquire) > since
                });
                let delta = if full { None } else { since };
                let panes = panes
                    .iter()
                    // An agent shown over a terminal hides it.
                    .filter(|pane| !covered.contains(&pane.id))
                    .filter(|pane| {
                        delta.is_none_or(|since| pane.revision.load(Ordering::Acquire) > since)
                    })
                    .map(|pane| pane.snapshot(delta))
                    .collect::<Result<Vec<_>>>()?;
                let mut panes = panes;
                for agent in &agents {
                    if delta.is_none_or(|since| agent.revision.load(Ordering::Acquire) > since) {
                        panes.push(agent.snapshot()?);
                    }
                }
                return Ok(Response::Snapshot(Snapshot {
                    version: PROTOCOL_VERSION,
                    revision,
                    workspaces,
                    panes,
                }));
            }
            Request::CreateWorkspace { name, cwd } => {
                validate_name(&name)?;
                let cwd = validated_cwd(cwd.as_deref())?;
                let workspace = self.reserve_id()?;
                let tab = self.reserve_id()?;
                let id = self.reserve_id()?;
                let size = self.pane_size()?;
                let pane = self.spawn_pane(id, &cwd, size)?;
                let mut state = self.lock()?;
                state.panes.insert(id, pane);
                state.workspaces.push(Workspace {
                    id: workspace,
                    name,
                    cwd: cwd.to_string_lossy().into_owned(),
                    tabs: vec![Tab {
                        id: tab,
                        title: spooky_core::DEFAULT_TITLE.into(),
                        renamed: false,
                        layout: Layout::Pane(id),
                    }],
                });
                self.changed();
            }
            Request::RenameWorkspace { workspace, name } => {
                validate_name(&name)?;
                let mut state = self.lock()?;
                state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .context("workspace not found")?
                    .name = name;
                self.changed();
            }
            Request::CreateTab { workspace, title } => {
                validate_name(&title)?;
                let cwd = {
                    let state = self.lock()?;
                    PathBuf::from(
                        &state
                            .workspaces
                            .iter()
                            .find(|w| w.id == workspace)
                            .context("workspace not found")?
                            .cwd,
                    )
                };
                let id = self.reserve_id()?;
                let tab = self.reserve_id()?;
                let size = self.pane_size()?;
                let pane = self.spawn_pane(id, &cwd, size)?;
                let mut state = self.lock()?;
                let ws = state
                    .workspaces
                    .iter_mut()
                    .find(|w| w.id == workspace)
                    .context("workspace closed during tab creation")?;
                ws.tabs.push(Tab {
                    id: tab,
                    title,
                    renamed: false,
                    layout: Layout::Pane(id),
                });
                state.panes.insert(id, pane);
                self.changed();
            }
            Request::RenameTab { tab, title } => {
                validate_name(&title)?;
                let mut state = self.lock()?;
                let tab = state
                    .workspaces
                    .iter_mut()
                    .flat_map(|w| &mut w.tabs)
                    .find(|t| t.id == tab)
                    .context("tab not found")?;
                tab.title = title;
                tab.renamed = true;
                self.changed();
            }
            Request::CloseTab { tab } => {
                let removed = {
                    let mut state = self.lock()?;
                    let wi = state
                        .workspaces
                        .iter()
                        .position(|w| w.tabs.iter().any(|t| t.id == tab))
                        .context("tab not found")?;
                    let ws = &mut state.workspaces[wi];
                    let ti = ws
                        .tabs
                        .iter()
                        .position(|t| t.id == tab)
                        .context("tab not found")?;
                    let closed = ws.tabs.remove(ti);
                    if ws.tabs.is_empty() {
                        state.workspaces.remove(wi);
                    }
                    let mut ids = Vec::new();
                    closed.layout.panes(&mut ids);
                    let panes = ids
                        .iter()
                        .filter_map(|id| state.panes.remove(id))
                        .collect::<Vec<_>>();
                    let agents = ids
                        .iter()
                        .filter_map(|id| state.agents.remove(id))
                        .collect::<Vec<_>>();
                    drop(agents);
                    self.changed();
                    panes
                };
                // Shells are terminated outside the lock.
                drop(removed);
            }
            Request::Split { pane, axis } => {
                let (cwd, size) = {
                    let state = self.lock()?;
                    if state.agents.contains_key(&pane) {
                        bail!("an agent pane cannot be split");
                    }
                    let p = state.panes.get(&pane).context("pane not found")?;
                    let ws = state
                        .workspaces
                        .iter()
                        .find(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                        .context("pane is detached")?;
                    (PathBuf::from(&ws.cwd), p.size())
                };
                let id = self.reserve_id()?;
                let split = self.reserve_id()?;
                let new_pane = self.spawn_pane(id, &cwd, size)?;
                let mut state = self.lock()?;
                let tab = state
                    .workspaces
                    .iter_mut()
                    .flat_map(|w| &mut w.tabs)
                    .find(|t| t.layout.contains(pane))
                    .context("pane closed during split")?;
                replace_leaf(
                    &mut tab.layout,
                    pane,
                    Layout::Split {
                        id: split,
                        axis,
                        ratio: EVEN_SPLIT,
                        first: Box::new(Layout::Pane(pane)),
                        second: Box::new(Layout::Pane(id)),
                    },
                )?;
                state.panes.insert(id, new_pane);
                self.changed();
            }
            Request::Input { pane, bytes } => {
                if bytes.len() > MAX_MESSAGE_BYTES {
                    bail!("input exceeds protocol size limit");
                }
                let pane = self.pane(pane)?;
                if pane.exited.load(Ordering::Acquire) {
                    bail!("pane has exited");
                }
                pane.writer
                    .try_send(bytes)
                    .map_err(|e| anyhow!("terminal input unavailable: {e}"))?;
                pane.scroll_to_bottom()?;
            }
            Request::Resize { pane, cols, rows } => {
                validate_size(cols, rows)?;
                self.pane(pane)?.resize(Size { cols, rows })?;
            }
            Request::Scroll { pane, lines } => {
                let pane = self.pane(pane)?;
                let mut term = pane
                    .terminal
                    .lock()
                    .map_err(|_| anyhow!("terminal state poisoned"))?;
                term.scroll_display(Scroll::Delta(lines));
                pane.changes.advance(&pane.revision);
            }
            Request::ClosePane { pane } => {
                let removed = {
                    let mut state = self.lock()?;
                    if state.panes.contains_key(&pane)
                        && let Some(agent) = state.agents.remove(&pane)
                    {
                        // The terminal under it shows again.
                        self.changed();
                        drop(state);
                        drop(agent);
                        return Ok(Response::Ok);
                    }
                    let wi = state
                        .workspaces
                        .iter()
                        .position(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                        .context("pane not found")?;
                    let ws = &mut state.workspaces[wi];
                    let ti = ws
                        .tabs
                        .iter()
                        .position(|t| t.layout.contains(pane))
                        .context("pane not found")?;
                    if matches!(ws.tabs[ti].layout, Layout::Pane(_)) {
                        ws.tabs.remove(ti);
                    } else {
                        remove_leaf(&mut ws.tabs[ti].layout, pane)?;
                    }
                    if ws.tabs.is_empty() {
                        state.workspaces.remove(wi);
                    }
                    let removed = state.panes.remove(&pane);
                    let agent = state.agents.remove(&pane);
                    let removed = (removed, agent);
                    self.changed();
                    removed
                };
                drop(removed);
            }
            Request::CloseWorkspace { workspace } => {
                let removed = {
                    let mut state = self.lock()?;
                    let index = state
                        .workspaces
                        .iter()
                        .position(|w| w.id == workspace)
                        .context("workspace not found")?;
                    let ws = state.workspaces.remove(index);
                    let mut ids = Vec::new();
                    for tab in ws.tabs {
                        tab.layout.panes(&mut ids);
                    }
                    let panes = ids
                        .iter()
                        .filter_map(|id| state.panes.remove(id))
                        .collect::<Vec<_>>();
                    let agents = ids
                        .iter()
                        .filter_map(|id| state.agents.remove(id))
                        .collect::<Vec<_>>();
                    drop(agents);
                    self.changed();
                    panes
                };
                drop(removed);
            }
            Request::SetSplitRatio { split, ratio } => {
                if !(MIN_SPLIT_SHARE..=1000 - MIN_SPLIT_SHARE).contains(&ratio) {
                    bail!("split ratio out of range");
                }
                let mut state = self.lock()?;
                let moved = state
                    .workspaces
                    .iter_mut()
                    .flat_map(|w| &mut w.tabs)
                    .any(|t| t.layout.set_ratio(split, ratio));
                if !moved {
                    bail!("split not found");
                }
                self.changed();
            }
            Request::Find {
                pane,
                query,
                direction,
            } => {
                if query.len() > 1024 {
                    bail!("search text is too long");
                }
                return self.pane(pane)?.find(&query, direction);
            }
            Request::Clipboard { pane } => {
                let pane = self.pane(pane)?;
                let (seq, text) = {
                    let held = pane
                        .clipboard
                        .text
                        .lock()
                        .map_err(|_| anyhow!("clipboard state poisoned"))?;
                    (pane.clipboard.seq.load(Ordering::Acquire), held.clone())
                };
                return Ok(Response::Clipboard { seq, text });
            }
            Request::CreateAgent { workspace, backend } => {
                let cwd = self.workspace_cwd(workspace)?;
                self.create_agent(workspace, &backend, &[], cwd, None)?;
            }
            Request::OpenAgent {
                pane,
                backend,
                cwd,
                args,
            } => {
                if args.len() > MAX_AGENT_ARGS || args.iter().any(|a| a.len() > MAX_AGENT_ARG_BYTES)
                {
                    bail!("too many or too long arguments");
                }
                let workspace = {
                    let state = self.lock()?;
                    state
                        .workspaces
                        .iter()
                        .find(|w| w.tabs.iter().any(|t| t.layout.contains(pane)))
                        .map(|w| w.id)
                        .context("pane not found")?
                };
                let cwd = validated_cwd(Some(&cwd))?;
                self.create_agent(workspace, &backend, &args, cwd, Some(pane))?;
            }
            Request::AgentPrompt { pane, text } => self.agent(pane)?.prompt(&text)?,
            Request::AgentAbort { pane } => self.agent(pane)?.abort()?,
            Request::AgentUsage { pane } => self.agent(pane)?.usage()?,
            Request::AgentSetModel { pane, model } => self.agent(pane)?.set_model(&model)?,
            Request::AgentSetThinking { pane, level } => {
                self.agent(pane)?.set_thinking(&level)?;
            }
            Request::AgentAnswer {
                pane,
                dialog,
                answer,
            } => self.agent(pane)?.answer(&dialog, &answer)?,
            Request::Status => return Ok(Response::Status(self.status()?)),
            Request::Shutdown => {
                info!("shutdown requested");
                self.stop_requested.store(true, Ordering::Release);
            }
        }
        Ok(Response::Ok)
    }
}
