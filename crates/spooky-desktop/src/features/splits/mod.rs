//! Dragging the divider between two split panes. The divider widget lives here; the
//! server owns the position, so this keeps the ratio as dragged only until the server
//! shows it.

mod divider;

pub(crate) use divider::{Divider, THICKNESS};

use divider::ratio_at;
use iced::{Point, Rectangle};
use spooky_core::{Axis, EVEN_SPLIT, Request};
use std::time::{Duration, Instant};

/// How often a dragged position is sent: the pointer moves faster than the server
/// needs to hear.
const SEND_EVERY: Duration = Duration::from_millis(40);
/// How long a dragged ratio is kept waiting for the server to show it.
const SETTLE_AFTER: Duration = Duration::from_secs(1);

/// What a divider tells the application.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Message {
    /// The divider of this split is at this window position.
    Drag(u64, Point),
    /// The drag ended.
    End,
    /// The divider was pressed twice: back to even halves.
    Reset(u64),
}

/// The ratio being dragged, until the server confirms it.
#[derive(Default)]
pub(crate) struct Splits {
    /// A divider's ratio as dragged, shown until the server confirms it.
    dragged: Option<(u64, u16)>,
    dragging: bool,
    /// When a position was last sent to the server.
    sent: Option<Instant>,
}

impl Splits {
    /// The ratio to draw for `split` instead of the server's, while it is being moved.
    pub(crate) fn dragged_ratio(&self, split: u64) -> Option<u16> {
        self.dragged
            .filter(|(dragged, _)| *dragged == split)
            .map(|(_, ratio)| ratio)
    }

    /// Applies a message. `geometry` gives a split's axis and the window area it
    /// occupies. Returns the request to send, if one is due.
    pub(crate) fn update(
        &mut self,
        message: Message,
        geometry: impl FnOnce(u64) -> Option<(Axis, Rectangle)>,
        now: Instant,
    ) -> Option<Request> {
        match message {
            Message::Drag(split, at) => {
                let (axis, region) = geometry(split)?;
                let ratio = ratio_at(region, axis, at);
                self.dragging = true;
                if self.dragged == Some((split, ratio)) {
                    return None;
                }
                self.dragged = Some((split, ratio));
                if self
                    .sent
                    .is_none_or(|sent| now.duration_since(sent) >= SEND_EVERY)
                {
                    self.sent = Some(now);
                    return Some(Request::SetSplitRatio { split, ratio });
                }
                None
            }
            Message::End => {
                self.dragging = false;
                let (split, ratio) = self.dragged?;
                self.sent = Some(now);
                Some(Request::SetSplitRatio { split, ratio })
            }
            Message::Reset(split) => {
                self.dragging = false;
                self.dragged = Some((split, EVEN_SPLIT));
                self.sent = Some(now);
                Some(Request::SetSplitRatio {
                    split,
                    ratio: EVEN_SPLIT,
                })
            }
        }
    }

    /// Drops the dragged ratio once the server shows it (or the split is gone), or
    /// has had long enough to. `current` is the ratio the server's layout gives.
    pub(crate) fn settle(&mut self, current: impl FnOnce(u64) -> Option<u16>, now: Instant) {
        let Some((split, ratio)) = self.dragged else {
            return;
        };
        if self.dragging {
            return;
        }
        let overdue = self
            .sent
            .is_none_or(|sent| now.duration_since(sent) > SETTLE_AFTER);
        if current(split).is_none_or(|current| current == ratio) || overdue {
            self.dragged = None;
        }
    }

    /// The server refused something: forget the dragged ratio unless still dragging.
    pub(crate) fn failed(&mut self) {
        if !self.dragging {
            self.dragged = None;
        }
    }

    /// Forgets everything, as when the target changes.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Size;

    const SPLIT: u64 = 7;

    fn region() -> Option<(Axis, Rectangle)> {
        // Two sides of 500 and the divider between them.
        Some((
            Axis::Vertical,
            Rectangle::new(Point::ORIGIN, Size::new(1000.0 + THICKNESS, 300.0)),
        ))
    }

