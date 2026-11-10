use super::App;
use crate::{
    layout::find_split,
    terminal::{self},
};
use iced::Rectangle;
use spooky_core::{Axis, Layout, Request};

impl App {
    /// Tells the server the grid a pane's area holds, when that changed.
    pub(super) fn fit(&mut self, pane: u64, area: Rectangle) {
        let (cw, ch) = terminal::metrics(self.config.font_size);
        let cols = ((area.width / cw).floor() as u16).max(1);
        let rows = ((area.height / ch).floor() as u16).max(1);
        if self.grids.get(&pane) != Some(&(cols, rows)) {
            self.grids.insert(pane, (cols, rows));
            self.send(Request::Resize { pane, cols, rows });
        }
    }
    /// Refits every pane after the font changed: the areas stay, the grids do not.
    pub(super) fn refit_grids(&mut self) {
        let areas: Vec<(u64, Rectangle)> = self
            .bounds
            .iter()
            .map(|(pane, area)| (*pane, *area))
            .collect();
        for (pane, area) in areas {
            self.fit(pane, area);
        }
    }
    /// The split's axis and the window area it occupies, both sides included.
    pub(super) fn split_geometry(&self, split: u64) -> Option<(Axis, iced::Rectangle)> {
        let (_, tab) = self.active()?;
        let (axis, node) = find_split(&tab.layout, split)?;
        let mut ids = Vec::new();
        node.panes(&mut ids);
        let mut region: Option<iced::Rectangle> = None;
        for id in ids {
            let inner = self.bounds.get(&id)?;
            let outer = iced::Rectangle {
                x: inner.x - super::view::PANE_PADDING_X,
                y: inner.y - super::view::PANE_PADDING_Y,
                width: inner.width + 2.0 * super::view::PANE_PADDING_X,
                height: inner.height + 2.0 * super::view::PANE_PADDING_Y,
            };
            region = Some(region.map_or(outer, |region| region.union(&outer)));
        }
        Some((axis, region?))
    }
    /// The ratio the selected tab's layout gives `split`.
    pub(super) fn split_ratio(&self, split: u64) -> Option<u16> {
        let (_, tab) = self.active()?;
        match find_split(&tab.layout, split)?.1 {
            Layout::Split { ratio, .. } => Some(*ratio),
            Layout::Pane(_) => None,
        }
    }
}
