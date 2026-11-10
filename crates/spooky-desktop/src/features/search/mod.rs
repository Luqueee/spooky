//! Scrollback search: the strip above the terminal, and the requests that move the
//! server's match position. The server owns the matches and the highlights; this keeps
//! what was asked and what was answered.

mod view;

pub(crate) use view::view;

use crate::terminal::is_chord_echo;
use spooky_core::{FindDirection, Request};

/// What the search strip tells the application.
#[derive(Clone, Debug)]
pub(crate) enum Message {
    /// Open the strip (when there is a pane to search).
    Open,
    /// The query text changed.
    Input(String),
    /// Go to the next or previous match.
    Step(FindDirection),
    /// Close the strip and clear the highlights.
    Close,
}

/// What a message asks the application to do.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    /// Requests for the server.
    pub requests: Vec<Request>,
    /// Hand the keyboard to the terminal (`true`) or take it away (`false`).
    pub terminal_focused: Option<bool>,
    /// Put the cursor in the query field, selecting its text.
    pub focus_field: bool,
}

/// The strip, while it is open.
#[derive(Default)]
struct Bar {
    query: String,
    /// The pane the last search ran in, which holds the highlights.
    pane: Option<u64>,
    /// `None` until a search answered; then the match `(current, total)` or none.
    result: Option<Option<(u32, u32)>>,
}

/// The scrollback search.
#[derive(Default)]
pub(crate) struct Search {
    bar: Option<Bar>,
}

impl Search {
    /// Whether the strip is showing.
    pub(crate) fn is_open(&self) -> bool {
        self.bar.is_some()
    }

    /// Forgets the search without telling the server, as when the target changes.
    pub(crate) fn reset(&mut self) {
        self.bar = None;
    }

    /// The server's answer to the last search: the match `(current, total)`, or none.
    pub(crate) fn found(&mut self, result: Option<(u32, u32)>) {
        if let Some(bar) = self.bar.as_mut() {
            bar.result = Some(result);
        }
    }

    /// Applies a message. `pane` is the selected pane; `primary_held` whether the
    /// application-shortcut modifier is down.
    pub(crate) fn update(
        &mut self,
        message: Message,
        pane: Option<u64>,
        primary_held: bool,
    ) -> Outcome {
        match message {
            Message::Open => {
                if pane.is_none() {
                    return Outcome::default();
                }
                self.bar.get_or_insert_with(Bar::default);
                Outcome {
                    terminal_focused: Some(false),
                    focus_field: true,
                    ..Outcome::default()
                }
            }
            Message::Input(query) => {
                // A text field inserts the letter of Cmd+F and Cmd+G; that edit is
                // the chord, handled as a key press, not text.
                if primary_held
                    && self
                        .bar
                        .as_ref()
                        .is_some_and(|bar| is_chord_echo(&bar.query, &query))
                {
                    return Outcome::default();
                }
                let mut outcome = self.find(&query, FindDirection::Previous, pane);
                outcome.terminal_focused = Some(false);
                outcome
            }
            Message::Step(direction) => match self.bar.as_ref().map(|bar| bar.query.clone()) {
                Some(query) => self.find(&query, direction, pane),
                None => Outcome::default(),
            },
            Message::Close => {
                let Some(bar) = self.bar.take() else {
                    return Outcome::default();
                };
                Outcome {
                    requests: bar.pane.map(clear).into_iter().collect(),
                    terminal_focused: Some(true),
                    focus_field: false,
                }
            }
        }
    }

    /// Searches the selected pane's scrollback and records what was asked. A search
    /// that moves to another pane first clears the highlights of the old one.
    fn find(&mut self, query: &str, direction: FindDirection, pane: Option<u64>) -> Outcome {
        let (Some(pane), Some(bar)) = (pane, self.bar.as_mut()) else {
            return Outcome::default();
        };
        bar.query = query.to_owned();
        bar.result = None;
        let mut requests = Vec::new();
        if let Some(old) = bar.pane.replace(pane).filter(|old| *old != pane) {
            requests.push(Request::Find {
                pane: old,
                query: String::new(),
                direction,
            });
        }
        requests.push(Request::Find {
            pane,
            query: query.to_owned(),
            direction,
        });
        Outcome {
            requests,
            ..Outcome::default()
        }
    }
}

