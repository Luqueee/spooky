//! Client-local unread activity derived from authoritative pane snapshots.

use spooky_core::{Layout, Pane, Snapshot};
use std::collections::{HashMap, hash_map::Entry};

/// The highest-priority unread event in a tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Indicator {
    /// A shell produced PTY output.
    Output,
    /// An agent completed a turn.
    AgentDone,
    /// An agent has a newly observed unanswered question or approval.
    Attention,
}

#[derive(Default)]
pub(crate) struct Activity {
    panes: HashMap<u64, Seen>,
    initialized: bool,
}

struct Seen {
    source: Source,
    unread: bool,
}

enum Source {
    Shell(u64),
    Agent {
        turns: u64,
        /// Current dialog IDs and whether each was opened while unseen.
        dialogs: Vec<(String, bool)>,
    },
}

/// The visible tab layout and, if zoomed, its sole visible pane.
pub(crate) type Visible<'a> = Option<(&'a Layout, Option<u64>)>;

fn pane_is_visible(layout: Visible<'_>, pane: u64) -> bool {
    layout.is_some_and(|(layout, zoom)| {
        layout.contains(pane) && zoom.is_none_or(|zoomed| zoomed == pane)
    })
}

/// Resolve the selected tab only when the window actually shows terminal content.
pub(crate) fn viewed_tab(
    snapshot: &Snapshot,
    workspace: Option<u64>,
    tab: Option<u64>,
    zoomed: Option<(u64, u64)>,
    showing: bool,
) -> Visible<'_> {
    if !showing {
        return None;
    }
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|item| Some(item.id) == workspace)?;
    let tab = workspace.tabs.iter().find(|item| Some(item.id) == tab)?;
    Some((
        &tab.layout,
        zoomed.filter(|(id, _)| *id == tab.id).map(|(_, pane)| pane),
    ))
}

impl Seen {
    fn new(pane: &Pane, viewed: bool, new_pane: bool) -> Self {
        let source = match &pane.agent {
            Some(agent) => Source::Agent {
                turns: agent.turn_seq,
                dialogs: agent
                    .dialogs
                    .iter()
                    .map(|dialog| (dialog.id.clone(), !viewed))
                    .collect(),
            },
            None => Source::Shell(pane.output_seq),
        };
        Self {
            source,
            unread: new_pane
                && !viewed
                && pane
                    .agent
                    .as_ref()
                    .map_or(pane.output_seq > 0, |agent| agent.turn_seq > 0),
        }
    }

    fn observe(&mut self, pane: &Pane, viewed: bool) {
        match (&mut self.source, &pane.agent) {
            (Source::Shell(previous), None) => {
                if pane.output_seq != *previous && !viewed {
                    self.unread = true;
                }
                *previous = pane.output_seq;
            }
            (Source::Agent { turns, dialogs }, Some(agent)) => {
                if agent.turn_seq != *turns && !viewed {
                    self.unread = true;
                }
                *turns = agent.turn_seq;
                dialogs.retain(|(id, _)| agent.dialogs.iter().any(|dialog| dialog.id == *id));
                for dialog in &agent.dialogs {
                    if !dialogs.iter().any(|(id, _)| *id == dialog.id) {
                        dialogs.push((dialog.id.clone(), !viewed));
                    }
                }
            }
            _ => *self = Self::new(pane, viewed, false),
        }
        if viewed {
            self.acknowledge();
        }
    }

    fn acknowledge(&mut self) {
        self.unread = false;
        if let Source::Agent { dialogs, .. } = &mut self.source {
            for (_, unread) in dialogs {
                *unread = false;
            }
        }
    }

    fn indicator(&self) -> Option<Indicator> {
        match &self.source {
            Source::Shell(_) if self.unread => Some(Indicator::Output),
            Source::Agent { dialogs, .. } if dialogs.iter().any(|(_, unread)| *unread) => {
                Some(Indicator::Attention)
            }
            Source::Agent { .. } if self.unread => Some(Indicator::AgentDone),
            _ => None,
        }
    }
}

impl Activity {
    /// Drop observations when the server identity may have changed.
    pub(crate) fn clear(&mut self) {
        self.panes.clear();
        self.initialized = false;
    }

    /// Observe a complete merged snapshot. The initial shell and turn counters are
    /// baselines, not new output; unanswered dialogs remain actionable on connect.
    pub(crate) fn observe(&mut self, snapshot: &Snapshot, viewed: Visible<'_>) {
        self.panes
            .retain(|id, _| snapshot.panes.iter().any(|pane| pane.id == *id));
        for pane in &snapshot.panes {
            let viewed = pane_is_visible(viewed, pane.id);
            match self.panes.entry(pane.id) {
                Entry::Vacant(entry) => {
                    entry.insert(Seen::new(pane, viewed, self.initialized));
                }
                Entry::Occupied(mut entry) => entry.get_mut().observe(pane, viewed),
            }
        }
        self.initialized = true;
    }

    /// Acknowledge panes as they become visible, even without another snapshot.
    pub(crate) fn acknowledge(&mut self, layout: &Layout, zoom: Option<u64>) {
        match layout {
            Layout::Pane(id) if zoom.is_none_or(|zoomed| zoomed == *id) => {
                if let Some(seen) = self.panes.get_mut(id) {
                    seen.acknowledge();
                }
            }
            Layout::Split { first, second, .. } => {
                self.acknowledge(first, zoom);
                self.acknowledge(second, zoom);
            }
            Layout::Pane(_) => {}
        }
    }

