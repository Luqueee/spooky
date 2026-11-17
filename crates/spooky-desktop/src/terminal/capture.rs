//! A pane as a picture, for attaching to an agent: the cells drawn on the CPU with the
//! terminal's font, so it needs no window and shows what the program printed, without the
//! cursor or a selection.

use super::{TerminalFont, font::face_data, metrics};
use crate::ui::TerminalColors;
use spooky_core::{DEFAULT_BACKGROUND, Pane};
use std::collections::HashMap;

/// How many device pixels each point of the terminal gets: the picture is sharper than
/// the window.
const SCALE: f32 = 2.0;
/// The most pixels a picture may have, so a huge grid cannot exhaust memory.
const MOST_PIXELS: u64 = 64 * 1024 * 1024;

/// A rendered pane: RGBA pixels, row after row.
pub(crate) struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// `foreground` laid over `background` with `coverage` (0 to 255) of it showing.
fn blend(background: u8, foreground: u8, coverage: u8) -> u8 {
    let coverage = u16::from(coverage);
    let mixed = u16::from(foreground) * coverage + u16::from(background) * (255 - coverage);
    ((mixed + 127) / 255) as u8
}

/// The size of the picture of a grid, or `None` when it would be too large to make.
fn picture_size(cols: u16, rows: u16, cell: (f32, f32)) -> Option<(u32, u32)> {
    let width = (f32::from(cols) * cell.0).ceil() as u32;
    let height = (f32::from(rows) * cell.1).ceil() as u32;
    (width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MOST_PIXELS)
        .then_some((width, height))
}

/// One face of the font, which draws the glyphs of one style.
struct Face {
    font: fontdue::Font,
    data: Vec<u8>,
    index: u32,
}

impl Face {
    fn load(family: &str, bold: bool, italic: bool, scale: f32) -> Option<Self> {
        let (data, index) = face_data(family, bold, italic)?;
        let font = fontdue::Font::from_bytes(
            data.as_slice(),
            fontdue::FontSettings {
                collection_index: index,
                scale,
                ..fontdue::FontSettings::default()
            },
        )
        .ok()?;
        Some(Self { font, data, index })
    }
}

/// The faces of a family, loaded when first asked for, and the glyphs drawn so far.
struct Faces<'a> {
    family: &'a str,
    size: f32,
    loaded: HashMap<(bool, bool), Option<Face>>,
    glyphs: HashMap<(char, bool, bool), (fontdue::Metrics, Vec<u8>)>,
}

impl Faces<'_> {
    /// The face of a style; the regular one stands in for a style the family lacks.
    fn face(&mut self, bold: bool, italic: bool) -> Option<&Face> {
        let (family, size) = (self.family, self.size);
        let wanted = self
            .loaded
            .entry((bold, italic))
            .or_insert_with(|| Face::load(family, bold, italic, size));
        if wanted.is_some() {
            return self.loaded.get(&(bold, italic))?.as_ref();
        }
        self.loaded
            .entry((false, false))
            .or_insert_with(|| Face::load(family, false, false, size))
            .as_ref()
    }

    /// The coverage bitmap of a character, and where it sits in its cell.
    fn glyph(
        &mut self,
        character: char,
        bold: bool,
        italic: bool,
    ) -> Option<&(fontdue::Metrics, Vec<u8>)> {
        if !self.glyphs.contains_key(&(character, bold, italic)) {
            let size = self.size;
            let face = self.face(bold, italic)?;
            // Through the Unicode map only: the rasterizer's own lookup may pick a legacy
            // Mac Roman map and draw a different character.
            let id = ttf_parser::Face::parse(&face.data, face.index)
                .ok()?
                .glyph_index(character)?;
            let drawn = face.font.rasterize_indexed(id.0, size);
            self.glyphs.insert((character, bold, italic), drawn);
        }
        self.glyphs.get(&(character, bold, italic))
    }
}