/// The request that clears a pane's highlights.
fn clear(pane: u64) -> Request {
    Request::Find {
        pane,
        query: String::new(),
        direction: FindDirection::Next,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finds(outcome: &Outcome) -> Vec<(u64, &str, FindDirection)> {
        outcome
            .requests
            .iter()
            .map(|request| match request {
                Request::Find {
                    pane,
                    query,
                    direction,
                } => (*pane, query.as_str(), *direction),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    fn open(pane: u64) -> Search {
        let mut search = Search::default();
        search.update(Message::Open, Some(pane), false);
        search
    }

    #[test]
    fn the_strip_needs_a_pane_and_takes_the_keyboard_when_it_opens() {
        let mut search = Search::default();
        let outcome = search.update(Message::Open, None, false);
        assert!(!search.is_open());
        assert_eq!(outcome.terminal_focused, None);
        let outcome = search.update(Message::Open, Some(1), false);
        assert!(search.is_open());
        assert_eq!(outcome.terminal_focused, Some(false));
        assert!(outcome.focus_field);
    }

    #[test]
    fn typing_searches_the_selected_pane_and_forgets_the_old_answer() {
        let mut search = open(1);
        search.found(Some((1, 3)));
        let outcome = search.update(Message::Input("err".into()), Some(1), false);
        assert_eq!(finds(&outcome), [(1, "err", FindDirection::Previous)]);
        assert_eq!(outcome.terminal_focused, Some(false));
        assert!(search.bar.as_ref().is_some_and(|bar| bar.result.is_none()));
        search.found(Some((2, 3)));
        assert_eq!(
            search.bar.as_ref().and_then(|bar| bar.result),
            Some(Some((2, 3)))
        );
    }

    #[test]
    fn stepping_repeats_the_query_in_the_asked_direction() {
        let mut search = open(1);
        search.update(Message::Input("err".into()), Some(1), false);
        let outcome = search.update(Message::Step(FindDirection::Next), Some(1), false);
        assert_eq!(finds(&outcome), [(1, "err", FindDirection::Next)]);
        let closed = Search::default().update(Message::Step(FindDirection::Next), Some(1), false);
        assert!(closed.requests.is_empty(), "nothing to step through");
    }

    #[test]
    fn moving_to_another_pane_clears_the_old_highlights_first() {
        let mut search = open(1);
        search.update(Message::Input("err".into()), Some(1), false);
        let outcome = search.update(Message::Step(FindDirection::Next), Some(2), false);
        assert_eq!(
            finds(&outcome),
            [
                (1, "", FindDirection::Next),
                (2, "err", FindDirection::Next)
            ]
        );
    }

    #[test]
    fn closing_clears_the_highlights_and_returns_the_keyboard() {
        let mut search = open(1);
        search.update(Message::Input("err".into()), Some(1), false);
        let outcome = search.update(Message::Close, Some(1), false);
        assert_eq!(finds(&outcome), [(1, "", FindDirection::Next)]);
        assert_eq!(outcome.terminal_focused, Some(true));
        assert!(!search.is_open());
        let again = search.update(Message::Close, Some(1), false);
        assert!(again.requests.is_empty() && again.terminal_focused.is_none());
        // A strip that never searched has nothing to clear.
        let mut fresh = open(1);
        assert!(
            fresh
                .update(Message::Close, Some(1), false)
                .requests
                .is_empty()
        );
    }

    #[test]
    fn the_letter_a_chord_leaves_in_the_field_is_not_a_query_edit() {
        let mut search = open(1);
        search.update(Message::Input("hit-".into()), Some(1), false);
        let echoed = search.update(Message::Input("hit-g".into()), Some(1), true);
        assert!(echoed.requests.is_empty());
        let typed = search.update(Message::Input("hit-g".into()), Some(1), false);
        assert_eq!(finds(&typed), [(1, "hit-g", FindDirection::Previous)]);
    }
}
