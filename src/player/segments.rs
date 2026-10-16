//! Which skip button the viewer is offered, and when it shows. The segments are what the server
//! marked; no window and no clock of its own here: the view reads this and the tests drive it
//! with the position and the time they choose.

use std::time::{Duration, Instant};

use bobine::provider::{Segment, SegmentKind};

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
            Self::Intro => "Passer l'intro",
            Self::Recap => "Passer le résumé",
            Self::Preview => "Passer l'aperçu",
            Self::Credits => "Passer le générique",
            Self::NextEpisode => "Épisode suivant",
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
/// segments overlap. The credits offer the next episode when there is one (`has_next`).
pub fn button_at(segments: &[Segment], position: f64, has_next: bool) -> Option<Button> {
    let segment = segments.iter().find(|segment| {
        segment.start.as_secs_f64() <= position && position < segment.end.as_secs_f64()
    })?;
    let end = segment.end.as_secs_f64();
    let (label, press) = match segment.kind {
        SegmentKind::Intro => (Label::Intro, Press::SeekTo(end)),
        SegmentKind::Recap => (Label::Recap, Press::SeekTo(end)),
        SegmentKind::Preview => (Label::Preview, Press::SeekTo(end)),
        SegmentKind::Outro if has_next => (Label::NextEpisode, Press::NextEpisode),
        SegmentKind::Outro => (Label::Credits, Press::EndPlayback),
    };
    Some(Button { label, press })
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

    fn episode_segments() -> Vec<Segment> {
        vec![
            segment(SegmentKind::Recap, 0, 40),
            segment(SegmentKind::Intro, 40, 130),
            segment(SegmentKind::Outro, 2700, 2820),
        ]
    }

    fn label(segments: &[Segment], position: f64, has_next: bool) -> Option<Label> {
        button_at(segments, position, has_next).map(|button| button.label)
    }

    #[test]
    fn during_an_intro_the_button_skips_it_and_seeks_to_its_end() {
        let segments = episode_segments();
        let button = button_at(&segments, 60., true).unwrap();
        assert_eq!(button.label, Label::Intro);
        assert_eq!(button.label.text(), "Passer l'intro");
        assert_eq!(button.press, Press::SeekTo(130.));
    }

    #[test]
    fn during_a_recap_the_button_skips_the_summary() {
        let segments = episode_segments();
        let button = button_at(&segments, 10., true).unwrap();
        assert_eq!(button.label.text(), "Passer le résumé");
        assert_eq!(button.press, Press::SeekTo(40.));
    }

    #[test]
    fn during_a_preview_the_button_skips_it() {
        let segments = [segment(SegmentKind::Preview, 100, 130)];
        let button = button_at(&segments, 100., false).unwrap();
        assert_eq!(button.label.text(), "Passer l'aperçu");
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
        let button = button_at(&segments, 2750., true).unwrap();
        assert_eq!(button.label.text(), "Épisode suivant");
        assert_eq!(button.press, Press::NextEpisode);
    }

    #[test]
    fn the_credits_of_a_last_episode_or_a_film_end_the_playback() {
        let segments = episode_segments();
        let button = button_at(&segments, 2750., false).unwrap();
        assert_eq!(button.label.text(), "Passer le générique");
        assert_eq!(button.press, Press::EndPlayback);
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
        button_at(&[segment(SegmentKind::Intro, 40, 130)], 60., false)
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
        let credits = button_at(&[segment(SegmentKind::Outro, 2700, 2820)], 2710., false);
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
