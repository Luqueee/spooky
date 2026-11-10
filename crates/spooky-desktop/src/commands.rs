//! The commands of the application: everything a button, menu entry, keyboard
//! shortcut or the command palette can ask for, each with a stable id and a title.
//!
//! The ids are the names later extensions will use (`core.tab.new`, and
//! `<mod-id>.<name>` for a mod's own). Shortcuts map keys to commands
//! ([`crate::shortcuts`]); the application maps commands to what they do.

use spooky_core::Axis;

/// What can be asked of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    NewTab,
    NewWorkspace,
    Split(Axis),
    ClosePane,
    CloseWorkspace,
    /// A zero-based tab position; position 8 means the last tab.
    Tab(usize),
    NextTab,
    PreviousTab,
    /// A zero-based workspace position; position 8 means the last workspace.
    Workspace(usize),
    NextWorkspace,
    PreviousWorkspace,
    RenameTab,
    RenameWorkspace,
    ToggleSidebar,
    /// Open the scrollback search, or focus it when it is open.
    Find,
    FindNext,
    FindPrevious,
    /// Open the settings page.
    Settings,
    Quit,
    /// Open the command palette.
    Palette,
    /// Pick a file to attach to the selected pane.
    Attach,
    /// Capture the selected pane as a PNG.
    Capture,
    /// Copy the selection.
    Copy,
    /// Paste the clipboard into the selected pane.
    Paste,
    /// Show the panel for connecting to another machine.
    Connect,
}

const TAB_IDS: [&str; 9] = [
    "core.tab.1",
    "core.tab.2",
    "core.tab.3",
    "core.tab.4",
    "core.tab.5",
    "core.tab.6",
    "core.tab.7",
    "core.tab.8",
    "core.tab.9",
];
const WORKSPACE_IDS: [&str; 9] = [
    "core.workspace.1",
    "core.workspace.2",
    "core.workspace.3",
    "core.workspace.4",
    "core.workspace.5",
    "core.workspace.6",
    "core.workspace.7",
    "core.workspace.8",
    "core.workspace.9",
];

/// The commands the palette lists, in the order it shows them when nothing is typed.
/// Selecting a tab or workspace by position is left out: it is a shortcut, not a choice.
pub(crate) const LISTED: &[Command] = &[
    Command::NewTab,
    Command::NewWorkspace,
    Command::Split(Axis::Vertical),
    Command::Split(Axis::Horizontal),
    Command::NextTab,
    Command::PreviousTab,
    Command::NextWorkspace,
    Command::PreviousWorkspace,
    Command::RenameTab,
    Command::RenameWorkspace,
    Command::ClosePane,
    Command::CloseWorkspace,
    Command::Find,
    Command::FindNext,
    Command::FindPrevious,
    Command::Copy,
    Command::Paste,
    Command::Attach,
    Command::Capture,
    Command::ToggleSidebar,
    Command::Connect,
    Command::Settings,
    Command::Quit,
];

