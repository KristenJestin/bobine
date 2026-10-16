//! Which skip button the viewer is offered, and when it shows. The segments are what the server
//! marked; no window and no clock of its own here: the view reads this and the tests drive it
//! with the position and the time they choose.

use std::time::{Duration, Instant};

use bobine::provider::{Segment, SegmentKind};

use crate::series_page::reached_the_end;

/// Without a pointer move for this long, the button fades out, though its segment goes on.
pub const SHOWN_FOR: Duration = Duration::from_secs(5);

/// What the button says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Label {
    Intro,
    Recap,
    Preview,
    Credits,
    NextEpisode,
}

impl Label {
    pub fn text(self) -> &'static str {
        match self {
            Self::Intro => "Skip intro",
            Self::Recap => "Skip recap",
            Self::Preview => "Skip preview",
            Self::Credits => "Skip credits",
            Self::NextEpisode => "Next episode",
        }
    }
}

/// What pressing the button does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Press {
    /// Seeks to the end of the segment, in seconds.
    SeekTo(f64),
    /// The playback ends as if it reached its end.
    EndPlayback,
    /// The playback ends as if it reached its end, and the next episode plays at once.
    NextEpisode,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Button {
    pub label: Label,
    pub press: Press,
}

/// The button of the segment `position` (seconds) is in, if any; the first of them when
/// segments overlap. The credits end the playback (and offer the next episode when there is
/// one, `has_next`) only when they run to the real end of the `duration`; with something after
/// them, or a duration not known yet, they only seek past themselves.
pub fn button_at(
    segments: &[Segment],
    position: f64,
    duration: f64,
    has_next: bool,
) -> Option<Button> {
    let segment = segments.iter().find(|segment| {
        segment.start.as_secs_f64() <= position && position < segment.end.as_secs_f64()
    })?;
    let end = segment.end.as_secs_f64();
    let (label, press) = match segment.kind {
        SegmentKind::Intro => (Label::Intro, Press::SeekTo(end)),
        SegmentKind::Recap => (Label::Recap, Press::SeekTo(end)),
        SegmentKind::Preview => (Label::Preview, Press::SeekTo(end)),
        SegmentKind::Outro if !reached_the_end(end, duration) => {
            (Label::Credits, Press::SeekTo(end))
        }
        SegmentKind::Outro if has_next => (Label::NextEpisode, Press::NextEpisode),
        SegmentKind::Outro => (Label::Credits, Press::EndPlayback),
    };
    Some(Button { label, press })
}

/// The button as the window offers it: it gives way while the seek preview shows, both sit
/// above the bar on the right and the button would cover the thumbnail.
pub fn offered(button: Option<Button>, previewing: bool) -> Option<Button> {
    button.filter(|_| !previewing)
}

/// Where the segments sit on the seek bar: the start of each and its width, as fractions of
/// the bar. A segment past the end is cut at the end; none shows before the duration is known.
pub fn marks(segments: &[Segment], duration: f64) -> Vec<(f32, f32)> {
    if duration <= 0. {
        return Vec::new();
    }
    segments
        .iter()
        .filter_map(|segment| {
            let start = segment.start.as_secs_f64().min(duration);
            let end = segment.end.as_secs_f64().min(duration);
            (end > start).then(|| ((start / duration) as f32, ((end - start) / duration) as f32))
        })
        .collect()
}

/// When the button shows: when its segment starts and each time the pointer moves, for
/// `SHOWN_FOR`; never outside its segment.
#[derive(Debug, Default)]
pub struct Prompt {
    label: Option<Label>,
    woke: Option<Instant>,
}

impl Prompt {
    /// The button the position gives now, at each update: a new one wakes the prompt.
    pub fn follow(&mut self, button: Option<Button>, now: Instant) {
        let label = button.map(|button| button.label);
        if label.is_some() && label != self.label {
            self.woke = Some(now);
        }
        self.label = label;
    }

    pub fn mouse_moved(&mut self, now: Instant) {
        self.woke = Some(now);
    }

