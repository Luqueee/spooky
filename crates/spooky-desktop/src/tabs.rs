//! How a tab is labelled.

use spooky_core::Tab;

/// Longest tab label, in characters.
pub(crate) const MAX_TAB_LABEL: usize = 28;

/// `text` cut to `max` characters, with an ellipsis when it was cut.
pub(crate) fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max - 1).chain(['…']).collect()
    }
}

/// A tab's label: the name the user gave it, or else its terminal's title
/// unless that is still the default.
pub(crate) fn label_for(tab: &Tab, terminal_title: Option<&str>) -> String {
    let title = match terminal_title {
        Some(title) if !tab.renamed && !title.is_empty() && title != spooky_core::DEFAULT_TITLE => {
            title
        }
        _ => &tab.title,
    };
    shorten(title, MAX_TAB_LABEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tab_shows_its_terminal_title_until_the_user_names_it() {
        let tab = |renamed| spooky_core::Tab {
            id: 1,
            title: "Shell".into(),
            renamed,
            layout: spooky_core::Layout::Pane(1),
        };
        assert_eq!(label_for(&tab(false), Some("vim notes.md")), "vim notes.md");
        assert_eq!(label_for(&tab(false), Some("Terminal")), "Shell");
        assert_eq!(label_for(&tab(false), Some("")), "Shell");
        assert_eq!(label_for(&tab(false), None), "Shell");
        assert_eq!(label_for(&tab(true), Some("vim notes.md")), "Shell");
        let long = "x".repeat(60);
        let shown = label_for(&tab(false), Some(&long));
        assert_eq!(shown.chars().count(), MAX_TAB_LABEL);
        assert!(shown.ends_with('…'));
    }
}
