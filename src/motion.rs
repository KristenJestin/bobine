//! How things move: every change is eased, short, and over. What shows or hides fades from where it
//! is (a menu closed while still opening fades out from there); once a change is over nothing asks
//! for frames, so an idle window draws nothing.

use std::time::{Duration, Instant};

use gpui_kit::Rgba;

/// A hover, a press, a menu, the player's controls.
const QUICK: Duration = Duration::from_millis(160);
/// Going to a page and back.
const PAGE: Duration = Duration::from_millis(220);

/// How many times slower every move is: the headless harness slows them down to photograph
/// their frames. Always 1 in the application.
#[cfg(feature = "harness")]
pub static SLOWER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

fn slowed(duration: Duration) -> Duration {
    #[cfg(feature = "harness")]
    return duration * SLOWER.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "harness"))]
    duration
}

pub fn quick() -> Duration {
    slowed(QUICK)
}

pub fn page() -> Duration {
    slowed(PAGE)
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

    /// Already shown (or hidden), with nothing to ease: a button drawn for the first time in
    /// its state.
    pub const fn starting_at(duration: Duration, shown: bool) -> Self {
        Self {
            shown,
            ..Self::new(duration)
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

/// A number that eases to wherever it is sent (a highlight going to another row, a panel
/// growing to its content), from where it is, not from where it was sent last.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    from: f32,
    to: f32,
    duration: Duration,
    since: Option<Instant>,
}

impl Glide {
    pub const fn new(value: f32, duration: Duration) -> Self {
        Self {
            from: value,
            to: value,
            duration,
            since: None,
        }
    }

    /// Sends it to `target`; sending it where it already goes changes nothing.
    pub fn set(&mut self, target: f32, now: Instant) {
        if target != self.to {
            self.from = self.value(now);
            self.to = target;
            self.since = Some(now);
        }
    }

    /// Puts it at `value` at once, no move.
    pub fn jump(&mut self, value: f32) {
        *self = Self::new(value, self.duration);
    }

    pub fn value(&self, now: Instant) -> f32 {
        match self.since {
            Some(since) => {
                let t = now.duration_since(since).as_secs_f32() / self.duration.as_secs_f32();
                self.from + (self.to - self.from) * ease_out(t)
            }
            None => self.to,
        }
    }

    /// Still on its way: the window needs another frame.
    pub fn moving(&self, now: Instant) -> bool {
        self.since
            .is_some_and(|since| now.duration_since(since) < self.duration)
    }

    /// For a value that something else carries out (a scroll offset): where it is while it
    /// moves, and the target once more as it arrives, then nothing, so that the carrier is free
    /// again (the wheel can scroll it).
    pub fn advance(&mut self, now: Instant) -> Option<f32> {
        let since = self.since?;
        if now.duration_since(since) >= self.duration {
            self.since = None;
            self.from = self.to;
            return Some(self.to);
        }
        Some(self.value(now))
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

    #[test]
    fn a_glide_eases_from_where_it_was_to_where_it_goes() {
        let t = Instant::now();
        let mut glide = Glide::new(10., QUICK);
        assert_eq!(glide.value(t), 10.);
        glide.set(110., t);
        let values: Vec<f32> = [0, 40, 80, 160].map(|n| glide.value(ms(t, n))).into();
        assert_eq!(values[0], 10.);
        assert!(values.windows(2).all(|w| w[0] < w[1]), "{values:?}");
        assert!(values[2] > 90., "{values:?}");
        assert_eq!(values[3], 110.);
        assert!(glide.moving(ms(t, 159)) && !glide.moving(ms(t, 160)));
    }

    #[test]
    fn a_glide_sent_elsewhere_midway_leaves_from_where_it_is() {
        let t = Instant::now();
        let mut glide = Glide::new(0., QUICK);
        glide.set(100., t);
        let midway = glide.value(ms(t, 40));
        glide.set(0., ms(t, 40));
        assert_eq!(glide.value(ms(t, 40)), midway);
        assert!(glide.value(ms(t, 80)) < midway);
    }

    #[test]
    fn a_glide_sent_where_it_goes_does_not_move() {
        let t = Instant::now();
        let mut glide = Glide::new(5., QUICK);
        glide.set(5., t);
        assert!(!glide.moving(t));
        glide.set(9., t);
        let on_its_way = glide.value(ms(t, 100));
        // Asking again for the same place does not start it over.
        glide.set(9., ms(t, 100));
        assert_eq!(glide.value(ms(t, 100)), on_its_way);
        assert!(on_its_way > 5.);
    }

    #[test]
    fn a_glide_carried_by_something_else_lands_once_then_lets_go() {
        let t = Instant::now();
        let mut glide = Glide::new(0., QUICK);
        assert_eq!(glide.advance(t), None);
        glide.set(50., t);
        assert!(glide.advance(ms(t, 40)).is_some_and(|v| 0. < v && v < 50.));
        assert_eq!(glide.advance(ms(t, 200)), Some(50.));
        assert_eq!(glide.advance(ms(t, 300)), None);
    }
}
