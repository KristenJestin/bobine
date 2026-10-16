//! "Black out other screens": while a film plays fullscreen, every other monitor is covered by
//! a black surface (not switched off), which goes when the film leaves fullscreen or stops.
//! What is covered is decided here, from the monitors and the window's; drawing the black is the
//! adapter's (`layer`), behind `Covers`, so the decisions are tested with a fake.

pub mod layer;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// The other monitors are wanted black: the option is on and a film plays fullscreen.
pub fn wanted(option: bool, playing: bool, fullscreen: bool) -> bool {
    option && playing && fullscreen
}

/// The monitors to cover: all but the one the window is fullscreen on. When that one is not
/// known, none: covering the film itself would be worse than covering nothing.
pub fn covered(outputs: &[String], window_on: Option<&str>, wanted: bool) -> BTreeSet<String> {
    match window_on {
        Some(on) if wanted && outputs.iter().any(|output| output == on) => outputs
            .iter()
            .filter(|output| *output != on)
            .cloned()
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// What puts a black surface on a monitor and takes it away again, by the monitor's name.
pub trait Covers {
    fn cover(&mut self, output: &str);
    fn uncover(&mut self, output: &str);
}

/// The monitor GPUI says the window is on, from GPUI's id for it: a v5 UUID of the output's
/// name.
pub fn monitor_with_id(outputs: &[String], id: Uuid) -> Option<String> {
    outputs
        .iter()
        .find(|name| Uuid::new_v5(&Uuid::NAMESPACE_DNS, name.as_bytes()) == id)
        .cloned()
}

/// Seen twice in a row: a window that just went fullscreen can be reported on its old monitor
/// by GPUI for a tick, and covering that one would black out the film itself.
pub struct Steady<T> {
    last: Option<T>,
}

impl<T> Default for Steady<T> {
    fn default() -> Self {
        Steady { last: None }
    }
}

impl<T: PartialEq> Steady<T> {
    /// What is seen now. Whether it is what was seen on the previous call too.
    pub fn settles(&mut self, seen: T) -> bool {
        let same = self.last.as_ref() == Some(&seen);
        self.last = Some(seen);
        same
    }
}

/// Where the wish to cover goes: the adapter's thread, or a fake.
pub trait Wish {
    fn want(&mut self, wanted: bool, window_on: Option<Uuid>);
}

/// Stops the film, the covers having been withdrawn first: mpv shutting down can take a
/// moment, and the other monitors must not stay black for it.
pub fn stop_with<W: Wish, R>(wish: Option<&mut W>, stop: impl FnOnce() -> R) -> R {
    if let Some(wish) = wish {
        wish.want(false, None);
    }
    stop()
}

/// How long the UI may stay silent while covering before the covers go: a UI thread that froze
/// must not leave the monitors black.
pub const SILENCE: Duration = Duration::from_secs(2);

/// The blackout thread's watch on the UI thread, which says it is alive on every player tick.
/// The time is passed in, so it is tested without waiting.
pub struct Heartbeat {
    last: Instant,
}

impl Heartbeat {
    pub fn new(now: Instant) -> Heartbeat {
        Heartbeat { last: now }
    }

    /// A message came from the UI thread.
    pub fn beat(&mut self, now: Instant) {
        self.last = now;
    }

    /// Nothing came for too long.
    pub fn lapsed(&self, now: Instant) -> bool {
        self.until_lapse(now).is_zero()
    }

    /// How long until it lapses, to wake up for it.
    pub fn until_lapse(&self, now: Instant) -> Duration {
        SILENCE.saturating_sub(now.saturating_duration_since(self.last))
    }
}

/// Follows the monitors and the wish, and covers or uncovers only what changed.
#[derive(Default)]
pub struct Blackout {
    covered: BTreeSet<String>,
}

impl Blackout {
    /// `outputs`: the monitors now plugged in, by name; `wanted`: the other monitors are wanted
    /// black; `window_on`: the monitor the window is on.
    pub fn follow(
        &mut self,
        outputs: &[String],
        wanted: bool,
        window_on: Option<&str>,
        covers: &mut dyn Covers,
    ) {
        let next = covered(outputs, window_on, wanted);
        for gone in self.covered.difference(&next) {
            covers.uncover(gone);
        }
        for new in next.difference(&self.covered) {
            covers.cover(new);
        }
        self.covered = next;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// Records what was covered and uncovered, in order.
    #[derive(Default)]
    struct FakeCovers {
        calls: Vec<String>,
    }

    impl Covers for FakeCovers {
        fn cover(&mut self, output: &str) {
            self.calls.push(format!("cover {output}"));
        }
        fn uncover(&mut self, output: &str) {
            self.calls.push(format!("uncover {output}"));
        }
    }

    const THREE: [&str; 3] = ["DP-1", "DP-2", "HDMI-A-1"];

    #[test]
    fn every_monitor_but_the_fullscreen_one_is_covered() {
        let outputs = names(&THREE);
        assert_eq!(
            covered(&outputs, Some("DP-2"), true),
            set(&["DP-1", "HDMI-A-1"])
        );
    }

    #[test]
    fn the_option_off_covers_nothing() {
        assert!(!wanted(false, true, true));
        let outputs = names(&THREE);
        assert!(covered(&outputs, Some("DP-1"), wanted(false, true, true)).is_empty());
    }

    #[test]
    fn nothing_is_covered_while_the_film_is_not_fullscreen_or_not_playing() {
        assert!(wanted(true, true, true));
        assert!(!wanted(true, true, false));
        assert!(!wanted(true, false, true));
    }

    #[test]
    fn an_unknown_window_monitor_covers_nothing() {
        let outputs = names(&THREE);
        assert!(covered(&outputs, None, true).is_empty());
        assert!(covered(&outputs, Some("HDMI-A-9"), true).is_empty());
    }

    #[test]
    fn a_single_monitor_gets_no_cover() {
        assert!(covered(&names(&["DP-1"]), Some("DP-1"), true).is_empty());
    }

    #[test]
    fn the_window_monitor_is_found_from_gpui_s_id_for_it() {
        let outputs = names(&THREE);
        let id = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"DP-2");
        assert_eq!(monitor_with_id(&outputs, id).as_deref(), Some("DP-2"));
        let unknown = Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"DP-9");
        assert_eq!(monitor_with_id(&outputs, unknown), None);
    }

    #[test]
    fn the_covers_come_with_fullscreen_and_go_when_it_ends() {
        let (mut covers, mut blackout) = (FakeCovers::default(), Blackout::default());
        let outputs = names(&THREE);
        blackout.follow(&outputs, false, Some("DP-2"), &mut covers);
        assert!(covers.calls.is_empty());
        blackout.follow(&outputs, true, Some("DP-2"), &mut covers);
        assert_eq!(covers.calls, ["cover DP-1", "cover HDMI-A-1"]);
        covers.calls.clear();
        blackout.follow(&outputs, false, Some("DP-2"), &mut covers);
        assert_eq!(covers.calls, ["uncover DP-1", "uncover HDMI-A-1"]);
    }

    #[test]
    fn the_same_wish_again_changes_nothing() {
        let (mut covers, mut blackout) = (FakeCovers::default(), Blackout::default());
        let outputs = names(&THREE);
        blackout.follow(&outputs, true, Some("DP-2"), &mut covers);
        covers.calls.clear();
        blackout.follow(&outputs, true, Some("DP-2"), &mut covers);
        assert!(covers.calls.is_empty());
    }

    #[test]
    fn a_monitor_plugged_in_during_the_film_is_covered() {
        let (mut covers, mut blackout) = (FakeCovers::default(), Blackout::default());
        blackout.follow(&names(&THREE), true, Some("DP-2"), &mut covers);
        covers.calls.clear();
        let four = names(&["DP-1", "DP-2", "HDMI-A-1", "DP-3"]);
        blackout.follow(&four, true, Some("DP-2"), &mut covers);
        assert_eq!(covers.calls, ["cover DP-3"]);
    }

    #[test]
    fn a_monitor_unplugged_during_the_film_is_uncovered() {
        let (mut covers, mut blackout) = (FakeCovers::default(), Blackout::default());
        blackout.follow(&names(&THREE), true, Some("DP-2"), &mut covers);
        covers.calls.clear();
        blackout.follow(&names(&["DP-1", "DP-2"]), true, Some("DP-2"), &mut covers);
        assert_eq!(covers.calls, ["uncover HDMI-A-1"]);
    }

    #[test]
    fn a_monitor_plugged_in_while_windowed_stays_uncovered() {
        let (mut covers, mut blackout) = (FakeCovers::default(), Blackout::default());
        blackout.follow(&names(&THREE), false, Some("DP-2"), &mut covers);
        let four = names(&["DP-1", "DP-2", "HDMI-A-1", "DP-3"]);
        blackout.follow(&four, false, Some("DP-2"), &mut covers);
        assert!(covers.calls.is_empty());
    }

    #[test]
    fn the_window_moving_to_another_monitor_uncovers_that_one_and_covers_the_one_left() {
        let (mut covers, mut blackout) = (FakeCovers::default(), Blackout::default());
        let outputs = names(&THREE);
        blackout.follow(&outputs, true, Some("DP-2"), &mut covers);
        covers.calls.clear();
        blackout.follow(&outputs, true, Some("DP-1"), &mut covers);
        assert_eq!(covers.calls, ["uncover DP-1", "cover DP-2"]);
    }

    /// Writes into a log shared with the stop, to tell which came first.
    struct FakeWish<'a> {
        log: &'a RefCell<Vec<String>>,
    }

    impl Wish for FakeWish<'_> {
        fn want(&mut self, wanted: bool, window_on: Option<Uuid>) {
            self.log
                .borrow_mut()
                .push(format!("want {wanted} {window_on:?}"));
        }
    }

    #[test]
    fn the_covers_are_withdrawn_before_the_film_stops() {
        let log = RefCell::new(Vec::new());
        let mut wish = FakeWish { log: &log };
        stop_with(Some(&mut wish), || log.borrow_mut().push("stop".into()));
        assert_eq!(*log.borrow(), ["want false None", "stop"]);
    }

    #[test]
    fn a_film_without_screens_just_stops() {
        let stopped = stop_with(None::<&mut FakeWish>, || 7);
        assert_eq!(stopped, 7);
    }

    #[test]
    fn the_ui_silent_for_two_seconds_lapses_the_heartbeat() {
        let start = Instant::now();
        let mut heartbeat = Heartbeat::new(start);
        assert!(!heartbeat.lapsed(start + Duration::from_millis(1999)));
        assert!(heartbeat.lapsed(start + SILENCE));
        heartbeat.beat(start + Duration::from_secs(1));
        assert!(!heartbeat.lapsed(start + Duration::from_millis(2999)));
        assert!(heartbeat.lapsed(start + Duration::from_secs(3)));
    }

    #[test]
    fn the_heartbeat_says_how_long_until_it_lapses() {
        let start = Instant::now();
        let heartbeat = Heartbeat::new(start);
        assert_eq!(heartbeat.until_lapse(start), SILENCE);
        let later = start + Duration::from_millis(500);
        assert_eq!(heartbeat.until_lapse(later), Duration::from_millis(1500));
        assert_eq!(
            heartbeat.until_lapse(start + Duration::from_secs(5)),
            Duration::ZERO
        );
    }

    #[test]
    fn what_is_seen_twice_in_a_row_is_steady() {
        let mut steady = Steady::default();
        assert!(!steady.settles((true, "DP-1")));
        assert!(steady.settles((true, "DP-1")));
        assert!(steady.settles((true, "DP-1")));
    }

    #[test]
    fn a_change_is_not_steady_until_seen_again() {
        let mut steady = Steady::default();
        steady.settles((false, "DP-1"));
        // Fullscreen, but GPUI still says the old monitor.
        assert!(!steady.settles((true, "DP-1")));
        assert!(!steady.settles((true, "DP-2")));
        assert!(steady.settles((true, "DP-2")));
    }
}
