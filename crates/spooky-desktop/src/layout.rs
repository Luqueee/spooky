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

/// The original tree remains authoritative; zoom only chooses a leaf to render.
pub(crate) fn visible_layout(layout: &Layout, zoomed: Option<u64>) -> &Layout {
    fn pane(layout: &Layout, id: u64) -> Option<&Layout> {
        match layout {
            Layout::Pane(found) => (*found == id).then_some(layout),
            Layout::Split { first, second, .. } => pane(first, id).or_else(|| pane(second, id)),
        }
    }
    zoomed.and_then(|id| pane(layout, id)).unwrap_or(layout)
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
    fn zoom_shows_one_nested_pane_and_restores_the_original_tree() {
        let tree = Layout::Split {
            id: 10,
            axis: Axis::Vertical,
            ratio: 400,
            first: Box::new(Layout::Pane(1)),
            second: Box::new(Layout::Split {
                id: 11,
                axis: Axis::Horizontal,
                ratio: 300,
                first: Box::new(Layout::Pane(2)),
                second: Box::new(Layout::Pane(3)),
            }),
        };
        assert!(matches!(visible_layout(&tree, Some(3)), Layout::Pane(3)));
        assert!(matches!(
            visible_layout(&tree, None),
            Layout::Split { ratio: 400, second, .. }
                if matches!(second.as_ref(), Layout::Split { ratio: 300, .. })
        ));
        assert!(std::ptr::eq(visible_layout(&tree, Some(99)), &tree));
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
                    locked: false,
                    locked_panes: Vec::new(),
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