impl Command {
    /// The stable name of the command.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::NewTab => "core.tab.new",
            Self::NewWorkspace => "core.workspace.new",
            Self::Split(Axis::Vertical) => "core.pane.split.columns",
            Self::Split(Axis::Horizontal) => "core.pane.split.rows",
            Self::ClosePane => "core.pane.close",
            Self::CloseWorkspace => "core.workspace.close",
            Self::Tab(position) => TAB_IDS[position.min(8)],
            Self::NextTab => "core.tab.next",
            Self::PreviousTab => "core.tab.previous",
            Self::Workspace(position) => WORKSPACE_IDS[position.min(8)],
            Self::NextWorkspace => "core.workspace.next",
            Self::PreviousWorkspace => "core.workspace.previous",
            Self::RenameTab => "core.tab.rename",
            Self::RenameWorkspace => "core.workspace.rename",
            Self::ToggleSidebar => "core.sidebar.toggle",
            Self::Find => "core.search.find",
            Self::FindNext => "core.search.next",
            Self::FindPrevious => "core.search.previous",
            Self::Settings => "core.settings.open",
            Self::Quit => "core.app.quit",
            Self::Palette => "core.palette.open",
            Self::Attach => "core.file.attach",
            Self::Capture => "core.pane.capture",
            Self::Copy => "core.clipboard.copy",
            Self::Paste => "core.clipboard.paste",
            Self::Connect => "core.connection.open",
        }
    }

    /// What the palette shows.
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::NewTab => "New tab",
            Self::NewWorkspace => "New workspace",
            Self::Split(Axis::Vertical) => "Split columns",
            Self::Split(Axis::Horizontal) => "Split rows",
            Self::ClosePane => "Close pane",
            Self::CloseWorkspace => "Close workspace",
            Self::Tab(_) => "Go to tab",
            Self::NextTab => "Next tab",
            Self::PreviousTab => "Previous tab",
            Self::Workspace(_) => "Go to workspace",
            Self::NextWorkspace => "Next workspace",
            Self::PreviousWorkspace => "Previous workspace",
            Self::RenameTab => "Rename tab",
            Self::RenameWorkspace => "Rename workspace",
            Self::ToggleSidebar => "Toggle sidebar",
            Self::Find => "Find in scrollback",
            Self::FindNext => "Find next match",
            Self::FindPrevious => "Find previous match",
            Self::Settings => "Settings",
            Self::Quit => "Quit Spooky",
            Self::Palette => "Command palette",
            Self::Attach => "Attach file",
            Self::Capture => "Capture pane as PNG",
            Self::Copy => "Copy selection",
            Self::Paste => "Paste",
            Self::Connect => "Connect to a machine",
        }
    }
}

/// The listed commands whose title contains every word of `query`, ignoring case; a
/// word with a dot (`core.search`) is matched against the id instead, so the namespace
/// does not match everything. Titles that start with the first word come first;
/// otherwise the listed order stays.
pub(crate) fn matching(query: &str) -> Vec<Command> {
    let query = query.to_lowercase();
    let words: Vec<&str> = query.split_whitespace().collect();
    let mut found: Vec<Command> = LISTED
        .iter()
        .copied()
        .filter(|command| {
            let title = command.title().to_lowercase();
            let id = command.id();
            words
                .iter()
                .all(|word| title.contains(word) || (word.contains('.') && id.contains(word)))
        })
        .collect();
    if let Some(first) = words.first() {
        found.sort_by_key(|command| !command.title().to_lowercase().starts_with(first));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_namespaced_and_stable_in_spelling() {
        let mut all: Vec<Command> = LISTED.to_vec();
        all.extend((0..9).map(Command::Tab));
        all.extend((0..9).map(Command::Workspace));
        all.push(Command::Palette);
        let ids: HashSet<&str> = all.iter().map(|command| command.id()).collect();
        assert_eq!(ids.len(), all.len(), "two commands share an id");
        assert!(
            ids.iter().all(|id| id.starts_with("core.")
                && id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.')),
            "ids are lowercase dotted names under core."
        );
        assert_eq!(Command::Tab(8).id(), "core.tab.9");
        assert_eq!(
            Command::Tab(40).id(),
            "core.tab.9",
            "positions past 9 clamp"
        );
    }

    #[test]
    fn no_listed_command_is_positional_or_the_palette_itself() {
        assert!(!LISTED.iter().any(|c| matches!(
            c,
            Command::Tab(_) | Command::Workspace(_) | Command::Palette
        )));
    }

    #[test]
    fn an_empty_query_lists_everything_in_order() {
        assert_eq!(matching(""), LISTED);
        assert_eq!(matching("   "), LISTED);
    }

    #[test]
    fn every_word_must_match_the_title_or_the_id_in_any_case() {
        let titles = |query| -> Vec<&'static str> {
            matching(query).into_iter().map(Command::title).collect()
        };
        assert_eq!(titles("SPLIT"), ["Split columns", "Split rows"]);
        assert_eq!(titles("split rows"), ["Split rows"]);
        assert_eq!(
            titles("core.search"),
            [
                "Find in scrollback",
                "Find next match",
                "Find previous match"
            ]
        );
        assert!(titles("nonexistent").is_empty());
    }

    #[test]
    fn titles_starting_with_the_first_word_come_first_and_the_rest_keep_their_order() {
        let titles: Vec<&str> = matching("re").into_iter().map(Command::title).collect();
        assert_eq!(&titles[..2], ["Rename tab", "Rename workspace"]);
        // "Previous tab" is listed before the renames but only contains the word.
        assert_eq!(titles[2], "Previous tab");
    }
}
