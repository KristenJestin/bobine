//! One film being played: the player and the video surface, and what the controls show. The view
//! draws this and forwards the keys, the mouse and the layout to it; the tests drive it with a
//! fake player, a fake display and their own clock.

use std::time::Instant;

use bobine::provider::Segment;

use super::controls::{Controls, KeyAction, key_action, subtitle_position};
use super::segments::{Button, Label, Press, Prompt, button_at};
use super::{Media, Player, PlayerError, PlayerEvent, Seek, Track, TrackKind, tracks};
use crate::video::{Rect, VideoDisplay, placement};

/// What the session asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRequest {
    ToggleFullscreen,
    ExitFullscreen,
    Leave,
    /// The credits were skipped: the playback ends as if it reached its end.
    EndPlayback,
    /// Likewise, and the next episode plays at once.
    NextEpisode,
}

/// One line of the audio and subtitles panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// `None`: the "Aucun" line of the subtitles.
    pub id: Option<i64>,
    pub label: String,
    pub current: bool,
}

pub struct Session {
    // Declared first, so dropped first: the surface stops drawing before the player goes.
    display: Box<dyn VideoDisplay>,
    player: Box<dyn Player>,
    pub title: String,
    pub subtitle: Option<String>,
    controls: Controls,
    panel_open: bool,
    pub position: f64,
    /// mpv sent a `time-pos`: before it, `position` is the 0 it starts at, not where the film is.
    position_known: bool,
    pub duration: f64,
    pub paused: bool,
    tracks: Vec<Track>,
    error: Option<PlayerError>,
    window_height: f32,
    /// Whether the controls showed at the last update, and mpv's `sub-pos` for it.
    controls_shown: bool,
    subtitle_position: f64,
    /// Kept once seen: an update whose answer is dropped (a mouse move) must not lose it.
    ended: bool,
    /// The file is open and playing: the server may be told.
    loaded: bool,
    /// When the file was open, and when its first picture came.
    loaded_at: Option<Instant>,
    first_frame_at: Option<Instant>,
    /// How many seeks the viewer asked for, so a seek can be told to the server.
    seeks: u64,
    /// What the server marked as skippable, by start, and whether an episode follows this one.
    segments: Vec<Segment>,
    has_next: bool,
    prompt: Prompt,
    /// The skip button as it shows now, if it does.
    skip: Option<Button>,
    /// The button just pressed, hidden until its segment is over: mpv takes a moment to say
    /// where the seek went.
    pressed: Option<Label>,
    /// The viewer skipped to the end: the position is the end, whatever mpv says meanwhile.
    skipped_to_end: bool,
}

impl Session {
    pub fn new(
        player: Box<dyn Player>,
        display: Box<dyn VideoDisplay>,
        title: String,
        subtitle: Option<String>,
    ) -> Self {
        Self {
            display,
            player,
            title,
            subtitle,
            controls: Controls::default(),
            panel_open: false,
            position: 0.,
            position_known: false,
            duration: 0.,
            paused: false,
            tracks: Vec::new(),
            error: None,
            window_height: 0.,
            controls_shown: false,
            subtitle_position: 100.,
            ended: false,
            loaded: false,
            loaded_at: None,
            first_frame_at: None,
            seeks: 0,
            segments: Vec::new(),
            has_next: false,
            prompt: Prompt::default(),
            skip: None,
            pressed: None,
            skipped_to_end: false,
        }
    }

    /// Plays `media` instead of the film that did not play, where it was: what was said of
    /// that one (its error, its loading, its end) goes with it.
    pub fn restart(&mut self, media: Media<'_>) -> Result<(), PlayerError> {
        self.error = None;
        self.loaded = false;
        self.loaded_at = None;
        self.first_frame_at = None;
        self.ended = false;
        self.tracks.clear();
        self.player.load(media)
    }

    /// The error is no longer the film's last word: something else is tried.
    pub fn forget_error(&mut self) {
        self.error = None;
    }

    /// The film cannot play, for a reason the player did not give.
    pub fn fail(&mut self, error: PlayerError) {
        self.error = Some(error);
    }

    pub fn loaded_at(&self) -> Option<Instant> {
        self.loaded_at
    }

    pub fn first_frame_at(&self) -> Option<Instant> {
        self.first_frame_at
    }

