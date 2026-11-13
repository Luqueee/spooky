//! The dropdown's slide through the top edge of the window, drawn by the application.
//!
//! KWin cannot clip a window to its own display, so moving the window itself shows it on
//! a neighbouring display above. The window therefore stays in place and its content
//! moves inside it, which the window edge clips. This is the approach Yakuake falls back
//! to where it cannot ask the compositor for a slide.

use std::time::{Duration, Instant};

/// How long the content takes to slide in or out.
pub(crate) const DURATION: Duration = Duration::from_millis(200);

/// How long the first open waits for KWin to know the window (see `platform::kde`), so
/// the content does not slide before the window has been placed.
pub(crate) const FIRST_DELAY: Duration = Duration::from_millis(300);

/// A slide in progress, or finished and holding the content out of sight.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Slide {
    started: Instant,
    /// Where the content starts and ends, as [`Slide::hidden_share`].
    from: f32,
    to: f32,
    progress: f32,
    /// The end of a closing slide has been reported.
    reported: bool,
}

impl Slide {
    /// The content comes down from `from` (a [`Slide::hidden_share`]; 1 when it was out
    /// of sight) into place, starting at `at`, which may be in the future; until then it
    /// stays where it was.
    pub(crate) fn open(at: Instant, from: f32) -> Self {
        Self::new(at, from, 0.0)
    }

    /// The content goes up out of the window from `from` (0 when it was in place).
    pub(crate) fn close(at: Instant, from: f32) -> Self {
        Self::new(at, from, 1.0)
    }

    /// Starting from where a slide cut short had got to makes a quick second press turn
    /// the content around instead of making it jump.
    fn new(started: Instant, from: f32, to: f32) -> Self {
        Self {
            started,
            from,
            to,
            progress: 0.0,
            reported: false,
        }
    }

    /// Moves to `now`.
    pub(crate) fn advance(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.started);
        self.progress = (elapsed.as_secs_f32() / DURATION.as_secs_f32()).min(1.0);
    }

    /// Whether frames are still needed.
    pub(crate) fn running(&self) -> bool {
        self.progress < 1.0
    }

    /// Whether the slide has ended with the content in place, so it can be dropped.
    pub(crate) fn finished_open(&self) -> bool {
        self.to == 0.0 && !self.running()
    }

    /// True once, when a closing slide has ended with the content out of sight, so the
    /// window can be taken off screen.
    pub(crate) fn closed(&mut self) -> bool {
        let done = self.to == 1.0 && !self.running() && !self.reported;
        self.reported |= done;
        done
    }

    /// The window's opacity, which follows the slide in a straight line: 0 while the
    /// content is out of sight and 1 in place.
    pub(crate) fn opacity(&self) -> f32 {
        let (from, to) = (1.0 - self.from, 1.0 - self.to);
        from + (to - from) * self.progress
    }

    /// How far above its place the content is, as a share of its height: 1 is entirely
    /// out of the window, 0 is in place. It eases out, so it settles gently.
    pub(crate) fn hidden_share(&self) -> f32 {
        let eased = 1.0 - (1.0 - self.progress).powi(3);
        self.from + (self.to - self.from) * eased
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_runs_from_out_of_the_window_into_place_and_closing_the_reverse() {
        let start = Instant::now();
        let mut open = Slide::open(start, 1.0);
        assert_eq!(open.hidden_share(), 1.0);
        open.advance(start + DURATION / 2);
        let halfway = open.hidden_share();
        assert!(
            halfway > 0.0 && halfway < 0.5,
            "eased out: more than half done at half time, got {halfway}"
        );
        open.advance(start + DURATION * 2);
        assert_eq!(open.hidden_share(), 0.0);
        assert!(open.finished_open() && !open.running());

        let mut close = Slide::close(start, 0.0);
        assert_eq!(close.hidden_share(), 0.0);
        close.advance(start + DURATION);
        assert_eq!(close.hidden_share(), 1.0);
        assert!(
            !close.finished_open(),
            "a closed window keeps its content out"
        );
    }

    #[test]
    fn the_fade_follows_the_slide_in_a_straight_line_and_the_close_is_reported_once() {
        let start = Instant::now();
        let mut open = Slide::open(start, 1.0);
        assert_eq!(open.opacity(), 0.0);
        open.advance(start + DURATION / 2);
        assert!((open.opacity() - 0.5).abs() < 1e-6);

        let mut close = Slide::close(start, 0.0);
        close.advance(start + DURATION / 2);
        assert!(!close.closed(), "still moving");
        close.advance(start + DURATION);
        assert_eq!(close.opacity(), 0.0);
        assert!(close.closed());
        assert!(!close.closed(), "reported once");
    }

    #[test]
    fn a_slide_cut_short_is_turned_around_from_where_it_was() {
        let start = Instant::now();
        let mut open = Slide::open(start, 1.0);
        open.advance(start + DURATION / 4);
        let reached = open.hidden_share();
        let mut back = Slide::close(start + DURATION / 4, reached);
        assert_eq!(back.hidden_share(), reached, "no jump when it turns around");
        back.advance(start + DURATION * 2);
        assert_eq!(back.hidden_share(), 1.0);
    }
}
