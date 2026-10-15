//! How things move: every change is eased, short, and over. What shows or hides fades from where it
//! is (a menu closed while still opening fades out from there); once a change is over nothing asks
//! for frames, so an idle window draws nothing.

use std::time::{Duration, Instant};

use gpui_kit::Rgba;

/// A hover, a press, a menu, the player's controls.
const QUICK: Duration = Duration::from_millis(160);
/// Going to a page and back.
const PAGE: Duration = Duration::from_millis(220);

pub fn quick() -> Duration {
    QUICK
}

pub fn page() -> Duration {
    PAGE
}

/// Fast at first, gentle at the end.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    1. - (1. - t).powi(3)
}

/// Between `from` and `to`, `t` of the way.
pub fn mix(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let at = |a: f32, b: f32| a + (b - a) * t;
    Rgba {
        r: at(from.r, to.r),
        g: at(from.g, to.g),
        b: at(from.b, to.b),
        a: at(from.a, to.a),
    }
}

/// Something that shows and hides: how much of it shows, from 0 to 1, at any time.
#[derive(Clone, Copy, Debug)]
pub struct Fade {
    shown: bool,
    duration: Duration,
    /// Where the last change started from, and when.
    from: f32,
    since: Option<Instant>,
}

impl Fade {
    pub const fn new(duration: Duration) -> Self {
        Self {
            shown: false,
            duration,
            from: 0.,
            since: None,
        }
    }

    pub fn set(&mut self, shown: bool, now: Instant) {
        if shown != self.shown {
            self.from = self.level(now);
            self.since = Some(now);
            self.shown = shown;
        }
    }

    pub fn level(&self, now: Instant) -> f32 {
        let target = if self.shown { 1. } else { 0. };
        match self.since {
            Some(since) => {
                let t = now.duration_since(since).as_secs_f32() / self.duration.as_secs_f32();
                self.from + (target - self.from) * ease_out(t)
            }
            None => target,
        }
    }

    /// Still on its way: the window needs another frame.
    pub fn moving(&self, now: Instant) -> bool {
        self.since
            .is_some_and(|since| now.duration_since(since) < self.duration)
    }

    /// Something to draw: shown, or still fading out.
    pub fn visible(&self, now: Instant) -> bool {
        self.shown || self.moving(now)
    }
}

/// One of several things that show in turn (a chip's menu), kept drawn while it fades out.
pub struct Presence<T> {
    fade: Fade,
    drawn: Option<T>,
}

impl<T: Clone> Presence<T> {
    pub const fn new(duration: Duration) -> Self {
        Self {
            fade: Fade::new(duration),
            drawn: None,
        }
    }

    /// Follows what should show now; gives what to draw and how much of it shows.
    pub fn follow(&mut self, wanted: Option<T>, now: Instant) -> Option<(T, f32)> {
        match wanted {
            Some(wanted) => {
                self.drawn = Some(wanted);
                self.fade.set(true, now);
            }
            None => self.fade.set(false, now),
        }
        if !self.fade.visible(now) {
            self.drawn = None;
        }
        Some((self.drawn.clone()?, self.fade.level(now)))
    }

    pub fn moving(&self, now: Instant) -> bool {
        self.fade.moving(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(start: Instant, n: u64) -> Instant {
        start + Duration::from_millis(n)
    }

    #[test]
    fn a_menu_opening_eases_in_then_stops_asking_for_frames() {
        let t = Instant::now();
        let mut fade = Fade::new(QUICK);
        assert_eq!(fade.level(t), 0.);
        fade.set(true, t);
        let levels: Vec<f32> = [0, 40, 80, 120, 160].map(|n| fade.level(ms(t, n))).into();
        assert_eq!(levels[0], 0.);
        assert!(levels.windows(2).all(|w| w[0] < w[1]), "{levels:?}");
        // Ease-out: most of the way in the first half.
        assert!(levels[2] > 0.8, "{levels:?}");
        assert_eq!(levels[4], 1.);
        assert!(fade.moving(ms(t, 159)));
        assert!(!fade.moving(ms(t, 160)));
    }

    #[test]
    fn a_closed_menu_fades_out_then_is_gone() {
        let t = Instant::now();
        let mut menu = Presence::new(QUICK);
        menu.follow(Some('a'), t);
        let (drawn, level) = menu.follow(None, ms(t, 200)).unwrap();
        assert_eq!((drawn, level), ('a', 1.));
        let (_, level) = menu.follow(None, ms(t, 280)).unwrap();
        assert!(0. < level && level < 1., "{level}");
        assert!(menu.moving(ms(t, 280)));
        assert_eq!(menu.follow(None, ms(t, 360)), None);
        assert!(!menu.moving(ms(t, 360)));
    }

    #[test]
    fn a_menu_closed_while_opening_fades_out_from_where_it_was() {
        let t = Instant::now();
        let mut fade = Fade::new(QUICK);
        fade.set(true, t);
        let halfway = fade.level(ms(t, 40));
        fade.set(false, ms(t, 40));
        assert_eq!(fade.level(ms(t, 40)), halfway);
        assert!(fade.level(ms(t, 60)) < halfway);
        assert_eq!(fade.level(ms(t, 200)), 0.);
    }

    #[test]
    fn nothing_moves_when_nothing_changed() {
        let t = Instant::now();
        let mut fade = Fade::new(QUICK);
        assert!(!fade.moving(t));
        fade.set(true, t);
        fade.set(true, ms(t, 500));
        assert!(!fade.moving(ms(t, 500)));
    }

    #[test]
    fn a_colour_is_mixed_on_the_way() {
        let black = Rgba {
            r: 0.,
            g: 0.,
            b: 0.,
            a: 1.,
        };
        let white = Rgba {
            r: 1.,
            g: 1.,
            b: 1.,
            a: 0.,
        };
        let half = mix(black, white, 0.5);
        assert_eq!((half.r, half.a), (0.5, 0.5));
    }
}
