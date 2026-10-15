//! One film being played: the player and the video surface, and what the controls show. The view
//! draws this and forwards the keys, the mouse and the layout to it; the tests drive it with a
//! fake player, a fake display and their own clock.

use std::time::Instant;

use super::controls::{Controls, KeyAction, key_action, subtitle_position};
use super::{Player, PlayerError, PlayerEvent, Seek, Track, TrackKind, tracks};
use crate::video::{Rect, VideoDisplay, placement};

/// What the session asks of the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowRequest {
    ToggleFullscreen,
    ExitFullscreen,
    Leave,
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
    /// How many seeks the viewer asked for, so a seek can be told to the server.
    seeks: u64,
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
            duration: 0.,
            paused: false,
            tracks: Vec::new(),
            error: None,
            window_height: 0.,
            controls_shown: false,
            subtitle_position: 100.,
            ended: false,
            loaded: false,
            seeks: 0,
        }
    }

    /// A key pressed in the player; what the window must do, if anything.
    pub fn key(
        &mut self,
        key: &str,
        with_modifier: bool,
        fullscreen: bool,
    ) -> Option<WindowRequest> {
        match key_action(key, with_modifier, fullscreen)? {
            KeyAction::TogglePause => self.toggle_pause(),
            KeyAction::SeekBy(seconds) => self.skip(seconds),
            KeyAction::ToggleFullscreen => return Some(WindowRequest::ToggleFullscreen),
            KeyAction::ExitFullscreen => return Some(WindowRequest::ExitFullscreen),
            KeyAction::Leave => return Some(WindowRequest::Leave),
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

    pub fn mouse_moved(&mut self, now: Instant) {
        self.controls.mouse_moved(now);
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
                PlayerEvent::Position(seconds) => self.position = seconds,
                PlayerEvent::Duration(seconds) => self.duration = seconds,
                PlayerEvent::Paused(paused) => self.paused = paused,
                PlayerEvent::Tracks(tracks) => self.tracks = tracks,
                PlayerEvent::EndOfFile => self.ended = true,
                PlayerEvent::Error(error) => self.error = Some(error),
                PlayerEvent::Loaded => self.loaded = true,
                // The film plays; a refused subtitle is simply not in the panel.
                PlayerEvent::SubtitleFailed(_) => {}
            }
        }
        let shown = self.controls_visible(now);
        // Told every time: the display can only act once the pointer is over the window.
        self.display.set_cursor_visible(shown);
        self.controls_shown = shown;
        self.place_subtitles();
        self.ended
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
}
