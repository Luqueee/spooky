//! Vector icons, stroked on a 24 by 24 grid.

use super::color::hsla;
use crate::ui::Color;
use gpui::{IntoElement, PathBuilder, Pixels, Styled, canvas, point, px};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icon {
    Folder,
    Plus,
    Terminal,
    SplitColumns,
    SplitRows,
    Zoom,
    More,
    Sidebar,
    Server,
    Chevron,
    ChevronRight,
    Close,
    Lock,
    Pencil,
    Settings,
    Search,
    ChevronUp,
    Copy,
    Paste,
    Agent,
    Send,
}

/// The strokes of an icon: polylines, as lists of points on the 24 by 24 grid.
fn strokes(kind: Icon) -> Vec<Vec<(f32, f32)>> {
    let square = |x0, y0, x1, y1| vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)];
    match kind {
        Icon::Plus => vec![vec![(12., 5.), (12., 19.)], vec![(5., 12.), (19., 12.)]],
        Icon::Terminal => vec![
            vec![(5., 6.), (10., 11.), (5., 16.)],
            vec![(12., 17.), (19., 17.)],
        ],
        Icon::Folder => vec![vec![
            (3., 18.),
            (3., 6.),
            (9., 6.),
            (11., 8.),
            (21., 8.),
            (21., 18.),
            (3., 18.),
        ]],
        Icon::SplitColumns => vec![square(4., 4., 20., 20.), vec![(12., 4.), (12., 20.)]],
        Icon::SplitRows => vec![square(4., 4., 20., 20.), vec![(4., 12.), (20., 12.)]],
        Icon::Sidebar => vec![square(4., 4., 20., 20.), vec![(10., 4.), (10., 20.)]],
        Icon::Server => vec![
            square(4., 4., 20., 20.),
            vec![(4., 12.), (20., 12.)],
            vec![(7., 8.), (9., 8.)],
            vec![(7., 16.), (9., 16.)],
        ],
        Icon::Zoom => vec![
            vec![(4., 9.), (4., 4.), (9., 4.)],
            vec![(15., 4.), (20., 4.), (20., 9.)],
            vec![(20., 15.), (20., 20.), (15., 20.)],
            vec![(9., 20.), (4., 20.), (4., 15.)],
        ],
        Icon::Chevron => vec![vec![(8., 10.), (12., 14.), (16., 10.)]],
        Icon::ChevronRight => vec![vec![(10., 8.), (14., 12.), (10., 16.)]],
        Icon::ChevronUp => vec![vec![(8., 14.), (12., 10.), (16., 14.)]],
        Icon::Search => vec![vec![(15.5, 15.5), (20., 20.)]],
        Icon::Close => vec![vec![(6., 6.), (18., 18.)], vec![(18., 6.), (6., 18.)]],
        Icon::Lock => vec![
            square(6., 10., 18., 20.),
            vec![
                (8., 10.),
                (8., 7.),
                (9., 4.),
                (12., 3.),
                (15., 4.),
                (16., 7.),
                (16., 10.),
            ],
        ],
        Icon::Pencil => vec![vec![
            (5., 16.),
            (5., 20.),
            (9., 20.),
            (20., 9.),
            (16., 5.),
            (5., 16.),
            (9., 20.),
        ]],
        Icon::Settings => vec![
            vec![(4., 7.), (20., 7.)],
            vec![(4., 12.), (20., 12.)],
            vec![(4., 17.), (20., 17.)],
        ],
        Icon::Copy => vec![
            square(9., 9., 20., 20.),
            vec![(5., 15.), (5., 4.), (15., 4.), (15., 9.)],
        ],
        Icon::Send => vec![
            vec![(12., 19.), (12., 5.)],
            vec![(6., 11.), (12., 5.), (18., 11.)],
        ],
        Icon::Agent => vec![vec![
            (12., 3.),
            (14., 10.),
            (21., 12.),
            (14., 14.),
            (12., 21.),
            (10., 14.),
            (3., 12.),
            (10., 10.),
            (12., 3.),
        ]],
        Icon::Paste => vec![square(5., 5., 19., 21.), square(9., 3., 15., 7.)],
        Icon::More => Vec::new(),
    }
}

/// Filled dots: centre and radius on the grid.
fn dots(kind: Icon) -> Vec<(f32, f32, f32)> {
    match kind {
        Icon::More => vec![(5., 12., 1.1), (12., 12., 1.1), (19., 12., 1.1)],
        Icon::Settings => vec![(9., 7., 2.2), (15., 12., 2.2), (9., 17., 2.2)],
        _ => Vec::new(),
    }
}

/// A circle outline on the grid, for the icons that have one.
fn ring(kind: Icon) -> Option<(f32, f32, f32)> {
    (kind == Icon::Search).then_some((11., 11., 6.))
}

/// The points of a circle, as a closed polygon fine enough to look round at icon sizes.
fn circle(cx: f32, cy: f32, radius: f32) -> Vec<(f32, f32)> {
    const STEPS: usize = 28;
    (0..=STEPS)
        .map(|step| {
            let angle = step as f32 / STEPS as f32 * std::f32::consts::TAU;
            (cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

/// An icon `side` pixels wide, in `color`.
pub(crate) fn icon(kind: Icon, color: Color, side: f32) -> impl IntoElement {
    let color = hsla(color);
    let scale = side / 24.0;
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let at = |x: f32, y: f32| -> gpui::Point<Pixels> {
                point(
                    bounds.origin.x + px(x * scale),
                    bounds.origin.y + px(y * scale),
                )
            };
            let mut lines = strokes(kind);
            if let Some((x, y, radius)) = ring(kind) {
                lines.push(circle(x, y, radius));
            }
            for line in lines {
                let mut builder = PathBuilder::stroke(px(1.6 * scale));
                for (index, (x, y)) in line.into_iter().enumerate() {
                    if index == 0 {
                        builder.move_to(at(x, y));
                    } else {
                        builder.line_to(at(x, y));
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            }
            for (x, y, radius) in dots(kind) {
                let mut builder = PathBuilder::fill();
                for (index, (px_, py_)) in circle(x, y, radius).into_iter().enumerate() {
                    if index == 0 {
                        builder.move_to(at(px_, py_));
                    } else {
                        builder.line_to(at(px_, py_));
                    }
                }
                builder.close();
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .size(px(side))
}