    /// What the server marked in this film or episode, by start.
    pub fn set_segments(&mut self, segments: Vec<Segment>) {
        self.segments = segments;
    }

    /// Whether an episode follows this one: its credits offer it.
    pub fn set_next_episode(&mut self, has_next: bool) {
        self.has_next = has_next;
    }

    /// The skip button, while it shows (an intro, a recap, the credits).
    pub fn skip_button(&self) -> Option<Button> {
        self.skip
    }

    /// The viewer pressed the skip button, with the mouse or Enter.
    pub fn press_skip(&mut self) -> Option<WindowRequest> {
        let button = self.skip.take()?;
        self.pressed = Some(button.label);
        match button.press {
            Press::SeekTo(seconds) => {
                self.seeks += 1;
                self.player.seek(Seek::To(seconds));
                None
            }
            Press::EndPlayback => {
                self.skip_to_end();
                Some(WindowRequest::EndPlayback)
            }
            Press::NextEpisode => {
                self.skip_to_end();
                Some(WindowRequest::NextEpisode)
            }
        }
    }

    /// The same end as the file's own (`ended`), at the end position: the server marks it
    /// watched by the rule of a real end.
    fn skip_to_end(&mut self) {
        self.skipped_to_end = true;
        self.ended = true;
        self.position = self.duration;
    }

    /// A key pressed in the player; what the window must do, if anything.
    pub fn key(
        &mut self,
        key: &str,
        with_modifier: bool,
        fullscreen: bool,
    ) -> Option<WindowRequest> {
        let action = key_action(key, with_modifier, fullscreen)?;
        // Escape closes the panel before it leaves fullscreen or the player.
        if self.panel_open && matches!(action, KeyAction::ExitFullscreen | KeyAction::Leave) {
            self.panel_open = false;
            return None;
        }
        match action {
            KeyAction::TogglePause => self.toggle_pause(),
            KeyAction::SeekBy(seconds) => self.skip(seconds),
            KeyAction::ToggleFullscreen => return Some(WindowRequest::ToggleFullscreen),
            KeyAction::ExitFullscreen => return Some(WindowRequest::ExitFullscreen),
            KeyAction::Leave => return Some(WindowRequest::Leave),
            KeyAction::PressSkip => return self.press_skip(),
        }
        None
    }

    pub fn toggle_pause(&mut self) {
        // Shown at once; mpv's own word follows as an event.
        self.paused = !self.paused;
        self.player.set_paused(self.paused);
    }

    pub fn skip(&mut self, seconds: f64) {
        self.seeks += 1;
        self.player.seek(Seek::By(seconds));
    }

    /// A click on the seek bar, `fraction` of the way along it.
    pub fn seek_to_fraction(&mut self, fraction: f32) {
        let fraction = f64::from(fraction.clamp(0., 1.));
        self.seeks += 1;
        self.player.seek(Seek::To(fraction * self.duration));
    }

    /// The pointer drags along the seek bar: the film follows to the nearest keyframe, which is
    /// quick, and no seek is counted. The seek the server is told of is the one on release.
    pub fn scrub_to_fraction(&mut self, fraction: f32) {
        let fraction = f64::from(fraction.clamp(0., 1.));
        self.player.seek(Seek::Rough(fraction * self.duration));
    }

    pub fn mouse_moved(&mut self, now: Instant) {
        self.controls.mouse_moved(now);
        self.prompt.mouse_moved(now);
    }

    /// The controls stay while the panel is open: it is reached through them.
    pub fn controls_visible(&self, now: Instant) -> bool {
        self.panel_open || self.controls.visible(now)
    }

    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    pub fn toggle_panel(&mut self) {
        self.panel_open = !self.panel_open;
    }

    /// A click anywhere but in the panel closes it, the button that opened it included.
    pub fn clicked_outside_panel(&mut self) {
        self.panel_open = false;
    }

    pub fn audio_choices(&self) -> Vec<Choice> {
        self.tracks_of(TrackKind::Audio).collect()
    }

    /// "Aucun" first, then the subtitle tracks.
    pub fn subtitle_choices(&self) -> Vec<Choice> {
        let tracks: Vec<Choice> = self.tracks_of(TrackKind::Subtitle).collect();
        let none = Choice {
            id: None,
            label: "Aucun".into(),
            current: !tracks.iter().any(|choice| choice.current),
        };
        std::iter::once(none).chain(tracks).collect()
    }

