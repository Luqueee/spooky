use alacritty_terminal::vte::ansi::{Color, NamedColor};
use spooky_core::{Line, Run};

/// The last rendered viewport of a pane and the revision at which each of its
/// rows last changed, so any client can be sent exactly the rows it lacks.
#[derive(Default)]
pub(crate) struct RowCache {
    pub(crate) cols: u16,
    pub(crate) lines: Vec<Line>,
    pub(crate) revisions: Vec<u64>,
}
impl RowCache {
    /// Records a render taken at `revision`. Rows that differ from the cached
    /// ones take that revision; a different grid size replaces every row.
    pub(crate) fn update(&mut self, cols: u16, rendered: Vec<Line>, revision: u64) {
        if self.cols != cols || self.lines.len() != rendered.len() {
            self.cols = cols;
            self.revisions = vec![revision; rendered.len()];
            self.lines = rendered;
            return;
        }
        for ((cached, changed_at), line) in
            self.lines.iter_mut().zip(&mut self.revisions).zip(rendered)
        {
            if *cached != line {
                *cached = line;
                *changed_at = revision;
            }
        }
    }
    /// Rows that changed after `since`; every row when `since` is `None`.
    pub(crate) fn changed_since(&self, since: Option<u64>) -> Vec<Line> {
        self.lines
            .iter()
            .zip(&self.revisions)
            .filter(|(_, changed_at)| since.is_none_or(|since| **changed_at > since))
            .map(|(line, _)| line.clone())
            .collect()
    }
}

pub(crate) struct Style {
    pub(crate) fg: [u8; 3],
    pub(crate) bg: [u8; 3],
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    pub(crate) strikethrough: bool,
}

/// Hyperlink targets longer than this are dropped.
const MAX_LINK_BYTES: usize = 2048;

/// An OSC 8 target the client may be shown: nonempty, bounded, and free of
/// control characters. The application chose it, so it stays untrusted.
fn link_target(uri: Option<&str>) -> Option<&str> {
    uri.filter(|uri| {
        !uri.is_empty() && uri.len() <= MAX_LINK_BYTES && !uri.chars().any(char::is_control)
    })
}

/// Appends one terminal cell to its row. Blank cells on the default background
/// are left implicit; neighbouring plain cells with the same style and link
/// share a run. A wide cell or one with combining characters is always a run
/// of its own.
pub(crate) fn push_cell(
    line: &mut Line,
    col: u16,
    text: String,
    style: Style,
    wide: bool,
    link: Option<&str>,
) {
    let link = link_target(link);
    if text == " "
        && style.bg == spooky_core::DEFAULT_BACKGROUND
        && !style.underline
        && !style.strikethrough
        && link.is_none()
    {
        return;
    }
    let single = wide || text.chars().nth(1).is_some();
    if !single
        && let Some(last) = line.runs.last_mut()
        && last.width == 0
        && last.fg == style.fg
        && last.bg == style.bg
        && last.bold == style.bold
        && last.italic == style.italic
        && last.underline == style.underline
        && last.strikethrough == style.strikethrough
        && last.link.as_deref() == link
        && last.col + last.columns() == col
    {
        last.text.push_str(&text);
        return;
    }
    line.runs.push(Run {
        col,
        text,
        fg: style.fg,
        bg: style.bg,
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strikethrough: style.strikethrough,
        width: if wide { 2 } else { u8::from(single) },
        link: link.map(str::to_owned),
    });
}

pub(crate) fn palette(index: usize) -> [u8; 3] {
    const BASE: [[u8; 3]; 16] = [
        [0, 0, 0],
        [205, 49, 49],
        [13, 188, 121],
        [229, 229, 16],
        [36, 114, 200],
        [188, 63, 188],
        [17, 168, 205],
        [229, 229, 229],
        [102, 102, 102],
        [241, 76, 76],
        [35, 209, 139],
        [245, 245, 67],
        [59, 142, 234],
        [214, 112, 214],
        [41, 184, 219],
        [255, 255, 255],
    ];
    match index {
        0..=15 => BASE[index],
        16..=231 => {
            let n = index - 16;
            let component = |v: usize| if v == 0 { 0 } else { (55 + 40 * v) as u8 };
            [component(n / 36), component((n / 6) % 6), component(n % 6)]
        }
        232..=255 => {
            let c = (8 + (index - 232) * 10) as u8;
            [c, c, c]
        }
        257 => spooky_core::DEFAULT_BACKGROUND,
        _ => [220, 220, 220],
    }
}
pub(crate) fn color(value: Color, colors: &alacritty_terminal::term::color::Colors) -> [u8; 3] {
    let index = match value {
        Color::Spec(rgb) => return [rgb.r, rgb.g, rgb.b],
        Color::Indexed(i) => usize::from(i),
        Color::Named(n) => n as usize,
    };
    if let Some(rgb) = colors[index] {
        [rgb.r, rgb.g, rgb.b]
    } else if index >= NamedColor::DimBlack as usize && index <= NamedColor::DimWhite as usize {
        let p = palette(index - NamedColor::DimBlack as usize);
        [p[0] / 2, p[1] / 2, p[2] / 2]
    } else {
        palette(index)
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_LINK_BYTES, link_target};

    #[test]
    fn a_link_target_must_be_bounded_and_free_of_control_characters() {
        assert_eq!(
            link_target(Some("https://example.com/a?b=c")),
            Some("https://example.com/a?b=c")
        );
        assert_eq!(link_target(None), None);
        assert_eq!(link_target(Some("")), None);
        assert_eq!(link_target(Some("https://exa\u{7}mple.com")), None);
        assert_eq!(link_target(Some("https://example.com/\n")), None);
        let long = format!("https://example.com/{}", "a".repeat(MAX_LINK_BYTES));
        assert_eq!(link_target(Some(&long)), None);
    }
}
