//! The pointer over the seek bar: where it is, whether the preview above it shows, and where
//! that preview sits. It follows the pointer with a glide, fades in and out, and stays inside
//! the window. No window here: the tests drive it with their own clock.

use std::time::Instant;

use crate::motion::{self, Fade, Glide};

/// The preview keeps this far from the edges of the window.
pub const MARGIN: f32 = 12.;

/// The left edge of a preview `width` wide centred on the pointer, kept inside a window
/// `window_width` wide. A window too narrow for it gets it at the left margin.
pub fn left_edge(pointer: f32, width: f32, window_width: f32) -> f32 {
    let widest_left = (window_width - width - MARGIN).max(MARGIN);
    (pointer - width / 2.).clamp(MARGIN, widest_left)
}

pub struct Scrub {
    hovering: bool,
    dragging: bool,
    /// How far along the bar the pointer is, from 0 to 1.
    fraction: f32,
    shown: Fade,
    left: Glide,
}

impl Scrub {
    pub fn new() -> Self {
        Self {
            hovering: false,
            dragging: false,
            fraction: 0.,
            shown: Fade::new(motion::quick()),
            left: Glide::new(0., motion::quick()),
        }
    }

    /// The pointer is over the bar, or drags along it, `fraction` of the way and with the
    /// preview's left edge at `left`.
    pub fn moved(&mut self, fraction: f32, left: f32, now: Instant) {
        // Coming back after the preview went, it starts at the pointer rather than sliding in
        // from where it was last.
        if !self.active() && self.shown.level(now) == 0. {
            self.left.jump(left);
        } else {
            self.left.set(left, now);
        }
        self.hovering = true;
        self.fraction = fraction.clamp(0., 1.);
    }

    /// The pointer left the bar. A drag goes on until the button is let go.
    pub fn left_bar(&mut self) {
        self.hovering = false;
    }

    pub fn press(&mut self) {
        self.dragging = true;
    }

    /// The button is let go. Where the film must settle when a drag ended, `fraction` of the
    /// way along the bar.
    pub fn release(&mut self) -> Option<f32> {
        std::mem::take(&mut self.dragging).then_some(self.fraction)
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// Hovered or dragged: the preview shows.
    pub fn active(&self) -> bool {
        self.hovering || self.dragging
    }

    /// The moment of the film under the pointer.
    pub fn seconds(&self, duration: f64) -> f64 {
        f64::from(self.fraction) * duration
    }

    /// How much of the preview shows, from 0 to 1. Called at each draw, which sends it where it
    /// is going.
    pub fn level(&mut self, now: Instant) -> f32 {
        self.shown.set(self.active(), now);
        self.shown.level(now)
    }

    pub fn left(&self, now: Instant) -> f32 {
        self.left.value(now)
    }

    /// Still on its way: the window needs another frame.
    pub fn moving(&self, now: Instant) -> bool {
        self.shown.moving(now) || self.left.moving(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(start: Instant, n: u64) -> Instant {
        start + Duration::from_millis(n)
    }

    #[test]
    fn the_preview_is_centred_on_the_pointer() {
        assert_eq!(left_edge(500., 240., 1440.), 380.);
    }

    #[test]
    fn the_preview_stays_inside_the_window_at_both_ends() {
        assert_eq!(left_edge(0., 240., 1440.), MARGIN);
        assert_eq!(left_edge(60., 240., 1440.), MARGIN);
        assert_eq!(left_edge(1440., 240., 1440.), 1440. - 240. - MARGIN);
        assert_eq!(left_edge(1400., 240., 1440.), 1440. - 240. - MARGIN);
    }

    #[test]
    fn a_window_narrower_than_the_preview_gets_it_at_the_left_margin() {
        assert_eq!(left_edge(100., 240., 200.), MARGIN);
    }

    #[test]
    fn nothing_shows_until_the_pointer_comes() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        assert!(!scrub.active());
        assert_eq!(scrub.level(t), 0.);
    }

    #[test]
    fn the_preview_fades_in_over_the_bar_and_out_when_the_pointer_leaves() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.level(t);
        scrub.moved(0.5, 300., t);
        assert!(scrub.active());
        let in_levels: Vec<f32> = [0, 50, 100, 200].map(|n| scrub.level(ms(t, n))).into();
        assert_eq!(in_levels[0], 0.);
        assert!(in_levels[1] > 0. && in_levels[1] < 1., "{in_levels:?}");
        assert!(in_levels[1] < in_levels[2] || in_levels[2] == 1.);
        assert_eq!(in_levels[3], 1.);
        assert!(scrub.moving(ms(t, 10)));

        scrub.left_bar();
        assert!(!scrub.active());
        let out = scrub.level(ms(t, 300));
        assert!(out > 0.9, "starts fading from full: {out}");
        assert!(scrub.level(ms(t, 360)) < out);
        assert_eq!(scrub.level(ms(t, 600)), 0.);
        assert!(!scrub.moving(ms(t, 600)));
    }

    #[test]
    fn the_preview_appears_at_the_pointer_then_glides_after_it() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.level(t);
        scrub.moved(0.2, 100., t);
        // At once where the pointer is: it does not slide in from the start.
        assert_eq!(scrub.left(t), 100.);
        let later = ms(t, 400);
        scrub.level(later);
        scrub.moved(0.6, 700., later);
        // Not yet there a little later, then there.
        let on_the_way = scrub.left(ms(later, 30));
        assert!(on_the_way > 100. && on_the_way < 700., "{on_the_way}");
        assert_eq!(scrub.left(ms(later, 400)), 700.);
    }

    #[test]
    fn coming_back_while_it_fades_out_glides_from_where_it_was() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.level(t);
        scrub.moved(0.2, 100., t);
        scrub.level(ms(t, 300));
        scrub.left_bar();
        scrub.level(ms(t, 310));
        scrub.moved(0.8, 900., ms(t, 330));
        // It was still fading out: no jump.
        assert!(scrub.left(ms(t, 335)) < 900.);
    }

    #[test]
    fn the_time_under_the_pointer_follows_it_along_the_film() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.moved(0.5, 0., t);
        assert_eq!(scrub.seconds(7860.), 3930.);
        scrub.moved(2., 0., t);
        assert_eq!(scrub.seconds(7860.), 7860.);
        scrub.moved(-1., 0., t);
        assert_eq!(scrub.seconds(7860.), 0.);
    }

    #[test]
    fn a_drag_keeps_the_preview_until_the_button_is_let_go_even_off_the_bar() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.moved(0.3, 200., t);
        scrub.press();
        scrub.left_bar();
        assert!(scrub.active() && scrub.dragging());
        scrub.release();
        assert!(!scrub.active() && !scrub.dragging());
    }

    #[test]
    fn letting_go_after_a_drag_says_where_to_settle() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.moved(0.3, 200., t);
        scrub.press();
        scrub.moved(0.7, 500., t);
        assert_eq!(scrub.release(), Some(0.7));
    }

    #[test]
    fn letting_go_without_a_press_on_the_bar_settles_nothing() {
        let (mut scrub, t) = (Scrub::new(), Instant::now());
        scrub.moved(0.3, 200., t);
        assert_eq!(scrub.release(), None);
    }
}