/// The pane as a picture, or `None` when the font cannot be read or the grid is too large.
/// Its colours are drawn with `colors`, as the window draws them.
pub(crate) fn render(
    pane: &Pane,
    font: &TerminalFont,
    size: f32,
    colors: &TerminalColors,
) -> Option<Picture> {
    let scaled = size * SCALE;
    let (cell_width, cell_height) = metrics(scaled, font.advance);
    let (width, height) = picture_size(pane.cols, pane.rows, (cell_width, cell_height))?;
    let mut rgba = vec![0_u8; usize::try_from(u64::from(width) * u64::from(height) * 4).ok()?];
    for pixel in rgba.as_chunks_mut::<4>().0.iter_mut() {
        pixel[..3].copy_from_slice(&DEFAULT_BACKGROUND);
        pixel[3] = 255;
    }
    let mut faces = Faces {
        family: &font.family,
        size: scaled,
        loaded: HashMap::new(),
        glyphs: HashMap::new(),
    };
    let ascent = faces
        .face(false, false)?
        .font
        .horizontal_line_metrics(scaled)
        .map_or(scaled * 0.8, |lines| lines.ascent);
    let descent = faces
        .face(false, false)?
        .font
        .horizontal_line_metrics(scaled)
        .map_or(scaled * 0.2, |lines| -lines.descent);
    let baseline = (cell_height - (ascent + descent)) / 2.0 + ascent;
    for line in &pane.lines {
        let top = f32::from(line.row) * cell_height;
        for run in &line.runs {
            let left = f32::from(run.col) * cell_width;
            let (fg, bg) = (colors.resolve(run.fg), colors.resolve(run.bg));
            if run.explicit_bg || run.bg != DEFAULT_BACKGROUND {
                fill(
                    &mut rgba,
                    width,
                    height,
                    (left, top),
                    (f32::from(run.columns()) * cell_width, cell_height),
                    bg,
                );
            }
            for (column, text, _) in run.cells() {
                let Some(character) = text.chars().next().filter(|c| !c.is_whitespace()) else {
                    continue;
                };
                let Some((glyph, coverage)) = faces.glyph(character, run.bold, run.italic) else {
                    continue;
                };
                let x = f32::from(column) * cell_width + glyph.xmin as f32;
                let y = top + baseline - glyph.height as f32 - glyph.ymin as f32;
                draw(&mut rgba, width, height, (x, y), glyph, coverage, fg);
            }
            let (x, run_width) = (left, f32::from(run.columns()) * cell_width);
            if run.underline {
                fill(
                    &mut rgba,
                    width,
                    height,
                    (x, top + cell_height - SCALE * 2.0),
                    (run_width, SCALE),
                    fg,
                );
            }
            if run.strikethrough {
                fill(
                    &mut rgba,
                    width,
                    height,
                    (x, top + cell_height * 0.5),
                    (run_width, SCALE),
                    fg,
                );
            }
        }
    }
    Some(Picture {
        width,
        height,
        rgba,
    })
}

/// Fills a rectangle with a solid colour, cut off where the picture ends.
fn fill(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    (x, y): (f32, f32),
    (rect_width, rect_height): (f32, f32),
    color: [u8; 3],
) {
    let first_column = x.max(0.0).floor() as u32;
    let first_row = y.max(0.0).floor() as u32;
    let last_column = ((x + rect_width).ceil().max(0.0) as u32).min(width);
    let last_row = ((y + rect_height).ceil().max(0.0) as u32).min(height);
    for row in first_row..last_row {
        for column in first_column..last_column {
            let at = ((row * width + column) * 4) as usize;
            rgba[at..at + 3].copy_from_slice(&color);
        }
    }
}

/// Draws a glyph's coverage bitmap in `color` with its top left at `(x, y)`.
fn draw(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    (x, y): (f32, f32),
    glyph: &fontdue::Metrics,
    coverage: &[u8],
    color: [u8; 3],
) {
    let (x, y) = (x.round() as i64, y.round() as i64);
    for row in 0..glyph.height {
        for column in 0..glyph.width {
            let (px, py) = (x + column as i64, y + row as i64);
            if px < 0 || py < 0 || px >= i64::from(width) || py >= i64::from(height) {
                continue;
            }
            let amount = coverage[row * glyph.width + column];
            if amount == 0 {
                continue;
            }
            let at = ((py as u32 * width + px as u32) * 4) as usize;
            for channel in 0..3 {
                rgba[at + channel] = blend(rgba[at + channel], color[channel], amount);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_blends_with_the_background_in_proportion_to_its_coverage() {
        assert_eq!(blend(0, 200, 0), 0);
        assert_eq!(blend(0, 200, 255), 200);
        assert_eq!(blend(100, 200, 128), 150);
    }

    #[test]
    fn a_huge_grid_is_refused_instead_of_allocated() {
        assert_eq!(picture_size(80, 24, (16.0, 32.0)), Some((1280, 768)));
        assert_eq!(picture_size(60_000, 60_000, (16.0, 32.0)), None);
        assert_eq!(picture_size(0, 24, (16.0, 32.0)), None);
    }

    #[test]
    fn a_fill_stays_inside_the_picture() {
        let mut pixels = vec![0_u8; 4 * 4 * 4];
        fill(&mut pixels, 4, 4, (2.0, 2.0), (10.0, 10.0), [9, 9, 9]);
        let painted = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] == 9)
            .count();
        assert_eq!(painted, 4);
    }
}
