//! The draggable divider between the two sides of a split.

use crate::{
    Message,
    ui::{ACCENT, BACKGROUND, BORDER},
};

/// The message a divider sends.
fn tell(message: super::Message) -> Message {
    Message::Splits(message)
}
use iced::{Point, Rectangle, Size, Theme, mouse, widget::canvas};
use spooky_core::{Axis, MIN_SPLIT_SHARE};
use std::time::{Duration, Instant};

/// Width of the gap a divider occupies between its two sides.
pub(crate) const THICKNESS: f32 = 6.0;
/// Two presses this close together reset the split to even halves.
const DOUBLE_PRESS: Duration = Duration::from_millis(400);

/// Share of a split's first side, in thousandths, when its divider sits at
/// `cursor`. `region` is the whole split, both sides and the divider between them.
pub(crate) fn ratio_at(region: Rectangle, axis: Axis, cursor: Point) -> u16 {
    let (origin, length, at) = match axis {
        Axis::Vertical => (region.x, region.width, cursor.x),
        Axis::Horizontal => (region.y, region.height, cursor.y),
    };
    // The divider's own thickness is not shared between the sides.
    let shared = (length - THICKNESS).max(1.0);
    let ratio = ((at - origin - THICKNESS / 2.0) / shared * 1000.0).round();
    ratio.clamp(
        f32::from(MIN_SPLIT_SHARE),
        f32::from(1000 - MIN_SPLIT_SHARE),
    ) as u16
}

pub(crate) struct Divider {
    pub split: u64,
    pub axis: Axis,
}

#[derive(Default)]
pub(crate) struct State {
    dragging: bool,
    last_press: Option<Instant>,
}

impl canvas::Program<Message> for Divider {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if cursor.is_over(bounds) =>
            {
                let again = state
                    .last_press
                    .is_some_and(|at| at.elapsed() < DOUBLE_PRESS);
                state.last_press = Some(Instant::now());
                if again {
                    state.dragging = false;
                    return Some(
                        canvas::Action::publish(tell(super::Message::Reset(self.split)))
                            .and_capture(),
                    );
                }
                state.dragging = true;
                Some(
                    canvas::Action::publish(tell(super::Message::Drag(
                        self.split,
                        cursor.position()?,
                    )))
                    .and_capture(),
                )
            }
            // The pointer may leave the thin divider while it is dragged.
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if state.dragging => Some(
                canvas::Action::publish(tell(super::Message::Drag(self.split, cursor.position()?)))
                    .and_capture(),
            ),
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.dragging =>
            {
                state.dragging = false;
                Some(canvas::Action::publish(tell(super::Message::End)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &State,
        renderer: &iced::Renderer,
        _: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        // The gap takes the terminals' own background, so the two panes read as
        // one surface with a single hairline between them.
        frame.fill_rectangle(Point::ORIGIN, bounds.size(), BACKGROUND);
        let active = state.dragging || cursor.is_over(bounds);
        let (thickness, color) = if active {
            (2.0, ACCENT.scale_alpha(0.7))
        } else {
            (1.0, BORDER)
        };
        let (origin, size) = match self.axis {
            Axis::Vertical => (
                Point::new(((bounds.width - thickness) / 2.0).floor(), 0.0),
                Size::new(thickness, bounds.height),
            ),
            Axis::Horizontal => (
                Point::new(0.0, ((bounds.height - thickness) / 2.0).floor()),
                Size::new(bounds.width, thickness),
            ),
        };
        frame.fill_rectangle(origin, size, color);
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !(state.dragging || cursor.is_over(bounds)) {
            return mouse::Interaction::None;
        }
        match self.axis {
            Axis::Vertical => mouse::Interaction::ResizingHorizontally,
            Axis::Horizontal => mouse::Interaction::ResizingVertically,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{THICKNESS, ratio_at};
    use iced::{Point, Rectangle, Size};
    use spooky_core::{Axis, MIN_SPLIT_SHARE};

    fn region(width: f32, height: f32) -> Rectangle {
        Rectangle::new(Point::new(100.0, 40.0), Size::new(width, height))
    }

    #[test]
    fn a_divider_in_the_middle_of_the_gap_splits_the_space_evenly() {
        // 1006 wide: two sides of 500 and the 6 px divider between them.
        let region = region(500.0 * 2.0 + THICKNESS, 300.0);
        assert_eq!(
            ratio_at(region, Axis::Vertical, Point::new(100.0 + 503.0, 0.0)),
            500
        );
        // 300 tall: 147 + 6 + 147.
        let rows = region_of_height(300.0);
        assert_eq!(
            ratio_at(rows, Axis::Horizontal, Point::new(0.0, 40.0 + 150.0)),
            500
        );
    }

    fn region_of_height(height: f32) -> Rectangle {
        region(800.0, height)
    }

    #[test]
    fn the_ratio_follows_the_pointer_along_the_split_axis_only() {
        let region = region(1006.0, 400.0);
        // Moving the pointer across the other axis changes nothing.
        let a = ratio_at(region, Axis::Vertical, Point::new(100.0 + 253.0, 0.0));
        let b = ratio_at(region, Axis::Vertical, Point::new(100.0 + 253.0, 399.0));
        assert_eq!((a, b), (250, 250));
        assert_eq!(
            ratio_at(region, Axis::Vertical, Point::new(100.0 + 753.0, 0.0)),
            750
        );
    }

    #[test]
    fn dragging_past_either_end_keeps_a_visible_share_on_both_sides() {
        let region = region(1006.0, 400.0);
        assert_eq!(
            ratio_at(region, Axis::Vertical, Point::new(-500.0, 0.0)),
            MIN_SPLIT_SHARE
        );
        assert_eq!(
            ratio_at(region, Axis::Vertical, Point::new(5000.0, 0.0)),
            1000 - MIN_SPLIT_SHARE
        );
    }
}