    pub fn visible(&self, now: Instant) -> bool {
        self.label.is_some()
            && self
                .woke
                .is_some_and(|woke| now.saturating_duration_since(woke) < SHOWN_FOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(kind: SegmentKind, start: u64, end: u64) -> Segment {
        Segment {
            kind,
            start: Duration::from_secs(start),
            end: Duration::from_secs(end),
        }
    }

    /// The episode of `episode_segments`: its credits run to the very end.
    const DURATION: f64 = 2820.;

    fn episode_segments() -> Vec<Segment> {
        vec![
            segment(SegmentKind::Recap, 0, 40),
            segment(SegmentKind::Intro, 40, 130),
            segment(SegmentKind::Outro, 2700, 2820),
        ]
    }

    #[test]
    fn segments_are_drawn_on_the_seek_bar_at_their_times() {
        let marks = marks(&episode_segments(), DURATION);
        let expected = [
            (0., 40. / 2820.),
            (40. / 2820., 90. / 2820.),
            (2700. / 2820., 120. / 2820.),
        ];
        assert_eq!(marks.len(), 3);
        for ((start, width), (want_start, want_width)) in marks.iter().zip(expected) {
            assert!((start - want_start).abs() < 1e-6, "{marks:?}");
            assert!((width - want_width).abs() < 1e-6, "{marks:?}");
        }
    }

    #[test]
    fn segments_wait_for_the_duration_and_stop_at_the_end() {
        assert_eq!(marks(&episode_segments(), 0.), []);
        let long = [segment(SegmentKind::Outro, 2700, 3000)];
        let [(start, width)] = marks(&long, DURATION)[..] else {
            panic!();
        };
        assert!((start + width - 1.).abs() < 1e-6);
        let past = [segment(SegmentKind::Preview, 2900, 3000)];
        assert_eq!(marks(&past, DURATION), []);
    }

    fn label(segments: &[Segment], position: f64, has_next: bool) -> Option<Label> {
        button_at(segments, position, DURATION, has_next).map(|button| button.label)
    }

    #[test]
    fn during_an_intro_the_button_skips_it_and_seeks_to_its_end() {
        let segments = episode_segments();
        let button = button_at(&segments, 60., DURATION, true).unwrap();
        assert_eq!(button.label, Label::Intro);
        assert_eq!(button.label.text(), "Skip intro");
        assert_eq!(button.press, Press::SeekTo(130.));
    }

    #[test]
    fn during_a_recap_the_button_skips_the_summary() {
        let segments = episode_segments();
        let button = button_at(&segments, 10., DURATION, true).unwrap();
        assert_eq!(button.label.text(), "Skip recap");
        assert_eq!(button.press, Press::SeekTo(40.));
    }

    #[test]
    fn during_a_preview_the_button_skips_it() {
        let segments = [segment(SegmentKind::Preview, 100, 130)];
        let button = button_at(&segments, 100., DURATION, false).unwrap();
        assert_eq!(button.label.text(), "Skip preview");
        assert_eq!(button.press, Press::SeekTo(130.));
    }

    #[test]
    fn the_button_shows_from_the_start_of_the_segment_to_its_end_excluded() {
        let segments = [segment(SegmentKind::Intro, 40, 130)];
        assert_eq!(label(&segments, 39.9, false), None);
        assert_eq!(label(&segments, 40., false), Some(Label::Intro));
        assert_eq!(label(&segments, 129.9, false), Some(Label::Intro));
        assert_eq!(label(&segments, 130., false), None);
    }

    #[test]
    fn between_the_segments_there_is_no_button() {
        let segments = episode_segments();
        assert_eq!(label(&segments, 1200., true), None);
    }

    #[test]
    fn the_credits_of_an_episode_with_a_next_one_offer_it() {
        let segments = episode_segments();
        let button = button_at(&segments, 2750., DURATION, true).unwrap();
        assert_eq!(button.label.text(), "Next episode");
        assert_eq!(button.press, Press::NextEpisode);
    }

    #[test]
    fn the_credits_of_a_last_episode_or_a_film_end_the_playback() {
        let segments = episode_segments();
        let button = button_at(&segments, 2750., DURATION, false).unwrap();
        assert_eq!(button.label.text(), "Skip credits");
        assert_eq!(button.press, Press::EndPlayback);
    }

    #[test]
    fn credits_at_the_end_end_the_film() {
        // Ten seconds before the duration still is the real end.
        let segments = [segment(SegmentKind::Outro, 2700, 2810)];
        let button = button_at(&segments, 2750., DURATION, false).unwrap();
        assert_eq!(button.label.text(), "Skip credits");
        assert_eq!(button.press, Press::EndPlayback);
    }

    #[test]
    fn credits_followed_by_a_scene_seek_past_them() {
        let segments = [segment(SegmentKind::Outro, 2700, 2780)];
        let button = button_at(&segments, 2750., DURATION, false).unwrap();
        assert_eq!(button.label.text(), "Skip credits");
        assert_eq!(button.press, Press::SeekTo(2780.));
    }

    #[test]
    fn a_next_episode_is_offered_only_for_credits_at_the_end() {
        let at_the_end = [segment(SegmentKind::Outro, 2700, 2820)];
        let button = button_at(&at_the_end, 2750., DURATION, true).unwrap();
        assert_eq!(button.label, Label::NextEpisode);
        let before_a_scene = [segment(SegmentKind::Outro, 2700, 2780)];
        let button = button_at(&before_a_scene, 2750., DURATION, true).unwrap();
        assert_eq!(button.label, Label::Credits);
        assert_eq!(button.press, Press::SeekTo(2780.));
    }

    #[test]
    fn credits_of_an_unknown_duration_seek_to_their_end() {
        let segments = [segment(SegmentKind::Outro, 2700, 2820)];
        let button = button_at(&segments, 2750., 0., true).unwrap();
        assert_eq!(button.label, Label::Credits);
        assert_eq!(button.press, Press::SeekTo(2820.));
    }

    #[test]
    fn the_button_gives_way_while_the_seek_preview_shows() {
        let segments = [segment(SegmentKind::Outro, 2700, 2820)];
        let button = button_at(&segments, 2750., DURATION, true);
        assert!(button.is_some());
        assert_eq!(offered(button, true), None);
        assert_eq!(offered(button, false), button);
        assert_eq!(offered(None, false), None);
    }

    #[test]
    fn an_item_without_segments_has_no_button() {
        assert_eq!(label(&[], 60., true), None);
    }

    #[test]
    fn overlapping_segments_give_the_first_one_started() {
        let segments = [
            segment(SegmentKind::Recap, 0, 60),
            segment(SegmentKind::Intro, 30, 90),
        ];
        assert_eq!(label(&segments, 45., false), Some(Label::Recap));
    }

    fn at(start: Instant, seconds: f64) -> Instant {
        start + Duration::from_secs_f64(seconds)
    }

    fn intro() -> Option<Button> {
        button_at(
            &[segment(SegmentKind::Intro, 40, 130)],
            60.,
            DURATION,
            false,
        )
    }

    #[test]
    fn nothing_shows_while_there_is_no_button() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(None, start);
        assert!(!prompt.visible(start));
        prompt.mouse_moved(at(start, 1.));
        assert!(!prompt.visible(at(start, 1.)));
    }

    #[test]
    fn the_button_shows_when_its_segment_starts_and_fades_after_a_few_seconds() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(intro(), start);
        assert!(prompt.visible(start));
        assert!(prompt.visible(at(start, 4.9)));
        assert!(!prompt.visible(at(start, 5.)));
    }