    fn tracks_of(&self, kind: TrackKind) -> impl Iterator<Item = Choice> {
        self.tracks
            .iter()
            .filter(move |track| track.kind == kind)
            .map(|track| Choice {
                id: Some(track.id),
                label: tracks::label(track),
                current: track.selected,
            })
    }

    pub fn select_audio(&mut self, id: i64) {
        self.player.select_audio(Some(id));
    }

    pub fn select_subtitle(&mut self, id: Option<i64>) {
        self.player.select_subtitle(id);
    }

    pub fn position_known(&self) -> bool {
        self.position_known
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn seeks(&self) -> u64 {
        self.seeks
    }

    pub fn error(&self) -> Option<&PlayerError> {
        self.error.as_ref()
    }

    /// Reads what the player says and shows or hides the cursor with the controls. Returns
    /// whether the film reached its end, now or at an earlier update.
    pub fn update(&mut self, now: Instant) -> bool {
        for event in self.player.events() {
            match event {
                PlayerEvent::Position(seconds) if !self.skipped_to_end => {
                    self.position = seconds;
                    self.position_known = true;
                }
                PlayerEvent::Position(_) => {}
                PlayerEvent::Duration(seconds) => self.duration = seconds,
                PlayerEvent::Paused(paused) => self.paused = paused,
                PlayerEvent::Tracks(tracks) => self.tracks = tracks,
                PlayerEvent::EndOfFile => self.ended = true,
                PlayerEvent::Error(error) => self.error = Some(error),
                PlayerEvent::Loaded => {
                    self.loaded = true;
                    self.loaded_at = Some(now);
                }
                PlayerEvent::FirstFrame => {
                    self.first_frame_at.get_or_insert(now);
                }
                // The film plays; a refused subtitle is simply not in the panel.
                PlayerEvent::SubtitleFailed(_) => {}
            }
        }
        self.follow_segments(now);
        let shown = self.controls_visible(now);
        // Told every time: the display can only act once the pointer is over the window.
        self.display.set_cursor_visible(shown);
        self.controls_shown = shown;
        self.place_subtitles();
        self.ended
    }

    /// The skip button of the segment the film is in, shown by the prompt's rules.
    fn follow_segments(&mut self, now: Instant) {
        let button = self
            .position_known
            .then(|| button_at(&self.segments, self.position, self.duration, self.has_next))
            .flatten();
        if button.map(|button| button.label) != self.pressed {
            self.pressed = None;
        }
        let button = button.filter(|button| Some(button.label) != self.pressed);
        self.prompt.follow(button, now);
        self.skip = button.filter(|_| self.prompt.visible(now));
    }

    /// Puts the video where the view laid it out: called on every layout, so the subsurface
    /// follows each size change of the window, fullscreen included.
    pub fn layout(&mut self, window: (f32, f32), video: Rect, scale: f32) {
        self.display.place(placement(window, video, scale));
        self.window_height = window.1;
        self.place_subtitles();
    }

    /// Above the bottom controls while they show, so neither hides the other.
    fn place_subtitles(&mut self) {
        let position = subtitle_position(self.window_height, self.controls_shown);
        if position != self.subtitle_position {
            self.subtitle_position = position;
            self.player.set_subtitle_position(position);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::player::fake::{Call, FakePlayer};
    use crate::video::Placement;
    use crate::video::fake::FakeDisplay;

    struct Fixture {
        session: Session,
        player: FakePlayer,
        display: FakeDisplay,
    }

    impl Fixture {
        fn calls(&self) -> Vec<Call> {
            self.player.calls.borrow().clone()
        }

        fn queue(&self, events: Vec<PlayerEvent>) {
            self.player.queued.borrow_mut().extend(events);
        }
    }

    fn fixture() -> Fixture {
        let player = FakePlayer::default();
        let display = FakeDisplay::default();
        let session = Session::new(
            Box::new(FakePlayer {
                calls: player.calls.clone(),
                queued: player.queued.clone(),
            }),
            Box::new(FakeDisplay {
                placements: display.placements.clone(),
                cursor: display.cursor.clone(),
            }),
            "Le Dernier Hiver".into(),
            None,
        );
        Fixture {
            session,
            player,
            display,
        }
    }

    fn whole(width: f32, height: f32) -> Rect {
        Rect {
            x: 0.,
            y: 0.,
            width,
            height,
        }
    }

    fn track(kind: TrackKind, id: i64, language: &str, codec: &str, selected: bool) -> Track {
        Track {
            id,
            kind,
            title: None,
            language: Some(language.into()),
            codec: Some(codec.into()),
            channels: None,
            forced: false,
            external: false,
            selected,
        }
    }

    #[test]
    fn the_subsurface_geometry_follows_every_window_size_change_fullscreen_included() {
        let mut f = fixture();
        f.session.layout((1440., 810.), whole(1440., 810.), 1.);
        f.session.layout((1920., 1080.), whole(1920., 1080.), 1.);
        f.session.layout((1000., 600.), whole(1000., 600.), 1.);
        f.session.layout((1920., 1080.), whole(1920., 1080.), 2.);
        let sizes: Vec<(i32, i32, i32, i32)> = f
            .display
            .placements
            .borrow()
            .iter()
            .map(|p: &Placement| (p.width, p.height, p.buffer_width, p.buffer_height))
            .collect();
        assert_eq!(
            sizes,
            [
                (1440, 810, 1440, 810),
                (1920, 1080, 1920, 1080),
                (1000, 600, 1000, 600),
                (1920, 1080, 3840, 2160),
            ]
        );
    }

    #[test]
    fn space_pauses_a_playing_film_and_plays_it_again() {
        let mut f = fixture();
        assert_eq!(f.session.key("space", false, false), None);
        assert!(f.session.paused);
        f.session.key("space", false, false);
        assert!(!f.session.paused);
        assert_eq!(f.calls(), [Call::Paused(true), Call::Paused(false)]);
    }

    #[test]
    fn the_arrows_skip_ten_seconds() {
        let mut f = fixture();
        f.session.key("left", false, false);
        f.session.key("right", false, false);
        assert_eq!(
            f.calls(),
            [Call::Seek(Seek::By(-10.)), Call::Seek(Seek::By(10.))]
        );
    }

    #[test]
    fn f_asks_the_window_for_fullscreen() {
        let mut f = fixture();
        assert_eq!(
            f.session.key("f", false, false),
            Some(WindowRequest::ToggleFullscreen)
        );
    }

    #[test]
    fn escape_leaves_fullscreen_then_the_player() {
        let mut f = fixture();
        assert_eq!(
            f.session.key("escape", false, true),
            Some(WindowRequest::ExitFullscreen)
        );
        assert_eq!(
            f.session.key("escape", false, false),
            Some(WindowRequest::Leave)
        );
    }

    #[test]
    fn a_click_on_the_seek_bar_seeks_there() {
        let mut f = fixture();
        f.session.duration = 6000.;
        f.session.seek_to_fraction(0.25);
        f.session.seek_to_fraction(1.5);
        assert_eq!(
            f.calls(),
            [Call::Seek(Seek::To(1500.)), Call::Seek(Seek::To(6000.))]
        );
    }

    #[test]
    fn a_drag_along_the_seek_bar_moves_the_film_without_counting_a_seek() {
        let mut f = fixture();
        f.session.duration = 6000.;
        f.session.scrub_to_fraction(0.25);
        f.session.scrub_to_fraction(0.5);
        f.session.scrub_to_fraction(1.5);
        assert_eq!(
            f.calls(),
            [
                Call::Seek(Seek::Rough(1500.)),
                Call::Seek(Seek::Rough(3000.)),
                Call::Seek(Seek::Rough(6000.))
            ]
        );
        assert_eq!(f.session.seeks(), 0);
    }

    #[test]
    fn the_cursor_is_hidden_until_the_mouse_moves_and_hides_again_with_the_controls() {
        let mut f = fixture();
        let start = Instant::now();
        f.session.update(start);
        f.session.mouse_moved(start);
        f.session.update(start);
        f.session.update(start + Duration::from_secs(1));
        f.session.update(start + Duration::from_secs(3));
        assert!(!f.session.controls_visible(start + Duration::from_secs(3)));
        let mut seen = f.display.cursor.borrow().clone();
        seen.dedup();
        assert_eq!(seen, [false, true, false]);
    }

    #[test]
    fn the_cursor_wish_is_told_on_every_update() {
        // The display may only be able to hide it later, once the pointer has entered.
        let mut f = fixture();
        let start = Instant::now();
        f.session.update(start);
        f.session.update(start + Duration::from_secs(1));
        assert_eq!(*f.display.cursor.borrow(), [false, false]);
    }

    #[test]
    fn the_panel_keeps_the_controls_shown() {
        let mut f = fixture();
        let start = Instant::now();
        f.session.mouse_moved(start);
        f.session.toggle_panel();
        assert!(f.session.panel_open());
        assert!(f.session.controls_visible(start + Duration::from_secs(10)));
        f.session.toggle_panel();
        assert!(!f.session.controls_visible(start + Duration::from_secs(10)));
    }

    #[test]
    fn escape_closes_the_panel_before_anything_else() {
        let mut f = fixture();
        f.session.toggle_panel();
        assert_eq!(f.session.key("escape", false, true), None);
        assert!(!f.session.panel_open());
        assert_eq!(
            f.session.key("escape", false, true),
            Some(WindowRequest::ExitFullscreen)
        );
    }

    #[test]
    fn a_click_outside_the_panel_closes_it() {
        let mut f = fixture();
        f.session.toggle_panel();
        f.session.clicked_outside_panel();
        assert!(!f.session.panel_open());
        // Closed, it stays closed.
        f.session.clicked_outside_panel();
        assert!(!f.session.panel_open());
    }

    #[test]
    fn the_player_events_update_what_the_controls_show() {
        let mut f = fixture();
        f.queue(vec![
            PlayerEvent::Duration(5400.),
            PlayerEvent::Position(61.),
            PlayerEvent::Paused(true),
        ]);
        assert!(!f.session.update(Instant::now()));
        assert_eq!(
            (f.session.duration, f.session.position, f.session.paused),
            (5400., 61., true)
        );
    }

    #[test]
    fn the_panel_lists_the_audio_tracks_with_the_current_one_checked() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Tracks(vec![
            track(TrackKind::Audio, 1, "fre", "eac3", true),
            track(TrackKind::Audio, 2, "eng", "aac", false),
            track(TrackKind::Subtitle, 1, "fre", "ass", false),
        ])]);
        f.session.update(Instant::now());
        assert_eq!(
            f.session.audio_choices(),
            [
                Choice {
                    id: Some(1),
                    label: "Français · E-AC-3".into(),
                    current: true
                },
                Choice {
                    id: Some(2),
                    label: "Anglais · AAC".into(),
                    current: false
                },
            ]
        );
    }

