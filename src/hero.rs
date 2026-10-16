//! The header of the Home, apart from the window: which of its titles shows, when the next one
//! comes, the cross-fade between two, and what it says of a title. Time is always given, never
//! read, so that each rule is tested without waiting.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bobine::provider::{Feature, FeatureKind, ImageRef, ItemId};

use crate::film_page;
use crate::images;
use crate::motion;

/// How long a title stays before the next one comes.
pub const ROTATE_EVERY: Duration = Duration::from_secs(8);

/// The titles of the header in turn: which one shows, how long it has, and the one fading out.
#[derive(Debug)]
pub struct Carousel {
    len: usize,
    current: usize,
    /// How long the current title has been shown, up to the last time it was running.
    shown: Duration,
    /// Since when it counts again; `None` while paused.
    running_since: Option<Instant>,
    /// The title that was shown before, and when the cross-fade from it began.
    leaving: Option<(usize, Instant)>,
}

/// What to draw of a change of title at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blend {
    /// The title fading out under the current one, if a change is under way.
    pub leaving: Option<usize>,
    /// How much of the current title shows over it, from 0 to 1.
    pub level: f32,
}

impl Carousel {
    pub fn new(len: usize, now: Instant) -> Self {
        Self {
            len,
            current: 0,
            shown: Duration::ZERO,
            running_since: Some(now),
            leaving: None,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn current(&self) -> usize {
        self.current
    }

    fn elapsed(&self, now: Instant) -> Duration {
        self.shown
            + self
                .running_since
                .map_or(Duration::ZERO, |since| now.saturating_duration_since(since))
    }

    /// How much of its time the current title has had, from 0 to 1: the bar under the header.
    pub fn progress(&self, now: Instant) -> f32 {
        (self.elapsed(now).as_secs_f32() / ROTATE_EVERY.as_secs_f32()).min(1.)
    }

    /// Moves on to the next title when the current one has had its time: whether it did.
    pub fn tick(&mut self, now: Instant) -> bool {
        if self.len < 2 || self.paused() || self.elapsed(now) < ROTATE_EVERY {
            return false;
        }
        self.show((self.current + 1) % self.len, now);
        true
    }

    /// The pointer is over the header, or the Home is not on screen: the time stops counting,
    /// and picks up where it was when it can again.
    pub fn pause(&mut self, paused: bool, now: Instant) {
        match (paused, self.running_since) {
            (true, Some(_)) => {
                self.shown = self.elapsed(now);
                self.running_since = None;
            }
            (false, None) => self.running_since = Some(now),
            _ => {}
        }
    }

    pub fn paused(&self) -> bool {
        self.running_since.is_none()
    }

    /// A click on the mark of title `to`: it shows now, with all its time.
    pub fn jump(&mut self, to: usize, now: Instant) {
        if to < self.len && to != self.current {
            self.show(to, now);
        }
    }

    /// `to` shows from now, over the title that was, with all its time.
    fn show(&mut self, to: usize, now: Instant) {
        self.leaving = Some((self.current, now));
        self.current = to;
        self.shown = Duration::ZERO;
        self.running_since = self.running_since.map(|_| now);
    }

    /// The list of titles was asked again: `keep` is where the title shown now is in the new
    /// list, if it still is. It keeps its place and its time; else the header starts again.
    pub fn retarget(&mut self, len: usize, keep: Option<usize>, now: Instant) {
        self.len = len;
        self.leaving = None;
        match keep.filter(|at| *at < len) {
            Some(at) => self.current = at,
            None => {
                self.current = 0;
                self.shown = Duration::ZERO;
                self.running_since = self.running_since.map(|_| now);
            }
        }
    }

    pub fn blend(&self, now: Instant) -> Blend {
        match self.leaving.filter(|_| self.crossfading(now)) {
            Some((leaving, since)) => Blend {
                leaving: Some(leaving),
                level: motion::ease_out(
                    now.saturating_duration_since(since).as_secs_f32()
                        / motion::crossfade().as_secs_f32(),
                ),
            },
            None => Blend {
                leaving: None,
                level: 1.,
            },
        }
    }

    /// A change of title is under way: the window needs another frame.
    pub fn crossfading(&self, now: Instant) -> bool {
        self.leaving
            .is_some_and(|(_, since)| now.saturating_duration_since(since) < motion::crossfade())
    }
}

/// How much of the title leaving and of the one arriving shows, at `level` of the cross-fade:
/// the backdrops blend all along, the texts one after the other so that two never overlap.
pub fn text_levels(level: f32) -> (f32, f32) {
    ((1. - 2. * level).max(0.), (2. * level - 1.).max(0.))
}

/// A bar under the header, one for each title: its width and its thickness. Short and thin, so
/// that the bars stay quiet beside the title.
pub const MARK: (f32, f32) = (28., 2.);

/// The most a logo takes of the header.
pub const LOGO: (f32, f32) = (440., 130.);

/// The most a logo takes of a film's or a series' page, whose title is smaller than the
/// header's.
pub const PAGE_LOGO: (f32, f32) = (360., 110.);

/// The size a logo of `size` is drawn at, to fit in the room it has; none for a logo with
/// nothing to draw (a size of zero), so that the title in text shows instead.
pub fn logo_fit(size: (f32, f32)) -> Option<(f32, f32)> {
    logo_fit_in(size, LOGO)
}

/// The size a logo of `size` is drawn at, to fit in `room` (its widest and its tallest), never
/// larger than it is: a small picture blown up turns blurry.
pub fn logo_fit_in((width, height): (f32, f32), room: (f32, f32)) -> Option<(f32, f32)> {
    if width <= 0. || height <= 0. {
        return None;
    }
    let scale = (room.0 / width).min(room.1 / height).min(1.);
    Some((width * scale, height * scale))
}

/// Whether the header's ticker runs: only while the Home shows and there are two titles or more
/// to take turns, so that nothing wakes the window the rest of the time.
pub fn ticks(on_home: bool, titles: usize) -> bool {
    on_home && titles > 1
}

/// Whether the header's time is stopped. The pointer holds it only if it is over the header
/// and still inside the window: the window hears nothing when the pointer leaves it, so
/// "over the header" can stay true for a pointer that is gone.
pub fn holds(on_home: bool, over_header: bool, pointer_in_window: bool) -> bool {
    !on_home || (over_header && pointer_in_window)
}

/// Where the title shown is in a list asked again, if it still is.
pub fn kept(shown: Option<&ItemId>, features: &[Feature]) -> Option<usize> {
    features
        .iter()
        .position(|feature| Some(&feature.id) == shown)
}

/// What the header writes as the title: its logo, else its name.
#[derive(Debug, Clone, PartialEq)]
pub enum Headline {
    Logo(ImageRef),
    Text(String),
}

pub fn headline(feature: &Feature) -> Headline {
    match &feature.logo {
        Some(logo) => Headline::Logo(logo.clone()),
        None => Headline::Text(feature.title.clone()),
    }
}

/// Every picture the header draws now, with its width: the backdrops of the title shown, of the
/// one fading out and of the next one (asked while this one shows, so that the change does not
/// wait for it), and the logos of the first two. The header only reads the pictures; the shell
/// asks for these outside the draw.
pub fn pictures(
    features: &[Feature],
    current: usize,
    leaving: Option<usize>,
) -> Vec<(ImageRef, u32)> {
    let Some(shown) = features.get(current) else {
        return Vec::new();
    };
    let next = (features.len() > 1).then(|| &features[(current + 1) % features.len()]);
    let leaving = leaving.and_then(|at| features.get(at));
    let backdrops = [Some(shown), next, leaving]
        .into_iter()
        .flatten()
        .filter_map(|feature| feature.backdrop.clone())
        .map(|image| (image, images::BACKDROP));
    let logos = [Some(shown), leaving]
        .into_iter()
        .flatten()
        .filter_map(|feature| feature.logo.clone())
        .map(|image| (image, images::LOGO));
    backdrops.chain(logos).collect()
}

/// When each logo was first drawn: it fades in then, once, not again each time its title comes
/// back. Only the titles of the header are remembered.
#[derive(Debug, Default)]
pub struct Logos(HashMap<ItemId, Instant>);

impl Logos {
    /// How much of the way in the logo of `title` is, from 0 to 1.
    pub fn level(&mut self, title: &ItemId, now: Instant) -> f32 {
        let since = *self.0.entry(title.clone()).or_insert(now);
        motion::ease_out(
            now.saturating_duration_since(since).as_secs_f32() / motion::page().as_secs_f32(),
        )
    }

    /// Forgets the logos of titles that are no longer in the header.
    pub fn keep_only(&mut self, features: &[Feature]) {
        self.0.retain(|title, _| {
            features.iter().any(|feature| {
                feature
                    .logo
                    .as_ref()
                    .is_some_and(|logo| &logo.item == title)
            })
        });
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

/// `Movie` or `Series`, over the title.
pub fn kind_label(feature: &Feature) -> &'static str {
    match feature.kind {
        FeatureKind::Film => "Movie",
        FeatureKind::Series => "Series",
    }
}

/// `1998 | ★ 7.4 | 1h 50m | PG-13`: year, rating, runtime (films), age rating.
pub fn meta(feature: &Feature) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    parts.extend(feature.year.map(|year| year.to_string()));
    parts.extend(
        feature
            .community_rating
            .map(|rating| format!("★ {rating:.1}")),
    );
    parts.extend(feature.runtime.map(film_page::short_duration));
    parts.extend(feature.official_rating.clone());
    parts
}

/// `Resume from 20m` for a film started, else `Play`.
pub fn play_label(feature: &Feature) -> String {
    match feature.resume {
        Some(resume) => format!("Resume from {}", film_page::short_duration(resume)),
        None => "Play".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobine::provider::{ImageKind, ItemId};

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn feature(kind: FeatureKind) -> Feature {
        Feature {
            kind,
            id: ItemId("title-1".into()),
            title: "Les Marées".into(),
            year: Some(2025),
            community_rating: Some(7.8),
            official_rating: Some("12+".into()),
            runtime: (kind == FeatureKind::Film).then(|| secs(124 * 60)),
            overview: None,
            backdrop: None,
            logo: None,
            resume: None,
        }
    }

    fn with_pictures(id: &str) -> Feature {
        let picture = |kind| ImageRef {
            item: ItemId(id.into()),
            kind,
            tag: "t".into(),
        };
        Feature {
            id: ItemId(id.into()),
            backdrop: Some(picture(ImageKind::Backdrop)),
            logo: Some(picture(ImageKind::Logo)),
            ..feature(FeatureKind::Film)
        }
    }

    #[test]
    fn the_header_has_its_pictures_asked_outside_the_draw_for_the_title_shown_the_next_and_the_leaving()
     {
        let features = [with_pictures("a"), with_pictures("b"), with_pictures("c")];
        let wanted: Vec<(String, ImageKind, u32)> = pictures(&features, 1, Some(0))
            .into_iter()
            .map(|(image, width)| (image.item.0, image.kind, width))
            .collect();
        assert_eq!(
            wanted,
            [
                ("b".to_string(), ImageKind::Backdrop, images::BACKDROP),
                ("c".to_string(), ImageKind::Backdrop, images::BACKDROP),
                ("a".to_string(), ImageKind::Backdrop, images::BACKDROP),
                ("b".to_string(), ImageKind::Logo, images::LOGO),
                ("a".to_string(), ImageKind::Logo, images::LOGO),
            ]
        );
    }

    #[test]
    fn a_header_with_one_title_asks_no_next_one_and_a_title_without_pictures_none() {
        let mut bare = feature(FeatureKind::Series);
        bare.id = ItemId("b".into());
        assert_eq!(pictures(&[with_pictures("a")], 0, None).len(), 2);
        assert!(pictures(&[bare], 0, None).is_empty());
        assert!(pictures(&[], 0, None).is_empty());
    }

    #[test]
    fn a_logo_fades_in_the_first_time_it_is_drawn_and_not_again() {
        let mut logos = Logos::default();
        let id = ItemId("title-1".into());
        let t0 = Instant::now();
        assert_eq!(logos.level(&id, t0), 0.);
        let midway = logos.level(&id, t0 + motion::page() / 2);
        assert!(midway > 0. && midway < 1., "{midway}");
        assert_eq!(logos.level(&id, t0 + motion::page()), 1.);
        // Shown again later, as its title comes back: it is there at once.
        assert_eq!(logos.level(&id, t0 + motion::page() * 40), 1.);
    }

    #[test]
    fn each_logo_has_its_own_fade() {
        let mut logos = Logos::default();
        let t0 = Instant::now();
        logos.level(&ItemId("title-1".into()), t0);
        assert_eq!(
            logos.level(&ItemId("title-2".into()), t0 + motion::page()),
            0.
        );
    }

    #[test]
    fn the_logo_times_of_titles_that_left_the_header_are_forgotten() {
        let mut logos = Logos::default();
        let t0 = Instant::now();
        for id in ["a", "b", "c"] {
            logos.level(&ItemId(id.into()), t0);
        }
        logos.keep_only(&[with_pictures("b")]);
        assert_eq!(logos.len(), 1);
        // A title that came back after leaving fades in again, like a new one.
        logos.level(&ItemId("a".into()), t0 + motion::page());
        assert_eq!(logos.level(&ItemId("b".into()), t0 + motion::page()), 1.);
        assert_eq!(logos.len(), 2);
    }

    #[test]
    fn a_logo_is_drawn_as_large_as_its_room_allows_with_its_proportions() {
        // Wide: the width fills the room. Tall: the height does.
        assert_eq!(logo_fit((880., 130.)), Some((440., 65.)));
        assert_eq!(logo_fit((200., 260.)), Some((100., 130.)));
    }

    #[test]
    fn a_logo_smaller_than_its_room_keeps_its_own_size() {
        assert_eq!(logo_fit((300., 60.)), Some((300., 60.)));
        assert_eq!(logo_fit_in((200., 50.), PAGE_LOGO), Some((200., 50.)));
        // Too wide for the page but not for the header: smaller only where it must be.
        assert_eq!(logo_fit((400., 100.)), Some((400., 100.)));
        assert_eq!(logo_fit_in((400., 100.), PAGE_LOGO), Some((360., 90.)));
    }

    #[test]
    fn a_logo_on_a_page_fits_a_smaller_room_than_the_header_logo() {
        assert_eq!(logo_fit_in((720., 144.), PAGE_LOGO), Some((360., 72.)));
        assert_eq!(
            logo_fit_in((200., 300.), PAGE_LOGO),
            Some((73.333336, 110.))
        );
        assert!(PAGE_LOGO.0 < LOGO.0 && PAGE_LOGO.1 < LOGO.1);
    }

    #[test]
    fn a_logo_with_no_size_is_not_drawn() {
        assert_eq!(logo_fit((0., 100.)), None);
        assert_eq!(logo_fit((100., 0.)), None);
        assert_eq!(logo_fit((0., 0.)), None);
    }

    #[test]
    fn the_ticker_runs_only_while_the_home_shows_two_titles_or_more() {
        assert!(ticks(true, 2));
        assert!(ticks(true, 5));
        assert!(!ticks(true, 1));
        assert!(!ticks(true, 0));
        // Left for another page: it stops, whatever the header holds.
        assert!(!ticks(false, 5));
    }

    #[test]
    fn the_time_stops_for_a_pointer_over_the_header_and_inside_the_window() {
        assert!(holds(true, true, true));
        assert!(!holds(true, true, false));
        assert!(!holds(true, false, true));
        assert!(!holds(true, false, false));
    }

    #[test]
    fn the_time_stops_too_when_the_home_is_not_shown() {
        assert!(holds(false, false, false));
        assert!(holds(false, true, true));
    }

    #[test]
    fn a_title_stays_eight_seconds_then_the_next_comes() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(5, t0);
        assert!(!carousel.tick(t0 + Duration::from_millis(7900)));
        assert_eq!(carousel.current(), 0);
        assert!(carousel.tick(t0 + secs(8)));
        assert_eq!(carousel.current(), 1);
        // The next one has its own eight seconds, counted from its coming.
        assert!(!carousel.tick(t0 + secs(15)));
        assert!(carousel.tick(t0 + secs(16)));
        assert_eq!(carousel.current(), 2);
    }

    #[test]
    fn the_last_title_gives_way_to_the_first() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(2, t0);
        assert!(carousel.tick(t0 + secs(8)));
        assert!(carousel.tick(t0 + secs(16)));
        assert_eq!(carousel.current(), 0);
    }

    #[test]
    fn the_bar_fills_over_the_eight_seconds() {
        let t0 = Instant::now();
        let carousel = Carousel::new(3, t0);
        assert_eq!(carousel.progress(t0), 0.);
        assert_eq!(carousel.progress(t0 + secs(2)), 0.25);
        assert_eq!(carousel.progress(t0 + secs(8)), 1.);
        assert_eq!(carousel.progress(t0 + secs(30)), 1.);
    }

    #[test]
    fn the_pointer_over_the_header_stops_the_time() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.pause(true, t0 + secs(3));
        assert!(carousel.paused());
        // A minute later, nothing moved.
        assert!(!carousel.tick(t0 + secs(63)));
        assert_eq!(carousel.current(), 0);
        assert_eq!(carousel.progress(t0 + secs(63)), 3. / 8.);
        // The pointer leaves: the five seconds left are still to run, no more, no less.
        carousel.pause(false, t0 + secs(63));
        assert!(!carousel.tick(t0 + Duration::from_millis(67900)));
        assert!(carousel.tick(t0 + secs(68)));
        assert_eq!(carousel.current(), 1);
    }