    #[test]
    fn the_button_goes_when_its_segment_ends() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(intro(), start);
        prompt.follow(None, at(start, 2.));
        assert!(!prompt.visible(at(start, 2.)));
    }

    #[test]
    fn a_pointer_move_brings_the_button_back_while_its_segment_goes_on() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(intro(), start);
        prompt.follow(intro(), at(start, 8.));
        assert!(!prompt.visible(at(start, 8.)));
        prompt.mouse_moved(at(start, 9.));
        assert!(prompt.visible(at(start, 9.)));
        assert!(prompt.visible(at(start, 13.9)));
        assert!(!prompt.visible(at(start, 14.)));
    }

    #[test]
    fn each_pointer_move_restarts_the_few_seconds() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(intro(), start);
        prompt.mouse_moved(at(start, 4.));
        assert!(prompt.visible(at(start, 8.9)));
        assert!(!prompt.visible(at(start, 9.)));
    }

    #[test]
    fn updates_in_the_same_segment_do_not_bring_the_button_back() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(intro(), start);
        for second in 1..10 {
            prompt.follow(intro(), at(start, f64::from(second)));
        }
        assert!(!prompt.visible(at(start, 10.)));
    }

    #[test]
    fn the_next_segment_shows_its_own_button_at_once() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.follow(intro(), start);
        let credits = button_at(
            &[segment(SegmentKind::Outro, 2700, 2820)],
            2710.,
            DURATION,
            false,
        );
        prompt.follow(credits, at(start, 20.));
        assert!(prompt.visible(at(start, 20.)));
        assert!(!prompt.visible(at(start, 25.)));
    }

    #[test]
    fn a_pointer_move_before_the_segment_does_not_show_a_button_in_advance() {
        let start = Instant::now();
        let mut prompt = Prompt::default();
        prompt.mouse_moved(start);
        prompt.follow(None, at(start, 1.));
        prompt.follow(intro(), at(start, 30.));
        assert!(prompt.visible(at(start, 30.)));
        assert!(!prompt.visible(at(start, 35.)));
    }
}
