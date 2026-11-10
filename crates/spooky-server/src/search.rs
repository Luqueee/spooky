//! Case-insensitive search over a terminal's scrollback and screen.
//!
//! A match lies within one grid line: text that the terminal wrapped onto the
//! next line is not joined back together.

use alacritty_terminal::{
    event::EventListener,
    grid::Dimensions,
    index::{Column, Line},
    term::{Term, cell::Flags},
};

/// Most recent lines one search reads. It bounds how long the terminal stays
/// locked, and with it how long a huge scrollback can stall its output.
pub const SEARCH_LINES: usize = 100_000;

/// One occurrence, in grid coordinates: negative lines are scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub line: i32,
    pub start: u16,
    /// Last column covered, inclusive.
    pub end: u16,
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The query prepared for comparison. Empty when there is nothing to find.
pub fn needle(query: &str) -> Vec<char> {
    query.chars().map(fold).collect()
}

/// Every occurrence of `needle` among the cells of one line, as `(start, end)`
/// columns. `cells` holds each character with the columns it covers; occurrences
/// do not overlap.
fn occurrences(cells: &[(char, u16, u16)], needle: &[char]) -> Vec<(u16, u16)> {
    let mut found = Vec::new();
    if needle.is_empty() {
        return found;
    }
    let mut at = 0;
    while at + needle.len() <= cells.len() {
        if cells[at..at + needle.len()]
            .iter()
            .zip(needle)
            .all(|((c, _, _), wanted)| fold(*c) == *wanted)
        {
            found.push((cells[at].1, cells[at + needle.len() - 1].2));
            at += needle.len();
        } else {
            at += 1;
        }
    }
    found
}

/// Matches on grid lines `from..=to`, oldest first, restricted to lines the grid
/// holds and to the most recent [`SEARCH_LINES`].
pub fn matches_in<T: EventListener>(
    term: &Term<T>,
    needle: &[char],
    from: i32,
    to: i32,
) -> Vec<Match> {
    let grid = term.grid();
    let oldest = grid
        .topmost_line()
        .0
        .max(grid.bottommost_line().0 - SEARCH_LINES as i32 + 1);
    let from = from.max(oldest);
    let to = to.min(grid.bottommost_line().0);
    let mut found = Vec::new();
    let mut cells = Vec::with_capacity(grid.columns());
    for line in from..=to {
        cells.clear();
        let row = &grid[Line(line)];
        for column in 0..grid.columns() {
            let cell = &row[Column(column)];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                2
            } else {
                1
            };
            let text = if cell.flags.contains(Flags::HIDDEN) {
                ' '
            } else {
                cell.c
            };
            let start = column as u16;
            cells.push((text, start, start + width - 1));
        }
        found.extend(
            occurrences(&cells, needle)
                .into_iter()
                .map(|(start, end)| Match { line, start, end }),
        );
    }
    found
}

/// All matches in the searchable part of the grid, oldest first.
pub fn all_matches<T: EventListener>(term: &Term<T>, needle: &[char]) -> Vec<Match> {
    matches_in(term, needle, i32::MIN, i32::MAX)
}

/// Which match a search moves to.
///
/// `anchor` is where the previous match was, if the same query is being
/// repeated; otherwise the search starts from the viewport (`top..=bottom`):
/// forward at its first line, backward at its last.
pub fn choose(
    matches: &[Match],
    anchor: Option<Match>,
    forward: bool,
    top: i32,
    bottom: i32,
) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let key = |m: &Match| (m.line, m.start);
    let index = match (anchor, forward) {
        (Some(anchor), true) => matches
            .iter()
            .position(|m| key(m) > key(&anchor))
            .unwrap_or(0),
        (Some(anchor), false) => matches
            .iter()
            .rposition(|m| key(m) < key(&anchor))
            .unwrap_or(matches.len() - 1),
        (None, true) => matches.iter().position(|m| m.line >= top).unwrap_or(0),
        (None, false) => matches
            .iter()
            .rposition(|m| m.line <= bottom)
            .unwrap_or(matches.len() - 1),
    };
    Some(index)
}

#[cfg(test)]
mod tests {
    use super::{Match, all_matches, choose, needle, occurrences};
    use alacritty_terminal::{
        event::VoidListener,
        grid::Dimensions,
        term::{Config, Term},
        vte::ansi::Processor,
    };

    struct Size(usize, usize);
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            self.0
        }
        fn screen_lines(&self) -> usize {
            self.0
        }
        fn columns(&self) -> usize {
            self.1
        }
    }

    fn term_with(rows: usize, cols: usize, text: &str) -> Term<VoidListener> {
        let mut term = Term::new(Config::default(), &Size(rows, cols), VoidListener);
        let mut parser: Processor = Processor::default();
        parser.advance(&mut term, text.as_bytes());
        term
    }

    #[test]
    fn matching_ignores_case_and_reports_cell_columns() {
        let cells: Vec<_> = "Say HELLO, hello"
            .chars()
            .enumerate()
            .map(|(i, c)| (c, i as u16, i as u16))
            .collect();
        assert_eq!(occurrences(&cells, &needle("hello")), [(4, 8), (11, 15)]);
        assert!(occurrences(&cells, &needle("")).is_empty());
    }

    #[test]
    fn a_wide_character_covers_two_columns() {
        let cells = [('a', 0, 0), ('界', 1, 2), ('b', 3, 3)];
        assert_eq!(occurrences(&cells, &needle("界b")), [(1, 3)]);
    }

    #[test]
    fn scrollback_is_searched_along_with_the_screen() {
        // Three rows, five lines written: the first two scroll into history.
        let term = term_with(3, 20, "needle one\r\nplain\r\nNEEDLE two\r\nplain\r\nlast");
        let found = all_matches(&term, &needle("needle"));
        assert_eq!(
            found,
            [
                Match {
                    line: -2,
                    start: 0,
                    end: 5
                },
                Match {
                    line: 0,
                    start: 0,
                    end: 5
                }
            ]
        );
    }

    #[test]
    fn a_wide_character_in_the_grid_is_matched_by_column() {
        let term = term_with(2, 10, "x界y");
        assert_eq!(
            all_matches(&term, &needle("y")),
            [Match {
                line: 0,
                start: 3,
                end: 3
            }]
        );
    }

    #[test]
    fn repeating_a_query_walks_the_matches_and_wraps() {
        let at = |line| Match {
            line,
            start: 0,
            end: 1,
        };
        let matches = [at(-10), at(-4), at(2)];
        // A fresh forward search starts at the viewport; backward ends at it.
        assert_eq!(choose(&matches, None, true, -5, 6), Some(1));
        assert_eq!(choose(&matches, None, false, -5, 6), Some(2));
        // Repeating continues from the previous match and wraps around.
        assert_eq!(choose(&matches, Some(at(-4)), true, 0, 0), Some(2));
        assert_eq!(choose(&matches, Some(at(2)), true, 0, 0), Some(0));
        assert_eq!(choose(&matches, Some(at(-10)), false, 0, 0), Some(2));
        assert_eq!(choose(&matches, Some(at(-4)), false, 0, 0), Some(0));
        assert_eq!(choose(&[], None, true, 0, 0), None);
    }
}
