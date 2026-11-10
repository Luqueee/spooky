//! The command palette: a searchable list of every command, run with Enter or a click.

mod view;

pub(crate) use view::view;

use crate::commands::{self, Command};

/// What the palette tells the application.
#[derive(Clone, Debug)]
pub(crate) enum Message {
    /// Open the palette, or close it when it is open.
    Toggle,
    /// The query text changed.
    Query(String),
    /// Move the highlight down (positive) or up (negative), wrapping around.
    Move(i32),
    /// Run the highlighted command.
    Submit,
    /// Run this command (the user clicked it).
    Run(Command),
    /// Close without running anything.
    Close,
}

/// What a message asks the application to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// A command to carry out, after the palette has closed.
    pub run: Option<Command>,
    /// Hand the keyboard to the terminal (`true`) or take it away (`false`).
    pub terminal_focused: Option<bool>,
    /// Put the cursor in the query field.
    pub focus_field: bool,
}

/// The palette while it is open.
struct Open {
    query: String,
    results: Vec<Command>,
    selected: usize,
}

/// The command palette.
#[derive(Default)]
pub(crate) struct Palette {
    open: Option<Open>,
}

impl Palette {
    /// Whether the palette is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Applies a message.
    pub(crate) fn update(&mut self, message: Message) -> Outcome {
        match message {
            Message::Toggle if self.is_open() => self.close(None),
            Message::Toggle => {
                self.open = Some(Open {
                    query: String::new(),
                    results: commands::matching(""),
                    selected: 0,
                });
                Outcome {
                    terminal_focused: Some(false),
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::Query(query) => {
                if let Some(open) = self.open.as_mut() {
                    open.results = commands::matching(&query);
                    open.query = query;
                    open.selected = 0;
                }
                Outcome::default()
            }
            Message::Move(by) => {
                if let Some(open) = self.open.as_mut()
                    && !open.results.is_empty()
                {
                    let len = open.results.len() as i64;
                    open.selected = (open.selected as i64 + i64::from(by)).rem_euclid(len) as usize;
                }
                Outcome::default()
            }
            Message::Submit => {
                let chosen = self
                    .open
                    .as_ref()
                    .and_then(|open| open.results.get(open.selected).copied());
                match chosen {
                    Some(command) => self.close(Some(command)),
                    None => Outcome::default(),
                }
            }
            Message::Run(command) => {
                if self.is_open() {
                    self.close(Some(command))
                } else {
                    Outcome::default()
                }
            }
            Message::Close => self.close(None),
        }
    }

    fn close(&mut self, run: Option<Command>) -> Outcome {
        self.open = None;
        Outcome {
            run,
            terminal_focused: Some(true),
            focus_field: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> Palette {
        let mut palette = Palette::default();
        palette.update(Message::Toggle);
        palette
    }

    fn selected(palette: &Palette) -> Option<Command> {
        palette
            .open
            .as_ref()
            .and_then(|open| open.results.get(open.selected).copied())
    }

    #[test]
    fn opening_takes_the_keyboard_and_toggling_again_gives_it_back() {
        let mut palette = Palette::default();
        let outcome = palette.update(Message::Toggle);
        assert!(palette.is_open() && outcome.focus_field);
        assert_eq!(outcome.terminal_focused, Some(false));
        let outcome = palette.update(Message::Toggle);
        assert!(!palette.is_open());
        assert_eq!(outcome.terminal_focused, Some(true));
        assert_eq!(outcome.run, None);
    }

    #[test]
    fn typing_narrows_the_list_and_puts_the_first_match_under_the_highlight() {
        let mut palette = open();
        assert_eq!(selected(&palette), Some(commands::LISTED[0]));
        palette.update(Message::Move(3));
        palette.update(Message::Query("rename".into()));
        assert_eq!(selected(&palette), Some(Command::RenameTab));
        let results = &palette.open.as_ref().expect("open").results;
        assert_eq!(results, &[Command::RenameTab, Command::RenameWorkspace]);
    }

    #[test]
    fn the_highlight_wraps_in_both_directions_and_survives_an_empty_list() {
        let mut palette = open();
        palette.update(Message::Query("rename".into()));
        palette.update(Message::Move(1));
        assert_eq!(selected(&palette), Some(Command::RenameWorkspace));
        palette.update(Message::Move(1));
        assert_eq!(selected(&palette), Some(Command::RenameTab));
        palette.update(Message::Move(-1));
        assert_eq!(selected(&palette), Some(Command::RenameWorkspace));
        palette.update(Message::Query("zzzz".into()));
        assert_eq!(palette.update(Message::Move(1)), Outcome::default());
        assert_eq!(palette.update(Message::Submit), Outcome::default());
        assert!(palette.is_open(), "nothing to run keeps the palette open");
    }

    #[test]
    fn submitting_closes_and_runs_the_highlighted_command() {
        let mut palette = open();
        palette.update(Message::Query("split rows".into()));
        let outcome = palette.update(Message::Submit);
        assert_eq!(
            outcome.run,
            Some(Command::Split(spooky_core::Axis::Horizontal))
        );
        assert_eq!(outcome.terminal_focused, Some(true));
        assert!(!palette.is_open());
    }

    #[test]
    fn a_click_runs_that_command_but_only_while_the_palette_is_open() {
        let mut palette = open();
        let outcome = palette.update(Message::Run(Command::NewTab));
        assert_eq!(outcome.run, Some(Command::NewTab));
        assert!(!palette.is_open());
        assert_eq!(
            palette.update(Message::Run(Command::NewTab)),
            Outcome::default()
        );
    }

    #[test]
    fn closing_runs_nothing() {
        let mut palette = open();
        let outcome = palette.update(Message::Close);
        assert_eq!(outcome.run, None);
        assert!(!palette.is_open());
    }
}