    #[test]
    fn the_panel_lists_none_then_the_subtitle_tracks() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Tracks(vec![
            track(TrackKind::Audio, 1, "fre", "eac3", true),
            track(TrackKind::Subtitle, 1, "fre", "ass", false),
            track(TrackKind::Subtitle, 2, "eng", "subrip", false),
        ])]);
        f.session.update(Instant::now());
        let choices = f.session.subtitle_choices();
        let lines: Vec<(Option<i64>, &str, bool)> = choices
            .iter()
            .map(|c| (c.id, c.label.as_str(), c.current))
            .collect();
        assert_eq!(
            lines,
            [
                (None, "Aucun", true),
                (Some(1), "Français · ASS", false),
                (Some(2), "Anglais · SRT", false),
            ]
        );
    }

    #[test]
    fn none_is_not_checked_when_a_subtitle_track_shows() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Tracks(vec![track(
            TrackKind::Subtitle,
            1,
            "fre",
            "ass",
            true,
        )])]);
        f.session.update(Instant::now());
        let current: Vec<Option<i64>> = f
            .session
            .subtitle_choices()
            .iter()
            .filter(|c| c.current)
            .map(|c| c.id)
            .collect();
        assert_eq!(current, [Some(1)]);
    }

    #[test]
    fn choosing_a_track_in_the_panel_asks_the_player() {
        let mut f = fixture();
        f.session.select_audio(2);
        f.session.select_subtitle(None);
        f.session.select_subtitle(Some(1));
        assert_eq!(
            f.calls(),
            [
                Call::Audio(Some(2)),
                Call::Subtitle(None),
                Call::Subtitle(Some(1))
            ]
        );
    }

    #[test]
    fn the_end_of_the_film_is_told_to_the_view() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::EndOfFile]);
        assert!(f.session.update(Instant::now()));
    }

    #[test]
    fn the_subtitles_rise_while_the_controls_show_and_come_back_when_they_hide() {
        let mut f = fixture();
        let start = Instant::now();
        f.session.layout((1440., 810.), whole(1440., 810.), 1.);
        f.session.update(start);
        f.session.mouse_moved(start);
        f.session.update(start);
        f.session.update(start + Duration::from_secs(1));
        f.session.update(start + Duration::from_secs(3));
        assert_eq!(
            f.calls(),
            [Call::SubtitlePosition(85.), Call::SubtitlePosition(100.)]
        );
    }

    #[test]
    fn shown_subtitles_follow_a_window_size_change() {
        let mut f = fixture();
        let start = Instant::now();
        f.session.layout((1440., 810.), whole(1440., 810.), 1.);
        f.session.mouse_moved(start);
        f.session.update(start);
        f.session.layout((1920., 1080.), whole(1920., 1080.), 1.);
        assert_eq!(
            f.calls(),
            [Call::SubtitlePosition(85.), Call::SubtitlePosition(89.)]
        );
    }

    #[test]
    fn the_end_of_the_film_is_not_lost_when_the_mouse_moves() {
        // A mouse move updates the session at once and drops the answer: the next tick must
        // still see the end.
        let mut f = fixture();
        let start = Instant::now();
        f.queue(vec![PlayerEvent::EndOfFile]);
        f.session.mouse_moved(start);
        let _ = f.session.update(start);
        assert!(f.session.update(start + Duration::from_millis(100)));
    }

    #[test]
    fn a_playback_error_is_kept_for_the_view() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Error(PlayerError(
            "loading failed".into(),
        ))]);
        f.session.update(Instant::now());
        assert_eq!(
            f.session.error().map(|e| e.0.as_str()),
            Some("loading failed")
        );
    }

    /// An episode of 47 minutes: a recap, an intro, then the credits.
    fn with_segments(f: &mut Fixture, has_next: bool) {
        use bobine::provider::SegmentKind::{Intro, Outro, Recap};
        let segment = |kind, start: u64, end: u64| Segment {
            kind,
            start: Duration::from_secs(start),
            end: Duration::from_secs(end),
        };
        f.session.set_segments(vec![
            segment(Recap, 0, 40),
            segment(Intro, 40, 130),
            segment(Outro, 2700, 2820),
        ]);
        f.session.set_next_episode(has_next);
        f.queue(vec![PlayerEvent::Duration(2820.)]);
    }

    fn at(f: &mut Fixture, now: Instant, position: f64) {
        f.queue(vec![PlayerEvent::Position(position)]);
        f.session.update(now);
    }

    fn seeks(f: &Fixture) -> Vec<Call> {
        f.calls()
            .into_iter()
            .filter(|call| matches!(call, Call::Seek(_)))
            .collect()
    }

    #[test]
    fn the_button_of_an_intro_shows_at_its_start_and_a_press_seeks_to_its_end() {
        let mut f = fixture();
        with_segments(&mut f, true);
        let start = Instant::now();
        at(&mut f, start, 20.);
        assert_eq!(f.session.skip_button().unwrap().label.text(), "Skip recap");
        at(&mut f, start, 60.);
        assert_eq!(f.session.skip_button().unwrap().label.text(), "Skip intro");
        assert_eq!(f.session.press_skip(), None);
        assert_eq!(seeks(&f), [Call::Seek(Seek::To(130.))]);
        assert_eq!(f.session.seeks(), 1);
        // Gone at once, before mpv says where it is now.
        assert_eq!(f.session.skip_button(), None);
        assert_eq!(f.session.press_skip(), None);
        assert_eq!(seeks(&f).len(), 1);
    }

    #[test]
    fn enter_presses_the_button_while_it_shows() {
        let mut f = fixture();
        with_segments(&mut f, true);
        at(&mut f, Instant::now(), 60.);
        assert_eq!(f.session.key("enter", false, false), None);
        assert_eq!(seeks(&f), [Call::Seek(Seek::To(130.))]);
    }

    #[test]
    fn enter_does_nothing_without_a_button() {
        let mut f = fixture();
        with_segments(&mut f, true);
        at(&mut f, Instant::now(), 1200.);
        assert_eq!(f.session.key("enter", false, false), None);
        assert_eq!(seeks(&f), []);
    }

    #[test]
    fn enter_does_nothing_once_the_button_has_faded() {
        let mut f = fixture();
        with_segments(&mut f, true);
        let start = Instant::now();
        at(&mut f, start, 60.);
        at(&mut f, start + Duration::from_secs(6), 66.);
        assert_eq!(f.session.skip_button(), None);
        assert_eq!(f.session.key("enter", false, false), None);
        assert_eq!(seeks(&f), []);
    }

    #[test]
    fn a_pointer_move_brings_the_faded_button_back() {
        let mut f = fixture();
        with_segments(&mut f, true);
        let start = Instant::now();
        at(&mut f, start, 60.);
        let later = start + Duration::from_secs(8);
        at(&mut f, later, 68.);
        assert_eq!(f.session.skip_button(), None);
        f.session.mouse_moved(later);
        f.session.update(later);
        assert!(f.session.skip_button().is_some());
    }

    #[test]
    fn no_button_shows_before_mpv_says_where_the_film_is() {
        // A resume far in: the position is the 0 it starts at, inside the recap.
        let mut f = fixture();
        with_segments(&mut f, true);
        f.session.update(Instant::now());
        assert_eq!(f.session.skip_button(), None);
    }

    #[test]
    fn a_film_without_segments_never_shows_a_button() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Duration(2820.)]);
        let start = Instant::now();
        for position in [0., 60., 2750.] {
            at(&mut f, start, position);
            assert_eq!(f.session.skip_button(), None);
        }
        assert_eq!(f.session.key("enter", false, false), None);
    }

    #[test]
    fn skipping_the_credits_of_a_last_episode_ends_the_playback_at_its_end() {
        let mut f = fixture();
        with_segments(&mut f, false);
        let start = Instant::now();
        at(&mut f, start, 2750.);
        assert_eq!(
            f.session.skip_button().unwrap().label.text(),
            "Skip credits"
        );
        assert_eq!(f.session.press_skip(), Some(WindowRequest::EndPlayback));
        // What mpv says before the end comes does not move the position back.
        f.queue(vec![PlayerEvent::Position(2751.)]);
        assert!(f.session.update(start + Duration::from_millis(100)));
        assert_eq!(f.session.position, 2820.);
        assert_eq!(seeks(&f), []);
    }

    #[test]
    fn skipping_the_credits_of_an_episode_with_a_next_one_ends_it_and_asks_the_next() {
        let mut f = fixture();
        with_segments(&mut f, true);
        let start = Instant::now();
        at(&mut f, start, 2750.);
        assert_eq!(
            f.session.skip_button().unwrap().label.text(),
            "Next episode"
        );
        assert_eq!(
            f.session.key("enter", false, false),
            Some(WindowRequest::NextEpisode)
        );
        assert!(f.session.update(start + Duration::from_millis(100)));
        assert_eq!(f.session.position, 2820.);
    }

    #[test]
    fn skipping_credits_followed_by_a_scene_seeks_and_does_not_end_the_film() {
        use bobine::provider::SegmentKind::Outro;
        let mut f = fixture();
        f.session.set_segments(vec![Segment {
            kind: Outro,
            start: Duration::from_secs(2700),
            end: Duration::from_secs(2760),
        }]);
        f.session.set_next_episode(true);
        f.queue(vec![PlayerEvent::Duration(2820.)]);
        let start = Instant::now();
        at(&mut f, start, 2710.);
        assert_eq!(
            f.session.skip_button().unwrap().label.text(),
            "Skip credits"
        );
        assert_eq!(f.session.press_skip(), None);
        assert_eq!(seeks(&f), [Call::Seek(Seek::To(2760.))]);
        assert!(!f.session.update(start + Duration::from_millis(100)));
    }

    #[test]
    fn a_stale_position_does_not_bring_the_pressed_button_back() {
        let mut f = fixture();
        with_segments(&mut f, true);
        let start = Instant::now();
        at(&mut f, start, 60.);
        f.session.press_skip();
        // mpv still says where the film was before the seek.
        at(&mut f, start + Duration::from_millis(100), 61.);
        assert_eq!(f.session.skip_button(), None);
        // Past the intro, then in the credits: the next button shows as usual.
        at(&mut f, start + Duration::from_millis(200), 131.);
        at(&mut f, start + Duration::from_secs(1), 2701.);
        assert!(f.session.skip_button().is_some());
    }
}