    #[test]
    fn pausing_twice_or_resuming_twice_changes_nothing() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.pause(true, t0 + secs(2));
        carousel.pause(true, t0 + secs(5));
        carousel.pause(false, t0 + secs(10));
        carousel.pause(false, t0 + secs(12));
        // Two seconds before the pause, and the two since the pointer left, counted from 10.
        assert_eq!(carousel.progress(t0 + secs(12)), 0.5);
    }

    #[test]
    fn a_click_on_a_mark_shows_that_title_with_all_its_time() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(5, t0);
        carousel.jump(3, t0 + secs(5));
        assert_eq!(carousel.current(), 3);
        assert_eq!(carousel.progress(t0 + secs(5)), 0.);
        assert!(!carousel.tick(t0 + secs(12)));
        assert!(carousel.tick(t0 + secs(13)));
        assert_eq!(carousel.current(), 4);
    }

    #[test]
    fn a_click_on_the_title_shown_changes_nothing() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(5, t0);
        carousel.jump(0, t0 + secs(5));
        assert_eq!(carousel.progress(t0 + secs(5)), 5. / 8.);
        assert_eq!(carousel.blend(t0 + secs(5)).leaving, None);
    }

    #[test]
    fn a_click_past_the_last_title_changes_nothing() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.jump(3, t0 + secs(1));
        assert_eq!(carousel.current(), 0);
    }

    #[test]
    fn a_click_while_the_pointer_is_over_the_header_leaves_the_time_stopped() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(5, t0);
        carousel.pause(true, t0 + secs(1));
        carousel.jump(2, t0 + secs(2));
        assert_eq!(carousel.current(), 2);
        assert!(carousel.paused());
        assert_eq!(carousel.progress(t0 + secs(50)), 0.);
        assert!(!carousel.tick(t0 + secs(50)));
    }

    #[test]
    fn one_title_never_rotates_and_none_has_nothing_to_show() {
        let t0 = Instant::now();
        let mut one = Carousel::new(1, t0);
        assert!(!one.tick(t0 + secs(100)));
        assert_eq!(one.current(), 0);
        let mut none = Carousel::new(0, t0);
        assert!(!none.tick(t0 + secs(100)));
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn a_change_cross_fades_from_the_title_it_leaves() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        assert_eq!(
            carousel.blend(t0),
            Blend {
                leaving: None,
                level: 1.
            }
        );
        carousel.tick(t0 + secs(8));
        let start = carousel.blend(t0 + secs(8));
        assert_eq!(start.leaving, Some(0));
        assert_eq!(start.level, 0.);
        assert!(carousel.crossfading(t0 + secs(8)));
        let middle = carousel.blend(t0 + secs(8) + motion::crossfade() / 2);
        assert!(middle.level > 0. && middle.level < 1., "{middle:?}");
        let end = carousel.blend(t0 + secs(8) + motion::crossfade());
        assert_eq!(
            end,
            Blend {
                leaving: None,
                level: 1.
            }
        );
        assert!(!carousel.crossfading(t0 + secs(8) + motion::crossfade()));
    }

    #[test]
    fn the_cross_fade_goes_on_while_the_time_is_stopped() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.jump(1, t0);
        carousel.pause(true, t0);
        let later = t0 + motion::crossfade();
        assert_eq!(carousel.blend(later).leaving, None);
    }

    #[test]
    fn a_click_during_a_cross_fade_leaves_from_the_title_shown_then() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(4, t0);
        carousel.jump(1, t0);
        carousel.jump(3, t0 + Duration::from_millis(50));
        assert_eq!(carousel.current(), 3);
        assert_eq!(
            carousel.blend(t0 + Duration::from_millis(50)).leaving,
            Some(1)
        );
    }

    #[test]
    fn the_texts_change_one_after_the_other_never_together() {
        assert_eq!(text_levels(0.), (1., 0.));
        assert_eq!(text_levels(0.5), (0., 0.));
        assert_eq!(text_levels(1.), (0., 1.));
        for step in 0..=100 {
            let (leaving, arriving) = text_levels(step as f32 / 100.);
            assert!(leaving == 0. || arriving == 0., "step {step}");
        }
    }

    #[test]
    fn a_reload_keeps_the_title_shown_and_its_time() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.jump(1, t0);
        // The new list has it in third place, and a title more.
        carousel.retarget(4, Some(2), t0 + secs(3));
        assert_eq!((carousel.len(), carousel.current()), (4, 2));
        assert_eq!(carousel.progress(t0 + secs(3)), 3. / 8.);
        assert_eq!(carousel.blend(t0 + secs(3)).leaving, None);
    }

    #[test]
    fn a_reload_that_lost_the_title_starts_again_from_the_first() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.jump(2, t0);
        carousel.retarget(2, None, t0 + secs(5));
        assert_eq!((carousel.len(), carousel.current()), (2, 0));
        assert_eq!(carousel.progress(t0 + secs(5)), 0.);
    }

    #[test]
    fn the_title_shown_is_found_again_in_the_list_asked_again() {
        let mut other = feature(FeatureKind::Series);
        other.id = ItemId("title-2".into());
        let list = [other, feature(FeatureKind::Film)];
        assert_eq!(kept(Some(&ItemId("title-1".into())), &list), Some(1));
        assert_eq!(kept(Some(&ItemId("title-9".into())), &list), None);
        assert_eq!(kept(None, &list), None);
    }

    #[test]
    fn a_reload_that_empties_the_list_leaves_nothing_to_show() {
        let t0 = Instant::now();
        let mut carousel = Carousel::new(3, t0);
        carousel.retarget(0, None, t0);
        assert_eq!(carousel.len(), 0);
        assert!(!carousel.tick(t0 + secs(100)));
    }

    #[test]
    fn the_title_is_the_logo_when_the_server_has_one() {
        let mut shown = feature(FeatureKind::Film);
        assert_eq!(headline(&shown), Headline::Text("Les Marées".into()));
        let logo = ImageRef {
            item: ItemId("title-1".into()),
            kind: ImageKind::Logo,
            tag: "l1".into(),
        };
        shown.logo = Some(logo.clone());
        assert_eq!(headline(&shown), Headline::Logo(logo));
    }

    #[test]
    fn the_header_says_whether_it_is_a_movie_or_a_series() {
        assert_eq!(kind_label(&feature(FeatureKind::Film)), "Movie");
        assert_eq!(kind_label(&feature(FeatureKind::Series)), "Series");
    }

    #[test]
    fn a_film_says_year_rating_runtime_and_age_rating() {
        assert_eq!(
            meta(&feature(FeatureKind::Film)),
            ["2025", "★ 7.8", "2h 4m", "12+"]
        );
    }

    #[test]
    fn a_series_has_no_runtime_and_a_title_with_nothing_known_says_less() {
        assert_eq!(
            meta(&feature(FeatureKind::Series)),
            ["2025", "★ 7.8", "12+"]
        );
        let mut bare = feature(FeatureKind::Film);
        bare.year = None;
        bare.community_rating = None;
        bare.official_rating = None;
        bare.runtime = None;
        assert_eq!(meta(&bare), Vec::<String>::new());
    }

    #[test]
    fn the_button_resumes_a_film_started_and_plays_the_rest() {
        let mut shown = feature(FeatureKind::Film);
        assert_eq!(play_label(&shown), "Play");
        shown.resume = Some(secs(72 * 60));
        assert_eq!(play_label(&shown), "Resume from 1h 12m");
        assert_eq!(play_label(&feature(FeatureKind::Series)), "Play");
    }
}
