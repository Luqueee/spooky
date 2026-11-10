//! Questions about the workspace tree and a snapshot that several features ask.

use crate::ContextTarget;
use spooky_core::{Axis, Layout, Snapshot};

/// Whether the thing a context menu was opened for is still in the snapshot.
pub(crate) fn context_target_exists(snapshot: &Snapshot, target: ContextTarget) -> bool {
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

pub(crate) fn first_pane(layout: &Layout) -> Option<u64> {
    let mut ids = Vec::new();
    layout.panes(&mut ids);
    ids.first().copied()
}

/// The split with this identity, anywhere in `layout`, with its axis.
pub(crate) fn find_split(layout: &Layout, split: u64) -> Option<(Axis, &Layout)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::pane;

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
}