    fn ratio_of(request: Option<Request>) -> Option<u16> {
        match request {
            Some(Request::SetSplitRatio {
                split: SPLIT,
                ratio,
            }) => Some(ratio),
            other => {
                assert!(other.is_none(), "unexpected {other:?}");
                None
            }
        }
    }

    fn drag(splits: &mut Splits, x: f32, now: Instant) -> Option<Request> {
        splits.update(Message::Drag(SPLIT, Point::new(x, 10.0)), |_| region(), now)
    }

    #[test]
    fn a_drag_is_shown_at_once_but_sent_at_a_gentler_pace() {
        let mut splits = Splits::default();
        let start = Instant::now();
        assert_eq!(ratio_of(drag(&mut splits, 503.0, start)), Some(500));
        assert_eq!(splits.dragged_ratio(SPLIT), Some(500));
        // Within the interval the new position is shown but not sent.
        assert_eq!(ratio_of(drag(&mut splits, 303.0, start)), None);
        assert_eq!(splits.dragged_ratio(SPLIT), Some(300));
        assert_eq!(
            ratio_of(drag(&mut splits, 253.0, start + SEND_EVERY)),
            Some(250)
        );
        assert_eq!(splits.dragged_ratio(8), None, "only the dragged split");
    }

    #[test]
    fn letting_go_sends_the_last_position_even_inside_the_interval() {
        let mut splits = Splits::default();
        let start = Instant::now();
        drag(&mut splits, 503.0, start);
        drag(&mut splits, 303.0, start);
        let sent = splits.update(Message::End, |_| None, start);
        assert_eq!(ratio_of(sent), Some(300));
        assert!(splits.update(Message::End, |_| None, start).is_some());
        let mut idle = Splits::default();
        assert!(idle.update(Message::End, |_| None, start).is_none());
    }

    #[test]
    fn pressing_twice_goes_back_to_even_halves() {
        let mut splits = Splits::default();
        let sent = splits.update(Message::Reset(SPLIT), |_| None, Instant::now());
        assert_eq!(ratio_of(sent), Some(EVEN_SPLIT));
        assert_eq!(splits.dragged_ratio(SPLIT), Some(EVEN_SPLIT));
    }

    #[test]
    fn a_split_that_cannot_be_found_is_not_dragged() {
        let mut splits = Splits::default();
        let sent = splits.update(
            Message::Drag(SPLIT, Point::new(1.0, 1.0)),
            |_| None,
            Instant::now(),
        );
        assert!(sent.is_none());
        assert_eq!(splits.dragged_ratio(SPLIT), None);
    }

    #[test]
    fn the_dragged_ratio_is_dropped_when_the_server_shows_it_or_takes_too_long() {
        let start = Instant::now();
        let dragged = |sent_at: Instant| {
            let mut splits = Splits::default();
            splits.update(Message::Reset(SPLIT), |_| None, sent_at);
            splits
        };
        // The server still shows the old ratio, recently: keep waiting.
        let mut splits = dragged(start);
        splits.settle(|_| Some(300), start);
        assert_eq!(splits.dragged_ratio(SPLIT), Some(EVEN_SPLIT));
        // The server caught up.
        splits.settle(|_| Some(EVEN_SPLIT), start);
        assert_eq!(splits.dragged_ratio(SPLIT), None);
        // The split is gone.
        let mut splits = dragged(start);
        splits.settle(|_| None, start);
        assert_eq!(splits.dragged_ratio(SPLIT), None);
        // It never showed up: give up after the grace period.
        let mut splits = dragged(start);
        splits.settle(
            |_| Some(300),
            start + SETTLE_AFTER + Duration::from_millis(1),
        );
        assert_eq!(splits.dragged_ratio(SPLIT), None);
        // Never while the pointer is still down.
        let mut splits = Splits::default();
        drag(&mut splits, 303.0, start);
        splits.settle(|_| Some(300), start);
        assert_eq!(splits.dragged_ratio(SPLIT), Some(300));
        splits.failed();
        assert_eq!(splits.dragged_ratio(SPLIT), Some(300));
        splits.update(Message::End, |_| None, start);
        splits.failed();
        assert_eq!(splits.dragged_ratio(SPLIT), None);
    }
}
