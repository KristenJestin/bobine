//! One film being played: the player and the video surface, and what the controls show. The view
//! draws this and forwards the keys, the mouse and the layout to it; the tests drive it with a
//! fake player, a fake display and their own clock.

use std::time::{Duration, Instant};

use bobine::provider::Segment;

use super::controls::{Controls, KeyAction, on_picture, subtitle_position};
use super::segments::{Button, Label, Press, Prompt, button_at};
use super::settings::{self, Page, Quality, SPEEDS};
use super::{
    Aspect, Media, PlaybackInfo, Player, PlayerError, PlayerEvent, Seek, Track, TrackKind, time,
    tracks,
};
use crate::video::{Rect, VideoDisplay, placement};

/// What a press of the volume keys changes, in points of 100.
pub const VOLUME_STEP: f64 = 5.;

/// How often "Playback info" is read from the player.
const INFO_EVERY: Duration = Duration::from_secs(1);

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
    /// What comes next plays at once, this one left where it is.
    PlayNext,
    /// The viewer chose another quality: the server is asked for it.
    Quality(Quality),
}

/// One line of the audio and subtitles panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// `None`: the "None" line of the subtitles.
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
    /// The settings menu, and its page: kept once it closes, as it fades out.
    settings_open: bool,
    settings_page: Page,
    speed: f64,
    /// 0 to 100. Each film has an mpv of its own: the window carries it from one to the next.
    volume: f64,
    muted: bool,
    /// In tenths of a second.
    subtitle_delay: i32,
    aspect: Aspect,
    quality: Quality,
    /// The server can convert the film: a quality can be chosen.
    quality_offered: bool,
    /// The server converts what plays.
    converted: bool,
    /// The file did not play as it is: "Original" is the server's conversion of it.
    direct_failed: bool,
    pub position: f64,
    /// mpv sent a `time-pos`: before it, `position` is the 0 it starts at, not where the film is.
    position_known: bool,
    pub duration: f64,
    pub paused: bool,
    tracks: Vec<Track>,
    /// What "Playback info" shows, and when it was read: mpv is asked once a second, not at
    /// every frame drawn.
    info: PlaybackInfo,
    info_read_at: Option<Instant>,
    error: Option<PlayerError>,
    window_height: f32,
    /// Whether the controls showed at the last update, and mpv's `sub-pos` for it.
    controls_shown: bool,
    subtitle_position: f64,
    /// Kept once seen: an update whose answer is dropped (a mouse move) must not lose it.
    ended: bool,
    /// The file is open and playing: the server may be told.
    loaded: bool,
    /// When the player was first heard of for this file, when it said `Loaded`, and when its
    /// first picture came.
    opened_at: Option<Instant>,
    loaded_at: Option<Instant>,
    first_frame_at: Option<Instant>,
    /// The stream waits for data to fill its cache.
    buffering: bool,
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
            settings_open: false,
            settings_page: Page::Main,
            speed: 1.,
            volume: 100.,
            muted: false,
            subtitle_delay: 0,
            aspect: Aspect::Auto,
            quality: Quality::Original,
            quality_offered: false,
            converted: false,
            direct_failed: false,
            position: 0.,
            position_known: false,
            duration: 0.,
            paused: false,
            tracks: Vec::new(),
            info: PlaybackInfo::default(),
            info_read_at: None,
            error: None,
            window_height: 0.,
            controls_shown: false,
            subtitle_position: 100.,
            ended: false,
            loaded: false,
            opened_at: None,
            loaded_at: None,
            first_frame_at: None,
            buffering: false,
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
        self.opened_at = None;
        self.loaded_at = None;
        self.first_frame_at = None;
        self.buffering = false;
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

    /// From the first update after the file was given to the player: a file that never says
    /// `Loaded` nor fails is waited for from here.
    pub fn opened_at(&self) -> Option<Instant> {
        self.opened_at
    }

    /// When the player said the file was open: the wait for its first picture counts from here.
    pub fn loaded_at(&self) -> Option<Instant> {
        self.loaded_at
    }

    pub fn first_frame_at(&self) -> Option<Instant> {
        self.first_frame_at
    }

    pub fn answers_slowly(&self) -> bool {
        self.player.answers_slowly()
    }

    /// A frame of the film is on the window, as the display says: asking it waits for nothing,
    /// unlike asking the player.
    pub fn picture_shown(&self) -> bool {
        self.display.picture_shown()
    }

    /// What the server marked in this film or episode, by start.
    pub fn set_segments(&mut self, segments: Vec<Segment>) {
        self.segments = segments;
    }

    /// Whether an episode follows this one: its credits offer it.
    pub fn set_next_episode(&mut self, has_next: bool) {
        self.has_next = has_next;
    }

    /// Where the segments sit on the seek bar.
    pub fn segment_marks(&self) -> Vec<(f32, f32)> {
        super::segments::marks(&self.segments, self.duration)
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
    pub fn key(&mut self, action: KeyAction, fullscreen: bool) -> Option<WindowRequest> {
        // Escape closes the panel or the menu before it leaves fullscreen or the player.
        if action == KeyAction::Escape && (self.panel_open || self.settings_open) {
            self.clicked_outside_panel();
            return None;
        }
        match action {
            KeyAction::TogglePause => self.toggle_pause(),
            KeyAction::SeekBy(seconds) => self.skip(seconds),
            KeyAction::Volume(points) => {
                self.volume = (self.volume + points).clamp(0., 100.);
                self.player.set_volume(self.volume);
            }
            KeyAction::ToggleMute => {
                self.muted = !self.muted;
                self.player.set_muted(self.muted);
            }
            KeyAction::ToggleFullscreen => return Some(WindowRequest::ToggleFullscreen),
            KeyAction::CycleSubtitles => {
                if let Some(id) = following(&self.subtitle_choices()) {
                    self.select_subtitle(id);
                }
            }
            KeyAction::CycleAudio => {
                if let Some(Some(id)) = following(&self.audio_choices()) {
                    self.select_audio(id);
                }
            }
            KeyAction::Next => return Some(WindowRequest::PlayNext),
            KeyAction::PressSkip => return self.press_skip(),
            KeyAction::ShiftSubtitles(tenths) => self.shift_subtitles(tenths),
            KeyAction::Speed(steps) => {
                let at = SPEEDS
                    .iter()
                    .position(|speed| *speed >= self.speed)
                    .unwrap_or(SPEEDS.len() - 1);
                let to = at
                    .saturating_add_signed(steps as isize)
                    .min(SPEEDS.len() - 1);
                self.set_speed(SPEEDS[to]);
            }
            KeyAction::Escape if fullscreen => return Some(WindowRequest::ExitFullscreen),
            KeyAction::Escape => return Some(WindowRequest::Leave),
        }
        None
    }

    /// The volume and the mute the film before was left at: each film has an mpv of its own,
    /// which starts loud and unmuted.
    pub fn carry_volume(&mut self, volume: f64, muted: bool) {
        self.volume = volume;
        self.muted = muted;
        self.player.set_volume(volume);
        self.player.set_muted(muted);
    }

    pub fn volume(&self) -> (f64, bool) {
        (self.volume, self.muted)
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

    /// Seconds from the start.
    pub fn seek_to(&mut self, seconds: f64) {
        self.seeks += 1;
        self.player.seek(Seek::To(seconds));
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

    /// The controls stay while the panel or the menu is open: they are reached through them.
    pub fn controls_visible(&self, now: Instant) -> bool {
        self.panel_open || self.settings_open || self.controls.visible(now)
    }

    pub fn panel_open(&self) -> bool {
        self.panel_open
    }

    /// The panel and the settings menu never show together.
    pub fn toggle_panel(&mut self) {
        self.panel_open = !self.panel_open;
        self.settings_open = false;
    }

    /// A click anywhere but in the panel or the menu closes it, the button that opened it
    /// included.
    pub fn clicked_outside_panel(&mut self) {
        self.panel_open = false;
        self.settings_open = false;
    }

    /// The gear: the menu opens on its list of settings, or closes.
    pub fn toggle_settings(&mut self) {
        self.settings_open = !self.settings_open;
        if self.settings_open {
            self.settings_page = Page::Main;
            self.panel_open = false;
        }
    }

    pub fn settings_open(&self) -> bool {
        self.settings_open
    }

    /// The page shown, or last shown while the menu fades out.
    pub fn settings_page(&self) -> Page {
        self.settings_page
    }

    pub fn show_settings_page(&mut self, page: Page) {
        self.settings_page = page;
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn set_speed(&mut self, speed: f64) {
        self.speed = speed;
        self.player.set_speed(speed);
    }

    /// In tenths of a second.
    pub fn subtitle_delay(&self) -> i32 {
        self.subtitle_delay
    }

    /// Later by `tenths` of a second, earlier when negative.
    pub fn shift_subtitles(&mut self, tenths: i32) {
        self.delay_subtitles(self.subtitle_delay + tenths);
    }

    pub fn reset_subtitle_delay(&mut self) {
        self.delay_subtitles(0);
    }

    fn delay_subtitles(&mut self, tenths: i32) {
        self.subtitle_delay = tenths;
        self.player.set_subtitle_delay(f64::from(tenths) / 10.);
    }

    pub fn aspect(&self) -> Aspect {
        self.aspect
    }

    pub fn set_aspect(&mut self, aspect: Aspect) {
        self.aspect = aspect;
        self.player.set_aspect(aspect);
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// The film comes from a server that converts it: its quality can be chosen.
    pub fn offer_quality(&mut self) {
        self.quality_offered = true;
    }

    pub fn quality_offered(&self) -> bool {
        self.quality_offered
    }

    /// The viewer chose `quality`: the window asks the server for it, unless it is what plays.
    pub fn choose_quality(&mut self, quality: Quality) -> Option<WindowRequest> {
        (self.quality_offered && quality != self.quality).then_some(WindowRequest::Quality(quality))
    }

    /// What plays now: `quality`, converted by the server or not.
    pub fn playing_as(&mut self, quality: Quality, converted: bool) {
        self.quality = quality;
        self.converted = converted;
    }

    /// The file did not play as it is: the server converts it whatever the quality.
    pub fn direct_play_failed(&mut self) {
        self.direct_failed = true;
    }

    /// How the quality page names `quality`.
    pub fn quality_label(&self, quality: Quality) -> String {
        quality.label(!self.direct_failed)
    }

    /// The lines of "Playback info".
    pub fn playback_info(&self) -> Vec<(&'static str, String)> {
        settings::info_lines(self.converted, &self.info)
    }

    /// A double click `y` pixels from the top of the window: on the picture, it toggles
    /// fullscreen.
    pub fn double_clicked(&self, y: f32) -> Option<WindowRequest> {
        on_picture(y, self.window_height, self.controls_shown)
            .then_some(WindowRequest::ToggleFullscreen)
    }

    /// Which line of the panel to scroll into view as it opens: the subtitle shown, as the
    /// panel lays them out (the audio title, the audio tracks, the subtitles title, "None",
    /// then the subtitle tracks).
    pub fn chosen_line(&self) -> usize {
        let audio = self.tracks_of(TrackKind::Audio).count();
        let shown = self
            .subtitle_choices()
            .iter()
            .position(|choice| choice.current)
            .unwrap_or(0);
        1 + audio + 1 + shown
    }

    /// The fullscreen button of the controls.
    pub fn press_fullscreen(&self) -> Option<WindowRequest> {
        Some(WindowRequest::ToggleFullscreen)
    }

    pub fn audio_choices(&self) -> Vec<Choice> {
        self.tracks_of(TrackKind::Audio).collect()
    }

    /// "None" first, then the subtitle tracks.
    pub fn subtitle_choices(&self) -> Vec<Choice> {
        let tracks: Vec<Choice> = self.tracks_of(TrackKind::Subtitle).collect();
        let none = Choice {
            id: None,
            label: "None".into(),
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

    /// What mpv lists, as it said last.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    pub fn select_audio(&mut self, id: i64) {
        self.player.select_audio(Some(id));
        self.mark_selected(TrackKind::Audio, Some(id));
    }

    pub fn select_subtitle(&mut self, id: Option<i64>) {
        self.player.select_subtitle(id);
        self.mark_selected(TrackKind::Subtitle, id);
    }

    /// Shown at once, so that a key pressed again goes on from it; mpv's own word follows.
    fn mark_selected(&mut self, kind: TrackKind, id: Option<i64>) {
        for track in self.tracks.iter_mut().filter(|track| track.kind == kind) {
            track.selected = Some(track.id) == id;
        }
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

    /// The view shows that it waits: from the title asked to its first frame, and while the stream
    /// fills its cache.
    pub fn loading(&self) -> bool {
        self.error.is_none()
            && ((self.first_frame_at.is_none() && !self.picture_shown()) || self.buffering)
    }

    /// What is left and when the film ends, for the clock now; nothing before the duration is
    /// known.
    pub fn time_left(&self, now: (u32, u32)) -> Option<String> {
        (self.duration > 0.).then(|| {
            let left = (self.duration - self.position).max(0.);
            format!("{} · {}", time::remaining(left), time::end_clock(now, left))
        })
    }

    /// Reads what the player says and shows or hides the cursor with the controls. Returns
    /// whether the film reached its end, now or at an earlier update.
    pub fn update(&mut self, now: Instant) -> bool {
        self.opened_at.get_or_insert(now);
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
                PlayerEvent::Buffering(waiting) => self.buffering = waiting,
                // The film plays; a refused subtitle is simply not in the panel.
                PlayerEvent::SubtitleFailed(_) => {}
            }
        }
        if self
            .info_read_at
            .is_none_or(|read| now.saturating_duration_since(read) >= INFO_EVERY)
        {
            self.info = self.player.info();
            self.info_read_at = Some(now);
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

/// The choice after the current one, back to the first after the last.
fn following(choices: &[Choice]) -> Option<Option<i64>> {
    let current = choices.iter().position(|choice| choice.current);
    let next = current.map_or(0, |at| (at + 1) % choices.len());
    choices.get(next).map(|choice| choice.id)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::player::fake::{Call, FakePlayer};
    use crate::player::{FirstSubtitle, Stream};
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
                info: player.info.clone(),
                ..Default::default()
            }),
            Box::new(FakeDisplay {
                placements: display.placements.clone(),
                cursor: display.cursor.clone(),
                shown: display.shown.clone(),
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
        assert_eq!(f.session.key(KeyAction::TogglePause, false), None);
        assert!(f.session.paused);
        f.session.key(KeyAction::TogglePause, false);
        assert!(!f.session.paused);
        assert_eq!(f.calls(), [Call::Paused(true), Call::Paused(false)]);
    }

    #[test]
    fn the_arrows_skip_ten_seconds() {
        let mut f = fixture();
        f.session.key(KeyAction::SeekBy(-10.), false);
        f.session.key(KeyAction::SeekBy(10.), false);
        assert_eq!(
            f.calls(),
            [Call::Seek(Seek::By(-10.)), Call::Seek(Seek::By(10.))]
        );
    }

    #[test]
    fn f_asks_the_window_for_fullscreen() {
        let mut f = fixture();
        assert_eq!(
            f.session.key(KeyAction::ToggleFullscreen, false),
            Some(WindowRequest::ToggleFullscreen)
        );
    }

    #[test]
    fn escape_leaves_fullscreen_then_the_player() {
        let mut f = fixture();
        assert_eq!(
            f.session.key(KeyAction::Escape, true),
            Some(WindowRequest::ExitFullscreen)
        );
        assert_eq!(
            f.session.key(KeyAction::Escape, false),
            Some(WindowRequest::Leave)
        );
    }

    #[test]
    fn the_volume_keys_step_by_five_between_silence_and_full() {
        let mut f = fixture();
        f.session.key(KeyAction::Volume(5.), false);
        f.session.key(KeyAction::Volume(-5.), false);
        f.session.key(KeyAction::Volume(-5.), false);
        assert_eq!(
            f.calls(),
            [Call::Volume(100.), Call::Volume(95.), Call::Volume(90.)]
        );
        for _ in 0..30 {
            f.session.key(KeyAction::Volume(-5.), false);
        }
        assert_eq!(f.calls().last(), Some(&Call::Volume(0.)));
    }

    #[test]
    fn the_volume_is_kept_from_one_session_to_the_next() {
        let mut f = fixture();
        f.session.carry_volume(40., true);
        f.session.key(KeyAction::Volume(5.), false);
        assert_eq!(f.session.volume(), (45., true));
        assert_eq!(
            f.calls(),
            [Call::Volume(40.), Call::Muted(true), Call::Volume(45.)]
        );
    }

    #[test]
    fn m_mutes_and_unmutes() {
        let mut f = fixture();
        f.session.key(KeyAction::ToggleMute, false);
        f.session.key(KeyAction::ToggleMute, false);
        assert_eq!(f.calls(), [Call::Muted(true), Call::Muted(false)]);
    }

    #[test]
    fn c_cycles_the_subtitles_through_none() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Tracks(vec![
            track(TrackKind::Subtitle, 1, "fre", "subrip", false),
            track(TrackKind::Subtitle, 2, "eng", "ass", false),
        ])]);
        f.session.update(Instant::now());
        for _ in 0..3 {
            f.session.key(KeyAction::CycleSubtitles, false);
        }
        assert_eq!(
            f.calls(),
            [
                Call::Subtitle(Some(1)),
                Call::Subtitle(Some(2)),
                Call::Subtitle(None)
            ]
        );
    }

    #[test]
    fn a_cycles_the_audio_tracks() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Tracks(vec![
            track(TrackKind::Audio, 1, "fre", "eac3", true),
            track(TrackKind::Audio, 2, "eng", "aac", false),
        ])]);
        f.session.update(Instant::now());
        f.session.key(KeyAction::CycleAudio, false);
        f.session.key(KeyAction::CycleAudio, false);
        assert_eq!(f.calls(), [Call::Audio(Some(2)), Call::Audio(Some(1))]);
    }

    #[test]
    fn the_brackets_move_the_subtitles_by_a_tenth() {
        let mut f = fixture();
        f.session.key(KeyAction::ShiftSubtitles(1), false);
        f.session.key(KeyAction::ShiftSubtitles(-1), false);
        f.session.key(KeyAction::ShiftSubtitles(-1), false);
        assert_eq!(
            f.calls(),
            [
                Call::SubtitleDelay(0.1),
                Call::SubtitleDelay(0.),
                Call::SubtitleDelay(-0.1)
            ]
        );
    }

    #[test]
    fn the_angle_brackets_step_through_the_speeds_and_stop_at_the_ends() {
        let mut f = fixture();
        f.session.key(KeyAction::Speed(1), false);
        f.session.key(KeyAction::Speed(-1), false);
        f.session.key(KeyAction::Speed(-1), false);
        f.session.key(KeyAction::Speed(-1), false);
        f.session.key(KeyAction::Speed(-1), false);
        assert_eq!(
            f.calls(),
            [
                Call::Speed(1.25),
                Call::Speed(1.),
                Call::Speed(0.75),
                Call::Speed(0.5),
                Call::Speed(0.5)
            ]
        );
    }

    #[test]
    fn n_asks_the_window_for_what_comes_next() {
        let mut f = fixture();
        assert_eq!(
            f.session.key(KeyAction::Next, false),
            Some(WindowRequest::PlayNext)
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
        assert_eq!(f.session.key(KeyAction::Escape, true), None);
        assert!(!f.session.panel_open());
        assert_eq!(
            f.session.key(KeyAction::Escape, true),
            Some(WindowRequest::ExitFullscreen)
        );
    }

    #[test]
    fn the_fullscreen_button_toggles_fullscreen() {
        let f = fixture();
        assert_eq!(
            f.session.press_fullscreen(),
            Some(WindowRequest::ToggleFullscreen)
        );
    }

    #[test]
    fn a_double_click_on_the_picture_toggles_fullscreen() {
        let mut f = fixture();
        let start = Instant::now();
        f.session.layout((1440., 810.), whole(1440., 810.), 1.);
        f.session.update(start);
        assert_eq!(
            f.session.double_clicked(750.),
            Some(WindowRequest::ToggleFullscreen)
        );
        f.session.mouse_moved(start);
        f.session.update(start);
        assert_eq!(
            f.session.double_clicked(400.),
            Some(WindowRequest::ToggleFullscreen)
        );
        assert_eq!(f.session.double_clicked(750.), None, "on the controls");
    }

    #[test]
    fn a_long_track_list_keeps_the_chosen_track_in_view() {
        let mut f = fixture();
        let mut tracks = vec![
            track(TrackKind::Audio, 1, "fre", "eac3", true),
            track(TrackKind::Audio, 2, "eng", "aac", false),
        ];
        tracks.extend((1..=40).map(|id| track(TrackKind::Subtitle, id, "eng", "ass", id == 30)));
        f.queue(vec![PlayerEvent::Tracks(tracks)]);
        f.session.update(Instant::now());
        // AUDIO, 2 tracks, SUBTITLES, None, then subtitle 30 is the 30th.
        assert_eq!(f.session.chosen_line(), 1 + 2 + 1 + 30);
    }

    #[test]
    fn with_no_subtitle_shown_none_is_kept_in_view() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Tracks(vec![
            track(TrackKind::Audio, 1, "fre", "eac3", true),
            track(TrackKind::Subtitle, 1, "fre", "ass", false),
        ])]);
        f.session.update(Instant::now());
        assert_eq!(f.session.chosen_line(), 1 + 1 + 1);
    }

    #[test]
    fn the_settings_menu_opens_on_its_list_and_keeps_the_controls_shown() {
        let mut f = fixture();
        let start = Instant::now();
        f.session.show_settings_page(Page::Info);
        f.session.toggle_settings();
        assert!(f.session.settings_open());
        assert_eq!(f.session.settings_page(), Page::Main);
        assert!(f.session.controls_visible(start + Duration::from_secs(10)));
        f.session.toggle_settings();
        assert!(!f.session.settings_open());
    }

    #[test]
    fn the_settings_menu_and_the_track_panel_never_show_together() {
        let mut f = fixture();
        f.session.toggle_panel();
        f.session.toggle_settings();
        assert!(f.session.settings_open() && !f.session.panel_open());
        f.session.toggle_panel();
        assert!(!f.session.settings_open() && f.session.panel_open());
    }

    #[test]
    fn escape_or_a_click_outside_closes_the_settings_menu() {
        let mut f = fixture();
        f.session.toggle_settings();
        assert_eq!(f.session.key(KeyAction::Escape, true), None);
        assert!(!f.session.settings_open());
        f.session.toggle_settings();
        f.session.clicked_outside_panel();
        assert!(!f.session.settings_open());
    }

    #[test]
    fn the_speed_setting_changes_the_player_speed() {
        let mut f = fixture();
        f.session.set_speed(1.5);
        assert_eq!(f.session.speed(), 1.5);
        assert_eq!(f.calls(), [Call::Speed(1.5)]);
    }

    #[test]
    fn the_subtitle_offset_moves_by_tenths_and_resets() {
        let mut f = fixture();
        f.session.shift_subtitles(1);
        f.session.shift_subtitles(1);
        f.session.shift_subtitles(1);
        f.session.shift_subtitles(-1);
        assert_eq!(f.session.subtitle_delay(), 2);
        f.session.reset_subtitle_delay();
        assert_eq!(
            f.calls(),
            [
                Call::SubtitleDelay(0.1),
                Call::SubtitleDelay(0.2),
                Call::SubtitleDelay(0.3),
                Call::SubtitleDelay(0.2),
                Call::SubtitleDelay(0.),
            ]
        );
        assert_eq!(f.session.subtitle_delay(), 0);
    }

    #[test]
    fn the_aspect_setting_changes_the_player_aspect() {
        let mut f = fixture();
        f.session.set_aspect(Aspect::Fill);
        f.session.set_aspect(Aspect::Scope);
        assert_eq!(f.session.aspect(), Aspect::Scope);
        assert_eq!(
            f.calls(),
            [Call::Aspect(Aspect::Fill), Call::Aspect(Aspect::Scope)]
        );
    }

    #[test]
    fn a_quality_cap_asks_the_window_for_it_and_back_to_original() {
        let mut f = fixture();
        f.session.offer_quality();
        let cap = Quality::Cap(4_000_000);
        assert_eq!(
            f.session.choose_quality(cap),
            Some(WindowRequest::Quality(cap))
        );
        f.session.playing_as(cap, true);
        assert_eq!(f.session.choose_quality(cap), None, "already playing");
        assert_eq!(
            f.session.choose_quality(Quality::Original),
            Some(WindowRequest::Quality(Quality::Original))
        );
    }

    #[test]
    fn a_film_the_server_cannot_convert_offers_no_quality() {
        let mut f = fixture();
        assert_eq!(f.session.choose_quality(Quality::Cap(4_000_000)), None);
    }

    #[test]
    fn playback_info_shows_the_method_and_the_codecs() {
        let f = fixture();
        *f.player.info.borrow_mut() = crate::player::PlaybackInfo {
            container: Some("matroska,webm".into()),
            video_codec: Some("hevc".into()),
            audio_codec: Some("eac3".into()),
            size: Some((3840, 2160)),
            video_bitrate: Some(18_460_000.),
            dropped_frames: Some(0),
            hardware_decoder: Some("vaapi".into()),
        };
        let mut session = f.session;
        session.update(Instant::now());
        assert_eq!(
            session.playback_info(),
            [
                ("Playback", "Direct play".to_string()),
                ("Container", "MKV".into()),
                ("Video", "HEVC".into()),
                ("Audio", "E-AC-3".into()),
                ("Resolution", "3840 × 2160".into()),
                ("Bitrate", "18.5 Mbps".into()),
                ("Dropped frames", "0".into()),
                ("Hardware decoding", "vaapi".into()),
            ]
        );
        session.playing_as(Quality::Cap(4_000_000), true);
        assert_eq!(
            session.playback_info()[0],
            ("Playback", "Converted by the server".to_string())
        );
    }

    #[test]
    fn playback_info_is_read_once_a_second_and_kept() {
        let mut f = fixture();
        let container = |name: &str| crate::player::PlaybackInfo {
            container: Some(name.into()),
            ..Default::default()
        };
        let shown = |session: &Session| session.playback_info()[1].1.clone();
        *f.player.info.borrow_mut() = container("matroska,webm");
        let now = Instant::now();
        f.session.update(now);
        *f.player.info.borrow_mut() = container("mp4");
        assert_eq!(shown(&f.session), "MKV", "kept between ticks");
        f.session.update(now + Duration::from_millis(500));
        assert_eq!(shown(&f.session), "MKV", "read once a second");
        f.session.update(now + Duration::from_secs(1));
        assert_eq!(shown(&f.session), "MP4");
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
                    label: "French · E-AC-3".into(),
                    current: true
                },
                Choice {
                    id: Some(2),
                    label: "English · AAC".into(),
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
                (None, "None", true),
                (Some(1), "French · ASS", false),
                (Some(2), "English · SRT", false),
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

    #[test]
    fn a_title_asked_shows_loading_until_its_first_frame() {
        let mut f = fixture();
        assert!(f.session.loading(), "asked, nothing heard yet");
        f.queue(vec![PlayerEvent::Loaded]);
        f.session.update(Instant::now());
        assert!(f.session.loading(), "open, no picture yet");
        f.queue(vec![PlayerEvent::FirstFrame]);
        f.session.update(Instant::now());
        assert!(!f.session.loading());
    }

    #[test]
    fn a_picture_the_display_shows_drops_the_loading_mark_before_the_player_tells_it() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Loaded]);
        f.session.update(Instant::now());
        *f.display.shown.borrow_mut() = true;
        assert!(!f.session.loading(), "the display already shows a frame");
    }

    #[test]
    fn a_stream_waiting_for_its_cache_shows_loading_until_it_plays_again() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        f.session.update(Instant::now());
        assert!(!f.session.loading());
        f.queue(vec![PlayerEvent::Buffering(true)]);
        f.session.update(Instant::now());
        assert!(f.session.loading());
        f.queue(vec![PlayerEvent::Buffering(false)]);
        f.session.update(Instant::now());
        assert!(!f.session.loading());
    }

    #[test]
    fn a_restart_clears_the_cache_stall() {
        let mut f = fixture();
        f.queue(vec![
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Buffering(true),
        ]);
        f.session.update(Instant::now());
        assert!(f.session.loading(), "stalled on the cache");
        let media = Media {
            stream: Stream {
                url: "file:///converted.mkv",
                headers: &[],
            },
            start: 0.,
            audio: None,
            subtitle: FirstSubtitle::Default,
            external_subtitles: &[],
        };
        f.session.restart(media).unwrap();
        // The conversion shows its first picture without ever saying that the stall ended.
        f.queue(vec![PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        f.session.update(Instant::now());
        assert!(!f.session.loading(), "the old stall does not come along");
    }

    #[test]
    fn the_time_left_and_end_are_hidden_until_the_duration_is_known() {
        let mut f = fixture();
        f.queue(vec![PlayerEvent::Position(0.)]);
        f.session.update(Instant::now());
        assert_eq!(f.session.time_left((22, 0)), None);
        f.queue(vec![PlayerEvent::Duration(5400.)]);
        f.session.update(Instant::now());
        assert_eq!(
            f.session.time_left((22, 0)).as_deref(),
            Some("−1:30:00 · ends at 23:30")
        );
    }

    #[test]
    fn a_stream_that_fails_to_open_drops_the_loading_mark_for_the_error() {
        let mut f = fixture();
        assert!(f.session.loading());
        f.queue(vec![PlayerEvent::Error(PlayerError("404".into()))]);
        f.session.update(Instant::now());
        assert!(!f.session.loading());
        assert!(f.session.error().is_some());
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
        assert_eq!(f.session.key(KeyAction::PressSkip, false), None);
        assert_eq!(seeks(&f), [Call::Seek(Seek::To(130.))]);
    }

    #[test]
    fn enter_does_nothing_without_a_button() {
        let mut f = fixture();
        with_segments(&mut f, true);
        at(&mut f, Instant::now(), 1200.);
        assert_eq!(f.session.key(KeyAction::PressSkip, false), None);
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
        assert_eq!(f.session.key(KeyAction::PressSkip, false), None);
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
        assert_eq!(f.session.key(KeyAction::PressSkip, false), None);
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
            f.session.key(KeyAction::PressSkip, false),
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