    /// Highest-priority unread event in this tab, including any split pane.
    pub(crate) fn indicator(&self, layout: &Layout) -> Option<Indicator> {
        match layout {
            Layout::Pane(id) => self.panes.get(id).and_then(Seen::indicator),
            Layout::Split { first, second, .. } => {
                self.indicator(first).max(self.indicator(second))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spooky_core::{Agent, AgentDialog, Axis, DialogKind, Tab, Workspace};

    fn shell(id: u64, seq: u64) -> Pane {
        let mut pane = crate::test_support::pane(&["$ "]);
        pane.id = id;
        pane.output_seq = seq;
        pane
    }

    fn agent(id: u64, turns: u64, dialogs: &[&str]) -> Pane {
        let mut pane = shell(id, 0);
        pane.agent = Some(Agent {
            turn_seq: turns,
            dialogs: dialogs
                .iter()
                .map(|id| AgentDialog {
                    id: (*id).into(),
                    kind: DialogKind::Confirm,
                    title: "Confirm".into(),
                    message: String::new(),
                    options: Vec::new(),
                })
                .collect(),
            ..Agent::default()
        });
        pane
    }

    fn snapshot(panes: Vec<Pane>) -> Snapshot {
        Snapshot {
            version: spooky_core::PROTOCOL_VERSION,
            revision: 1,
            workspaces: vec![Workspace {
                id: 1,
                name: "workspace".into(),
                cwd: "/".into(),
                tabs: panes
                    .iter()
                    .map(|pane| Tab {
                        id: pane.id,
                        title: String::new(),
                        renamed: false,
                        locked: false,
                        locked_panes: Vec::new(),
                        layout: Layout::Pane(pane.id),
                    })
                    .collect(),
            }],
            panes,
        }
    }

    #[test]
    fn shell_marks_only_new_output_and_clears_when_viewed() {
        let mut activity = Activity::default();
        let tab = Layout::Pane(1);
        let mut state = snapshot(vec![shell(1, 7)]);
        activity.observe(&state, None);
        assert_eq!(
            activity.indicator(&tab),
            None,
            "existing output is a baseline"
        );
        state.panes[0].cols = 120;
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), None, "resize is not output");
        state.panes[0].output_seq += 1;
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), Some(Indicator::Output));
        activity.acknowledge(&tab, None);
        assert_eq!(activity.indicator(&tab), None);
        state.panes[0].output_seq += 1;
        activity.observe(&state, Some((&tab, None)));
        assert_eq!(
            activity.indicator(&tab),
            None,
            "visible output is already seen"
        );
    }

    #[test]
    fn new_unseen_panes_report_activity_already_in_their_first_snapshot() {
        let mut activity = Activity::default();
        let mut state = snapshot(vec![shell(1, 9)]);
        activity.observe(&state, None);
        state = snapshot(vec![
            shell(1, 9),
            shell(2, 1),
            agent(3, 1, &[]),
            shell(4, 0),
        ]);
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&Layout::Pane(1)), None);
        assert_eq!(
            activity.indicator(&Layout::Pane(2)),
            Some(Indicator::Output)
        );
        assert_eq!(
            activity.indicator(&Layout::Pane(3)),
            Some(Indicator::AgentDone)
        );
        assert_eq!(activity.indicator(&Layout::Pane(4)), None);
    }

    #[test]
    fn agent_completion_and_unseen_dialogs_are_distinct_and_acknowledged() {
        let mut activity = Activity::default();
        let tab = Layout::Pane(2);
        let mut state = snapshot(vec![agent(2, 0, &[])]);
        activity.observe(&state, None);
        state.panes[0] = agent(2, 1, &[]);
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), Some(Indicator::AgentDone));
        state.panes[0] = agent(2, 1, &["first"]);
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), Some(Indicator::Attention));
        activity.acknowledge(&tab, None);
        activity.observe(&state, None);
        assert_eq!(
            activity.indicator(&tab),
            None,
            "an already viewed question does not re-alert"
        );
        state.panes[0] = agent(2, 1, &["first", "second"]);
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), Some(Indicator::Attention));
        state.panes[0] = agent(2, 1, &["first"]);
        activity.observe(&state, None);
        assert_eq!(
            activity.indicator(&tab),
            None,
            "a withdrawn unseen question is not pending"
        );
    }

    #[test]
    fn split_aggregation_and_zoom_only_acknowledge_visible_panes() {
        let mut activity = Activity::default();
        let tab = Layout::Split {
            id: 3,
            axis: Axis::Horizontal,
            ratio: 500,
            first: Box::new(Layout::Pane(1)),
            second: Box::new(Layout::Pane(2)),
        };
        let mut state = snapshot(vec![shell(1, 1), agent(2, 0, &[])]);
        activity.observe(&state, Some((&tab, None)));
        state.panes[0].output_seq += 1;
        state.panes[1] = agent(2, 1, &["approval"]);
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), Some(Indicator::Attention));
        activity.acknowledge(&tab, Some(2));
        assert_eq!(activity.indicator(&tab), Some(Indicator::Output));
        activity.acknowledge(&tab, None);
        assert_eq!(activity.indicator(&tab), None);
    }

    #[test]
    fn reconnect_resets_counters_but_preserves_actionable_questions() {
        let mut activity = Activity::default();
        let tab = Layout::Pane(1);
        let mut state = snapshot(vec![shell(1, 20)]);
        activity.observe(&state, None);
        state.panes[0].output_seq = 21;
        activity.observe(&state, None);
        activity.clear();
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), None);
        activity.clear();
        state.panes[0] = agent(1, 3, &["question"]);
        activity.observe(&state, None);
        assert_eq!(activity.indicator(&tab), Some(Indicator::Attention));
        state.panes.clear();
        activity.observe(&state, None);
        assert_eq!(
            activity.indicator(&tab),
            None,
            "removed panes leave no marker"
        );
    }
}
