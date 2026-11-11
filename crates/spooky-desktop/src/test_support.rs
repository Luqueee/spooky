//! Fixtures shared by unit tests.

pub(crate) fn pane(rows: &[&str]) -> spooky_core::Pane {
    let cols = rows
        .iter()
        .map(|row| row.chars().count())
        .max()
        .unwrap_or(0);
    let lines = rows
        .iter()
        .enumerate()
        .map(|(row, text)| spooky_core::Line {
            row: row as u16,
            runs: if text.trim_end().is_empty() {
                Vec::new()
            } else {
                vec![spooky_core::Run {
                    col: 0,
                    text: text.trim_end().to_owned(),
                    fg: [0; 3],
                    bg: [0; 3],
                    bold: false,
                    italic: false,
                    underline: false,
                    strikethrough: false,
                    width: 0,
                    link: None,
                }]
            },
        })
        .collect();
    spooky_core::Pane {
        id: 1,
        title: String::new(),
        cols: cols as u16,
        rows: rows.len() as u16,
        lines,
        cursor: spooky_core::Cursor {
            row: 0,
            col: 0,
            visible: false,
            shape: spooky_core::CursorShape::Beam,
            blinking: false,
        },
        exited: false,
        bracketed_paste: false,
        application_cursor: false,
        mouse_tracking: spooky_core::MouseTracking::Off,
        mouse_encoding: spooky_core::MouseEncoding::Normal,
        alternate_scroll: false,
        history: 0,
        scrolled: 0,
        focus_reporting: false,
        clipboard_seq: 0,
        highlights: Vec::new(),
        agent: None,
    }
}
