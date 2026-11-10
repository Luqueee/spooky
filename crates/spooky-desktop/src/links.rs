//! Links in terminal output: OSC 8 hyperlinks and plain `http(s)://` URLs.
//!
//! What a program prints is untrusted. A link is only ever followed by an
//! explicit Cmd/Ctrl+click, only for the schemes in [`OPENABLE`], and its target
//! is shown before it is followed.

use spooky_core::Pane;
use std::{
    process::{Command, Stdio},
    thread,
};

/// Schemes a click may open. Everything else (`file:`, `ssh:`, `javascript:`,
/// application schemes) is refused: opening it would let output run things.
const OPENABLE: [&str; 3] = ["http", "https", "mailto"];

/// A link under the pointer: where it leads and the columns of `row` it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub row: u16,
    pub start: u16,
    /// Last column covered, inclusive.
    pub end: u16,
    pub target: String,
}

/// The scheme of `target`, lowercased, when it is a well-formed URL.
fn scheme(target: &str) -> Option<String> {
    let (scheme, rest) = target.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c));
    (valid && !rest.is_empty() && !target.chars().any(|c| c.is_control() || c == ' '))
        .then(|| scheme.to_ascii_lowercase())
}

/// Whether a click may open `target`.
pub(crate) fn openable(target: &str) -> bool {
    scheme(target).is_some_and(|scheme| {
        OPENABLE.contains(&scheme.as_str())
            && target.split_once(':').is_some_and(|(_, rest)| {
                scheme == "mailto" || !rest.trim_start_matches('/').is_empty()
            })
    })
}

