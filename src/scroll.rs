//! Where a scroll goes on a page that scrolls down and holds rows that scroll sideways. GPUI gives
//! the same wheel event to every container under the pointer and lets each one take both axes,
//! so the page and a row moved together. Here one rule decides: the vertical part moves the page
//! only, the horizontal part (a touchpad sideways, Shift and the wheel) moves the row under the
//! pointer only. A touchpad sends both axes at once: the axis that dominates when a gesture
//! starts is kept until it ends, so a mostly vertical swipe never moves a row.

use std::time::{Duration, Instant};

/// Events closer than this belong to the same gesture when the platform does not say.
const GESTURE_GAP: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Vertical,
    Horizontal,
}

/// What one scroll event says, in pixels; `precise` for a touchpad, not for a wheel.
#[derive(Clone, Copy, Debug)]
pub struct Wheel {
    pub dx: f32,
    pub dy: f32,
    pub precise: bool,
    pub shift: bool,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
}

/// What a scroll event moves, and by how much.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    Page(f32),
    /// The row under the pointer, by its index.
    Row(usize, f32),
}

/// The touchpad gesture going on, and the axis it took.
#[derive(Default)]
pub struct Gesture {
    axis: Option<Axis>,
    last: Option<Instant>,
}

impl Gesture {
    /// What `wheel` moves, `row` being the row under the pointer, if any.
    pub fn route(&mut self, wheel: Wheel, row: Option<usize>, now: Instant) -> Option<Move> {
        let (dx, dy) = match wheel {
            // A wheel turned with Shift held scrolls sideways, if the platform did not do it.
            Wheel {
                shift: true,
                dx: 0.,
                dy,
                ..
            } => (dy, 0.),
            Wheel { dx, dy, .. } => (dx, dy),
        };
        if wheel.phase == Phase::Ended {
            *self = Self::default();
        }
        if dx == 0. && dy == 0. {
            return None;
        }
        let dominant = if dx.abs() > dy.abs() {
            Axis::Horizontal
        } else {
            Axis::Vertical
        };
        let axis = if wheel.precise {
            let new_gesture = wheel.phase == Phase::Started
                || self
                    .last
                    .is_none_or(|last| now.duration_since(last) > GESTURE_GAP);
            self.last = Some(now);
            *self.axis.insert(match self.axis {
                Some(axis) if !new_gesture => axis,
                _ => dominant,
            })
        } else {
            dominant
        };
        match axis {
            Axis::Vertical => Some(Move::Page(dy)),
            Axis::Horizontal => row.map(|row| Move::Row(row, dx)),
        }
    }
}

/// A scroll offset moved by `delta`, kept between `-max` and 0 (GPUI's offsets grow negative).
pub fn moved(offset: f32, delta: f32, max: f32) -> f32 {
    (offset + delta).clamp(-max.max(0.), 0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(dx: f32, dy: f32) -> Wheel {
        Wheel {
            dx,
            dy,
            precise: false,
            shift: false,
            phase: Phase::Moved,
        }
    }

    fn swipe(dx: f32, dy: f32, phase: Phase) -> Wheel {
        Wheel {
            precise: true,
            phase,
            ..wheel(dx, dy)
        }
    }

    #[test]
    fn vertical_scrolling_over_a_row_scrolls_the_page_only() {
        let mut g = Gesture::default();
        let now = Instant::now();
        assert_eq!(
            g.route(wheel(0., -60.), Some(1), now),
            Some(Move::Page(-60.))
        );
    }

    #[test]
    fn horizontal_scrolling_over_a_row_scrolls_the_row_only() {
        let mut g = Gesture::default();
        let now = Instant::now();
        assert_eq!(
            g.route(swipe(-30., 0., Phase::Started), Some(0), now),
            Some(Move::Row(0, -30.))
        );
    }

    #[test]
    fn the_page_never_moves_sideways() {
        let mut g = Gesture::default();
        let now = Instant::now();
        assert_eq!(g.route(wheel(-60., 0.), None, now), None);
    }

    #[test]
    fn shift_and_the_wheel_scroll_a_row_sideways() {
        let mut g = Gesture::default();
        let now = Instant::now();
        let shifted = Wheel {
            shift: true,
            ..wheel(0., -60.)
        };
        assert_eq!(g.route(shifted, Some(2), now), Some(Move::Row(2, -60.)));
        // A platform that already turned it sideways is not turned back.
        let turned = Wheel {
            shift: true,
            ..wheel(-60., 0.)
        };
        assert_eq!(g.route(turned, Some(2), now), Some(Move::Row(2, -60.)));
    }

    #[test]
    fn a_mostly_vertical_swipe_never_moves_a_row() {
        let mut g = Gesture::default();
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        assert_eq!(
            g.route(swipe(2., -12., Phase::Started), Some(0), ms(0)),
            Some(Move::Page(-12.))
        );
        // The fingers drift sideways for a moment: the gesture stays vertical.
        assert_eq!(
            g.route(swipe(9., -3., Phase::Moved), Some(0), ms(16)),
            Some(Move::Page(-3.))
        );
        assert_eq!(
            g.route(swipe(9., 0., Phase::Moved), Some(0), ms(32)),
            Some(Move::Page(0.))
        );
    }

    #[test]
    fn a_sideways_swipe_never_moves_the_page() {
        let mut g = Gesture::default();
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        g.route(swipe(-14., 3., Phase::Started), Some(1), ms(0));
        assert_eq!(
            g.route(swipe(-2., 6., Phase::Moved), Some(1), ms(16)),
            Some(Move::Row(1, -2.))
        );
        // Off the row, the rest of the sideways gesture moves nothing.
        assert_eq!(g.route(swipe(-2., 6., Phase::Moved), None, ms(32)), None);
    }

    #[test]
    fn a_new_gesture_takes_its_own_axis() {
        let mut g = Gesture::default();
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        g.route(swipe(0., -10., Phase::Started), Some(0), ms(0));
        g.route(swipe(0., 0., Phase::Ended), Some(0), ms(10));
        assert_eq!(
            g.route(swipe(-10., 1., Phase::Started), Some(0), ms(20)),
            Some(Move::Row(0, -10.))
        );
        // A platform that only says "moved": a pause long enough starts a new gesture.
        assert_eq!(
            g.route(swipe(1., -10., Phase::Moved), Some(0), ms(400)),
            Some(Move::Page(-10.))
        );
    }

    #[test]
    fn an_offset_stays_within_what_can_be_scrolled() {
        assert_eq!(moved(-10., -50., 40.), -40.);
        assert_eq!(moved(-10., 50., 40.), 0.);
        assert_eq!(moved(0., -50., 0.), 0.);
    }
}
