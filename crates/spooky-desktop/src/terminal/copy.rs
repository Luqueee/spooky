//! What the clipboard and a selection hold.

use spooky_core::Pane;
use std::collections::HashMap;

/// Panes whose program stored something in the clipboard since `seen` was
/// last updated. A pane seen for the first time only sets the baseline, so
/// writes made before this client looked are not replayed into the clipboard.
pub(crate) fn new_clipboard_writes(seen: &mut HashMap<u64, u64>, panes: &[Pane]) -> Vec<u64> {
    seen.retain(|id, _| panes.iter().any(|pane| pane.id == *id));
    panes
        .iter()
        .filter(|pane| {
            matches!(seen.insert(pane.id, pane.clipboard_seq), Some(before) if pane.clipboard_seq > before)
        })
        .map(|pane| pane.id)
        .collect()
}

/// Text under a selection: one line per row, without the blank cells that pad
/// each row to the grid width.
pub(crate) fn selection_text(pane: &Pane, anchor: (u16, u16), head: (u16, u16)) -> String {
    let (lo, hi) = if anchor <= head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let mut out = String::new();
    for row in lo.0..=hi.0 {
        let start = if row == lo.0 { lo.1 } else { 0 };
        let end = if row == hi.0 {
            hi.1
        } else {
            pane.cols.saturating_sub(1)
        };
        if row > lo.0 {
            out.push('\n');
        }
        if let Some(line) = pane.lines.iter().find(|line| line.row == row) {
            out.push_str(&line.text_between(start, end));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::pane;

    #[test]
    fn the_clipboard_is_set_only_by_writes_made_while_watching() {
        let panes = |seqs: &[(u64, u64)]| {
            seqs.iter()
                .map(|&(id, seq)| {
                    let mut pane = pane(&["x"]);
                    pane.id = id;
                    pane.clipboard_seq = seq;
                    pane
                })
                .collect::<Vec<_>>()
        };
        let mut seen = HashMap::new();
        // First sight is the baseline whatever its number: nothing is replayed.
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 5), (2, 0)])).is_empty());
        assert_eq!(
            new_clipboard_writes(&mut seen, &panes(&[(1, 5), (2, 1)])),
            [2]
        );
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 5), (2, 1)])).is_empty());
        assert_eq!(
            new_clipboard_writes(&mut seen, &panes(&[(1, 7), (2, 1)])),
            [1]
        );
        // A pane that went away is forgotten, so a new one with its number starts afresh.
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 7)])).is_empty());
        assert!(new_clipboard_writes(&mut seen, &panes(&[(1, 7), (2, 1)])).is_empty());
    }

    #[test]
    fn copied_selection_drops_the_blank_padding_of_each_row() {
        let pane = pane(&["foo       ", "bar       "]);
        assert_eq!(selection_text(&pane, (0, 0), (1, 9)), "foo\nbar");
        assert_eq!(selection_text(&pane, (1, 2), (0, 0)), "foo\nbar");
        assert_eq!(selection_text(&pane, (0, 1), (0, 2)), "oo");
    }
}