/// Opens `target` with the system's default handler.
pub(crate) fn open(target: &str) -> Result<(), String> {
    if !openable(target) {
        return Err(match scheme(target) {
            Some(scheme) => format!(
                "Not opening a {scheme}: link; only {} links are opened",
                OPENABLE.join(", ")
            ),
            None => "Not opening a malformed link".to_owned(),
        });
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(program)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Cannot run {program}: {error}"))?;
    // Reaped on its own thread so the handler never lingers as a zombie.
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The link covering `col` of `row`, if any. An OSC 8 hyperlink wins over a URL
/// that merely appears in the text.
pub(crate) fn link_at(pane: &Pane, row: u16, col: u16) -> Option<Link> {
    let line = pane.lines.iter().find(|line| line.row == row)?;
    hyperlink_at(line, row, col).or_else(|| url_at(line, row, col))
}

/// Merges adjacent runs sharing one OSC 8 target into the span that covers `col`.
fn hyperlink_at(line: &spooky_core::Line, row: u16, col: u16) -> Option<Link> {
    let hit = line.runs.iter().find(|run| {
        run.link.is_some() && run.col <= col && col < run.col.saturating_add(run.columns())
    })?;
    let target = hit.link.as_deref()?;
    let covers = |run: &spooky_core::Run| run.link.as_deref() == Some(target);
    let mut start = hit.col;
    let mut end = hit.col.saturating_add(hit.columns()).saturating_sub(1);
    // Runs are in column order; extend over neighbours that touch the span.
    loop {
        let before = (start, end);
        for run in line.runs.iter().filter(|run| covers(run)) {
            let run_end = run.col.saturating_add(run.columns()).saturating_sub(1);
            if run.col <= end.saturating_add(1) && run_end.saturating_add(1) >= start {
                start = start.min(run.col);
                end = end.max(run_end);
            }
        }
        if (start, end) == before {
            break;
        }
    }
    Some(Link {
        row,
        start,
        end,
        target: target.to_owned(),
    })
}

/// A character of the row with the columns it occupies.
struct Glyph {
    c: char,
    start: u16,
    end: u16,
}

/// The row's characters in column order. Columns no run covers are blank, which
/// ends any URL.
fn glyphs(line: &spooky_core::Line) -> Vec<Glyph> {
    let mut glyphs: Vec<Glyph> = Vec::new();
    for run in &line.runs {
        for (col, cell, width) in run.cells() {
            if let Some(last) = glyphs.last()
                && last.end.saturating_add(1) < col
            {
                glyphs.push(Glyph {
                    c: ' ',
                    start: last.end + 1,
                    end: col - 1,
                });
            }
            let end = col.saturating_add(width).saturating_sub(1);
            for c in cell.chars() {
                glyphs.push(Glyph { c, start: col, end });
            }
        }
    }
    glyphs
}

/// Characters that may be part of a URL in running text.
fn url_char(c: char) -> bool {
    !c.is_whitespace() && !c.is_control() && !"<>\"'`".contains(c)
}

/// A plain `http://` or `https://` URL covering `col`. Punctuation that
/// usually closes a sentence, and brackets the URL does not itself open, are
/// not part of it.
fn url_at(line: &spooky_core::Line, row: u16, col: u16) -> Option<Link> {
    let glyphs = glyphs(line);
    let text: Vec<char> = glyphs.iter().map(|glyph| glyph.c).collect();
    let mut at = 0;
    while at < text.len() {
        if !matches!(text[at], 'h' | 'H') {
            at += 1;
            continue;
        }
        let rest: String = text[at..]
            .iter()
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase();
        let scheme_len = if rest.starts_with("https://") {
            8
        } else if rest.starts_with("http://") {
            7
        } else {
            at += 1;
            continue;
        };
        if at > 0 && text[at - 1].is_alphanumeric() {
            at += scheme_len;
            continue;
        }
        let mut stop = at + scheme_len;
        while stop < text.len() && url_char(text[stop]) {
            stop += 1;
        }
        while stop > at + scheme_len && closes_a_sentence(&text[at..stop]) {
            stop -= 1;
        }
        if stop > at + scheme_len && glyphs[at].start <= col && col <= glyphs[stop - 1].end {
            return Some(Link {
                row,
                start: glyphs[at].start,
                end: glyphs[stop - 1].end,
                target: text[at..stop].iter().collect(),
            });
        }
        at = stop.max(at + scheme_len);
    }
    None
}

/// Whether the last character of `url` is punctuation that belongs to the
/// sentence around it, not to the URL.
fn closes_a_sentence(url: &[char]) -> bool {
    let Some(&last) = url.last() else {
        return false;
    };
    match last {
        '.' | ',' | ';' | ':' | '!' | '?' => true,
        ')' | ']' | '}' => {
            let open = match last {
                ')' => '(',
                ']' => '[',
                _ => '{',
            };
            url.iter().filter(|&&c| c == last).count() > url.iter().filter(|&&c| c == open).count()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Link, link_at, openable};
    use spooky_core::{Cursor, CursorShape, Line, MouseEncoding, MouseTracking, Pane, Run};

    fn run(col: u16, text: &str, link: Option<&str>) -> Run {
        Run {
            col,
            text: text.into(),
            fg: [1, 2, 3],
            bg: spooky_core::DEFAULT_BACKGROUND,
            bold: false,
            italic: false,
            underline: false,
            strikethrough: false,
            width: 0,
            link: link.map(str::to_owned),
        }
    }

    fn pane(runs: Vec<Run>) -> Pane {
        Pane {
            id: 1,
            title: String::new(),
            cols: 80,
            rows: 1,
            lines: vec![Line { row: 0, runs }],
            cursor: Cursor {
                row: 0,
                col: 0,
                visible: true,
                shape: CursorShape::Beam,
                blinking: false,
            },
            exited: false,
            bracketed_paste: false,
            application_cursor: false,
            mouse_tracking: MouseTracking::Off,
            mouse_encoding: MouseEncoding::Normal,
            alternate_scroll: false,
            history: 0,
            scrolled: 0,
            focus_reporting: false,
            clipboard_seq: 0,
            highlights: Vec::new(),
        }
    }

    fn target(pane: &Pane, col: u16) -> Option<String> {
        link_at(pane, 0, col).map(|link| link.target)
    }

    #[test]
    fn a_plain_url_is_found_by_column_without_the_sentence_around_it() {
        let pane = pane(vec![run(0, "see https://example.com/a_b. next", None)]);
        let url = "https://example.com/a_b";
        assert_eq!(target(&pane, 3), None);
        assert_eq!(
            link_at(&pane, 0, 4),
            Some(Link {
                row: 0,
                start: 4,
                end: 4 + url.len() as u16 - 1,
                target: url.into()
            })
        );
        assert_eq!(
            target(&pane, 4 + url.len() as u16 - 1).as_deref(),
            Some(url)
        );
        assert_eq!(
            target(&pane, 4 + url.len() as u16),
            None,
            "the full stop is not the link"
        );
        assert_eq!(target(&pane, 28), None);
    }

    #[test]
    fn brackets_belong_to_the_url_only_when_it_opens_them() {
        let wiki = pane(vec![run(
            0,
            "(https://en.wikipedia.org/wiki/Rust_(language))",
            None,
        )]);
        assert_eq!(
            target(&wiki, 10).as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        let quoted = pane(vec![run(
            0,
            "<https://example.com/x>, \"https://example.com/y\"",
            None,
        )]);
        assert_eq!(target(&quoted, 5).as_deref(), Some("https://example.com/x"));
        assert_eq!(
            target(&quoted, 30).as_deref(),
            Some("https://example.com/y")
        );
    }

    #[test]
    fn a_url_ends_at_a_blank_column_and_needs_something_after_the_scheme() {
        let split = pane(vec![
            run(0, "https://a.example/one", None),
            run(24, "tail", None),
        ]);
        assert_eq!(target(&split, 2).as_deref(), Some("https://a.example/one"));
        assert_eq!(target(&split, 24), None);
        let bare = pane(vec![run(0, "http:// and https://", None)]);
        assert_eq!(target(&bare, 2), None);
        let embedded = pane(vec![run(0, "xhttps://example.com", None)]);
        assert_eq!(target(&embedded, 8), None, "not at a word boundary");
    }

    #[test]
    fn an_osc_8_hyperlink_covers_its_runs_and_wins_over_the_text() {
        let pane = pane(vec![
            run(0, "go ", None),
            run(3, "https://good.example", Some("https://evil.example/x")),
            run(23, " now", None),
        ]);
        let link = link_at(&pane, 0, 5).expect("link");
        assert_eq!(link.target, "https://evil.example/x");
        assert_eq!((link.start, link.end), (3, 22));
        assert_eq!(target(&pane, 2), None);
        assert_eq!(target(&pane, 23), None);
    }

    #[test]
    fn adjacent_runs_with_one_target_are_one_link() {
        let pane = pane(vec![
            run(0, "ab", Some("https://example.com")),
            run(2, "cd", Some("https://example.com")),
            run(4, "ef", Some("https://other.example")),
        ]);
        let link = link_at(&pane, 0, 3).expect("link");
        assert_eq!((link.start, link.end), (0, 3));
        assert_eq!(target(&pane, 4).as_deref(), Some("https://other.example"));
    }

    #[test]
    fn only_web_and_mail_links_may_be_opened() {
        for ok in [
            "https://example.com",
            "HTTP://example.com/a?b=c#d",
            "mailto:me@example.com",
        ] {
            assert!(openable(ok), "{ok}");
        }
        for refused in [
            "file:///etc/passwd",
            "ssh://host",
            "javascript:alert(1)",
            "x-apple.systempreferences:com.apple.preference",
            "https://",
            "https:///",
            "https://exa mple.com",
            "https://example.com/\u{7}",
            " https://example.com",
            "example.com",
            "",
        ] {
            assert!(!openable(refused), "{refused:?}");
        }
    }
}
