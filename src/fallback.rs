//! Direct play first; when the file does not play, once, the server's conversion. A file does
//! not play when its load ends with an error, or when no picture comes within a few seconds of
//! its opening, or when it never opens within a longer limit. A file that showed a picture
//! plays: an error later is not one a conversion mends, and is shown as it is. A downloaded file
//! is never converted: it does not play, and that is said.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bobine::provider::{
    ItemId, MediaServer, MediaSourceId, PlayMethod, Playback, PlaybackReport, ProviderError,
    TranscodeRequest,
};

use crate::film_page::Plan;
use crate::player::PlayerError;
use crate::player::session::Session;
use crate::player::settings::Quality;

/// How long an open file may go without a picture: the seek to a resume point over a slow
/// network comes first.
pub const FIRST_PICTURE_WAIT: Duration = Duration::from_secs(20);

/// The same for a conversion: a slow server encodes its first segment after the resume point,
/// a 4K one to H.264 at that, before any picture comes.
pub const FIRST_CONVERTED_PICTURE_WAIT: Duration = Duration::from_secs(60);

/// How long a file may stay without saying it is open, from when it is given to the player:
/// a spinning disk or a remote link opens slowly, but not for ever.
pub const OPEN_WAIT: Duration = Duration::from_secs(60);

/// The same for a conversion: the server answers the playlist once it has started to encode, and
/// the verdict that follows is final (the film cannot play), so it is given more room.
pub const OPEN_CONVERTED_WAIT: Duration = Duration::from_secs(90);

/// What the player says of a downloaded file that does not play.
#[allow(
    dead_code,
    reason = "played from the shell once offline mode (#137) wires downloads in"
)]
pub const DOWNLOAD_CANNOT_PLAY: &str = "this download cannot be played";

/// How long the conversion notice stays once the converted picture shows.
pub const NOTICE_FOR: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Nothing,
    /// Ask the server to convert the version, from there.
    Convert {
        from: Duration,
    },
    /// The conversion shows no picture either: the film cannot play.
    CannotPlay,
}

/// What to do now about a playback sent by `method`, planned to start at `planned`.
fn step(method: PlayMethod, session: &Session, planned: Duration, now: Instant) -> Step {
    if session.first_frame_at().is_some() {
        return Step::Nothing;
    }
    let failed = session.error().is_some();
    let (picture_wait, open_wait) = match method {
        PlayMethod::DirectPlay => (FIRST_PICTURE_WAIT, OPEN_WAIT),
        PlayMethod::Transcode => (FIRST_CONVERTED_PICTURE_WAIT, OPEN_CONVERTED_WAIT),
    };
    // The picture is waited for from `Loaded`; a file that never says it, from its opening.
    let late = match (session.loaded_at(), session.opened_at()) {
        (Some(loaded), _) => now.duration_since(loaded) >= picture_wait,
        (None, Some(opened)) => now.duration_since(opened) >= open_wait,
        (None, None) => false,
    };
    match method {
        PlayMethod::DirectPlay if failed || late => Step::Convert {
            from: if session.position_known() {
                Duration::from_secs_f64(session.position.max(0.))
            } else {
                planned
            },
        },
        PlayMethod::Transcode if late && !failed => Step::CannotPlay,
        // Playing, still opening, or the error already shown.
        _ => Step::Nothing,
    }
}

/// Plays the conversion `opened` of `plan` (already [`Plan::converted`]) in place of the file.
fn restart(session: &mut Session, plan: &Plan, opened: &Playback) -> Result<(), PlayerError> {
    let externals = plan.externals(opened);
    session.restart(plan.media(opened, &externals))
}

/// The line under the title once the server converts: "direct play" becomes "converted
/// by the server", and back when the file plays as it is again.
fn said(subtitle: Option<String>, method: PlayMethod) -> Option<String> {
    let (from, to) = match method {
        PlayMethod::Transcode => ("direct play", "converted by the server"),
        PlayMethod::DirectPlay => ("converted by the server", "direct play"),
    };
    subtitle.map(|line| line.replace(from, to))
}

/// What the server is asked for a new playback of the film: its conversion, or the file as it
/// is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reopen {
    Convert(TranscodeRequest),
    Direct(ItemId, MediaSourceId),
}

impl Reopen {
    /// Off the window's thread: it blocks on the server.
    pub fn open(&self, server: &dyn MediaServer) -> Result<Playback, ProviderError> {
        match self {
            Self::Convert(request) => server.transcode(request),
            Self::Direct(film, source) => server.play(film, source),
        }
    }
}

/// Asks the server for the first playback of `plan`: its conversion under the cap of `quality`
/// at once, or the file as it is. A server that gives no conversion leaves the film playing as it
/// is, which the player's menu then says could not be had at that quality.
pub fn open_first(
    server: &dyn MediaServer,
    plan: &Plan,
    quality: Quality,
) -> Result<Playback, ProviderError> {
    if let Quality::Cap(bits) = quality {
        let request = TranscodeRequest {
            max_bitrate: Some(bits),
            ..plan.transcode_request(plan.start)
        };
        match server.transcode(&request) {
            Ok(opened) => return Ok(opened),
            Err(error) => eprintln!("bobine: the server does not give the quality asked: {error}"),
        }
    }
    server.play(&plan.film, &plan.source)
}

/// A film of the server, which the server may convert when it does not play as it is: once.
pub struct Fallback {
    pub server: Arc<dyn MediaServer>,
    pub plan: Plan,
    /// The plan of the file as it is, which every new playback starts from.
    original: Plan,
    method: PlayMethod,
    /// The server is being asked for a new playback from there.
    asking: Option<Duration>,
    /// What the viewer asked for, if they did, and how the film played before: a refusal
    /// leaves it playing so.
    chosen: Option<(Quality, PlayMethod)>,
    /// The file did not play as it is: the server converts it whatever the quality.
    direct_failed: bool,
    /// The server's indexes of the external subtitles of what plays, in the order the player
    /// was given them.
    externals: Vec<i32>,
    /// When the server last refused the quality the viewer chose.
    refused_at: Option<Instant>,
    /// Names this playback, so an answer for one the viewer left is told apart.
    id: u64,
    /// A downloaded file: the server never converts it.
    local: bool,
}

impl Fallback {
    /// `opened`: what the server gave for `plan`, which plays; the conversion [`open_first`] asked
    /// for plays from the start as one, `plan` being the file as it is.
    pub fn new(server: Arc<dyn MediaServer>, plan: Plan, opened: &Playback) -> Self {
        static PLAYBACKS: AtomicU64 = AtomicU64::new(0);
        let (method, played) = if opened.converted {
            (PlayMethod::Transcode, plan.converted(plan.start, opened))
        } else {
            (PlayMethod::DirectPlay, plan.clone())
        };
        Self {
            server,
            original: plan,
            plan: played,
            method,
            asking: None,
            chosen: None,
            direct_failed: false,
            externals: opened.external_subtitles.iter().map(|e| e.index).collect(),
            refused_at: None,
            id: PLAYBACKS.fetch_add(1, Ordering::Relaxed),
            local: false,
        }
    }

    #[allow(
        dead_code,
        reason = "played from the shell once offline mode (#137) wires downloads in"
    )]
    /// A downloaded file `opened` (`Local::playback`) of `plan`, which nothing replaces.
    pub fn local(server: Arc<dyn MediaServer>, plan: Plan, opened: &Playback) -> Self {
        Self {
            local: true,
            ..Self::new(server, plan, opened)
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// How the film plays now.
    pub fn method(&self) -> PlayMethod {
        self.method
    }

    /// A film that began as the server's conversion at `quality`: the session says so, as it
    /// does once a conversion answers.
    pub fn announce(&self, session: &mut Session, quality: Quality) {
        if self.method == PlayMethod::Transcode {
            session.playing_as(quality, true);
            session.subtitle = said(session.subtitle.take(), self.method);
        }
    }

    /// At each tick: what to ask the server when the file does not play. The conversion is
    /// asked once: whether the server refuses it or it shows no picture, the film cannot play.
    pub fn tick(&mut self, session: &mut Session, now: Instant) -> Option<TranscodeRequest> {
        if self.asking.is_some() {
            return None;
        }
        match step(self.method, session, self.plan.start, now) {
            Step::Convert { .. } if self.local => {
                session.fail(PlayerError(DOWNLOAD_CANNOT_PLAY.into()));
                None
            }
            Step::Convert { from } => {
                self.method = PlayMethod::Transcode;
                self.asking = Some(from);
                self.direct_failed = true;
                session.direct_play_failed();
                // Its words go: the conversion says what it does instead.
                session.forget_error();
                Some(self.original.transcode_request(from))
            }
            Step::CannotPlay => {
                session.fail(PlayerError("no picture".into()));
                None
            }
            Step::Nothing => None,
        }
    }

    /// The server's answer to the playback `id`: the player plays the conversion where the file
    /// stopped, and what to report of it comes back. Nothing for a playback the viewer left, or
    /// when the conversion cannot play, which is said.
    pub fn answered(
        &mut self,
        id: u64,
        session: &mut Session,
        opened: Result<Playback, ProviderError>,
        now: Instant,
    ) -> Option<PlaybackReport> {
        if id != self.id {
            return None;
        }
        let asked_from = self.asking.take()?;
        let chosen = self.chosen.take();
        // A quality the viewer chose starts where they are now: they may have sought meanwhile.
        let from = match chosen {
            Some(_) if session.position_known() => {
                Duration::from_secs_f64(session.position.max(0.))
            }
            _ => asked_from,
        };
        let opened = match (opened, chosen) {
            (Ok(opened), _) => opened,
            // The film the viewer was watching plays on as it did.
            (Err(error), Some((_, before))) => {
                eprintln!("bobine: the server does not give the quality asked: {error}");
                self.method = before;
                self.refused_at = Some(now);
                return None;
            }
            (Err(error), None) => {
                eprintln!("bobine: the server does not convert the film: {error}");
                session.fail(PlayerError("the server cannot convert it".into()));
                return None;
            }
        };
        self.plan = match self.method {
            PlayMethod::Transcode => self.original.converted(from, &opened),
            PlayMethod::DirectPlay => {
                let mut plan = self.original.clone();
                plan.start = from;
                plan
            }
        };
        if let Err(error) = restart(session, &self.plan, &opened) {
            // mpv's error code in words: no address in it.
            eprintln!("bobine: the conversion does not play: {error}");
            session.fail(error);
            return None;
        }
        self.externals = opened.external_subtitles.iter().map(|e| e.index).collect();
        let quality = chosen.map_or(session.quality(), |(quality, _)| quality);
        session.playing_as(quality, self.method == PlayMethod::Transcode);
        session.subtitle = said(session.subtitle.take(), self.method);
        Some(PlaybackReport {
            session: opened.session,
            film: opened.film,
            source: opened.source,
            position: from,
            paused: false,
            audio_track: self.plan.audio_index,
            subtitle_track: self.plan.subtitle_index,
            method: self.method,
        })
    }

    /// The viewer chose `quality` in the settings: what to ask the server, from where the film
    /// is. Nothing while the server is already asked.
    pub fn choose(&mut self, quality: Quality, session: &mut Session) -> Option<Reopen> {
        if self.asking.is_some() || self.local {
            return None;
        }
        let from = if session.position_known() {
            Duration::from_secs_f64(session.position.max(0.))
        } else {
            self.plan.start
        };
        // The tracks the viewer picked in the panel go with them.
        let converted = self.method == PlayMethod::Transcode;
        let (audio, subtitle) = self
            .plan
            .shown(session.tracks(), &self.externals, converted);
        self.original = self.original.with_tracks(audio, subtitle);
        self.refused_at = None;
        let reopen = match quality {
            Quality::Original if !self.direct_failed => {
                Reopen::Direct(self.original.film.clone(), self.original.source.clone())
            }
            Quality::Original => Reopen::Convert(self.original.transcode_request(from)),
            Quality::Cap(bits) => Reopen::Convert(TranscodeRequest {
                max_bitrate: Some(bits),
                ..self.original.transcode_request(from)
            }),
        };
        self.chosen = Some((quality, self.method));
        self.method = match reopen {
            Reopen::Convert(_) => PlayMethod::Transcode,
            Reopen::Direct(..) => PlayMethod::DirectPlay,
        };
        self.asking = Some(from);
        Some(reopen)
    }

    /// Whether "This quality could not be started." shows.
    pub fn refusal_shown(&self, now: Instant) -> bool {
        self.refused_at
            .is_some_and(|refused| now.duration_since(refused) < NOTICE_FOR)
    }

    /// Whether "Conversion par le serveur" shows.
    pub fn notice_shown(&self, session: &Session, now: Instant) -> bool {
        notice_shown(self.method, self.asking.is_some(), session, now)
    }
}

/// Whether "Conversion par le serveur" shows: while the server is asked for a conversion, then
/// until a moment after the converted picture comes, unless the film cannot play.
fn notice_shown(method: PlayMethod, asking: bool, session: &Session, now: Instant) -> bool {
    if asking {
        return method == PlayMethod::Transcode;
    }
    method == PlayMethod::Transcode
        && session.error().is_none()
        && session
            .first_frame_at()
            .is_none_or(|shown| now.duration_since(shown) < NOTICE_FOR)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bobine::provider::{ItemId, MediaServer, PlaySessionId, PlaybackReport, TranscodeRequest};

    use super::*;
    use crate::fake::{FILM, FakeServer, TRANSCODE_SESSION};
    use crate::film_page::{Choices, From, plan};
    use crate::player::fake::{Call, FakePlayer};
    use crate::player::{PlayerEvent, Track, TrackKind};
    use crate::video::fake::FakeDisplay;

    struct Fixture {
        server: Arc<FakeServer>,
        plan: Plan,
        player: FakePlayer,
        session: Session,
        method: PlayMethod,
        now: Instant,
    }

    impl Fixture {
        /// The film resumed at 72 min, playing directly.
        fn new() -> Self {
            let server = Arc::new(FakeServer::new(3, false));
            let detail = server.film(&ItemId(FILM.into())).unwrap();
            let plan = plan(
                &detail.film,
                &detail.versions,
                &Choices::new(&detail.versions),
                From::Resume,
            )
            .unwrap();
            let player = FakePlayer::default();
            let session = Session::new(
                Box::new(FakePlayer {
                    calls: player.calls.clone(),
                    queued: player.queued.clone(),
                    info: player.info.clone(),
                    ..Default::default()
                }),
                Box::new(FakeDisplay::default()),
                "Le Dernier Hiver".into(),
                None,
            );
            Self {
                server,
                plan,
                player,
                session,
                method: PlayMethod::DirectPlay,
                now: Instant::now(),
            }
        }

        /// The player says `events`, `seconds` later; what to do then.
        fn after(&mut self, seconds: u64, events: Vec<PlayerEvent>) -> Step {
            self.now += Duration::from_secs(seconds);
            self.player.queued.borrow_mut().extend(events);
            self.session.update(self.now);
            step(self.method, &self.session, self.plan.start, self.now)
        }

        /// What the window does on `Convert`: the server converts, the player restarts on it.
        fn convert(&mut self, from: Duration) {
            let opened = self
                .server
                .transcode(&self.plan.transcode_request(from))
                .unwrap();
            self.method = PlayMethod::Transcode;
            self.plan = self.plan.converted(from, &opened);
            restart(&mut self.session, &self.plan, &opened).unwrap();
        }

        fn loads(&self) -> Vec<(String, f64)> {
            self.player
                .calls
                .borrow()
                .iter()
                .filter_map(|call| match call {
                    Call::Load { url, start, .. } => Some((url.clone(), *start)),
                    _ => None,
                })
                .collect()
        }
    }

    fn failed() -> PlayerEvent {
        PlayerEvent::Error(PlayerError("loading failed".into()))
    }

    #[test]
    fn a_direct_stream_that_fails_to_load_starts_the_transcoded_one_from_the_same_position_once() {
        let mut f = Fixture::new();
        let resume = Duration::from_secs(72 * 60);
        assert_eq!(f.plan.start, resume);
        let Step::Convert { from } = f.after(1, vec![failed()]) else {
            panic!("not converted");
        };
        assert_eq!(from, resume);
        f.convert(from);

        assert_eq!(f.method, PlayMethod::Transcode);
        assert_eq!(f.session.error(), None, "the first error is not shown");
        let converted = f.server.state.lock().unwrap().converted.clone();
        assert_eq!(f.loads(), [(converted, resume.as_secs_f64())]);
        let asked = f.server.state.lock().unwrap().record.transcodes.clone();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].start, resume);

        assert_eq!(f.after(1, vec![PlayerEvent::Loaded]), Step::Nothing);
        assert_eq!(
            f.after(
                1,
                vec![PlayerEvent::FirstFrame, PlayerEvent::Position(4321.)]
            ),
            Step::Nothing
        );
        assert_eq!(f.after(30, vec![]), Step::Nothing);
    }

    #[test]
    fn a_second_failure_says_it_cannot_play() {
        let mut f = Fixture::new();
        let Step::Convert { from } = f.after(1, vec![failed()]) else {
            panic!("not converted");
        };
        f.convert(from);
        assert_eq!(f.after(2, vec![failed()]), Step::Nothing);
        assert_eq!(
            f.session.error(),
            Some(&PlayerError("loading failed".into()))
        );
        assert_eq!(f.after(20, vec![]), Step::Nothing);
        assert_eq!(f.server.state.lock().unwrap().record.transcodes.len(), 1);
    }

    #[test]
    fn a_direct_stream_without_a_picture_is_converted_from_where_it_was() {
        let mut f = Fixture::new();
        assert_eq!(
            f.after(1, vec![PlayerEvent::Loaded, PlayerEvent::Position(4330.)]),
            Step::Nothing
        );
        assert_eq!(f.after(19, vec![]), Step::Nothing, "still within the wait");
        assert_eq!(
            f.after(1, vec![]),
            Step::Convert {
                from: Duration::from_secs(4330)
            }
        );
    }

    #[test]
    fn a_file_that_never_opens_and_never_fails_is_converted_after_the_wait() {
        let mut f = Fixture::new();
        assert_eq!(f.after(1, vec![]), Step::Nothing);
        assert_eq!(
            f.after(20, vec![]),
            Step::Nothing,
            "a slow open is waited for"
        );
        assert_eq!(
            f.after(39, vec![]),
            Step::Nothing,
            "still within the long wait"
        );
        let Step::Convert { from } = f.after(1, vec![]) else {
            panic!("a black screen for ever");
        };
        assert_eq!(from, f.plan.start);
    }

    #[test]
    fn a_slow_open_that_shows_its_picture_late_is_not_converted() {
        let mut f = Fixture::new();
        assert_eq!(f.after(1, vec![]), Step::Nothing);
        assert_eq!(f.after(15, vec![PlayerEvent::Loaded]), Step::Nothing);
        assert_eq!(
            f.after(10, vec![]),
            Step::Nothing,
            "25 s in, 10 s after Loaded"
        );
        assert_eq!(f.after(1, vec![PlayerEvent::FirstFrame]), Step::Nothing);
        assert_eq!(f.after(600, vec![]), Step::Nothing);
    }

    #[test]
    fn a_conversion_that_never_opens_cannot_play_after_the_wait() {
        let mut f = Fixture::new();
        f.convert(Duration::from_secs(60));
        assert_eq!(f.after(1, vec![]), Step::Nothing);
        assert_eq!(
            f.after(60, vec![]),
            Step::Nothing,
            "a slow server's playlist"
        );
        assert_eq!(f.after(29, vec![]), Step::Nothing);
        assert_eq!(f.after(1, vec![]), Step::CannotPlay);
    }

    #[test]
    fn a_converted_stream_without_a_picture_cannot_play() {
        let mut f = Fixture::new();
        f.convert(Duration::from_secs(60));
        assert_eq!(f.after(1, vec![PlayerEvent::Loaded]), Step::Nothing);
        assert_eq!(
            f.after(59, vec![]),
            Step::Nothing,
            "a slow server's first converted picture"
        );
        assert_eq!(f.after(1, vec![]), Step::CannotPlay);
    }

    #[test]
    fn a_film_that_showed_a_picture_is_never_converted() {
        let mut f = Fixture::new();
        f.after(1, vec![PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        assert_eq!(f.after(600, vec![failed()]), Step::Nothing);
        assert!(f.session.error().is_some(), "the error is shown");
    }

    #[test]
    fn the_line_under_the_title_says_the_server_converts() {
        let line = Some("2025 · 4K HDR10 · direct play".to_string());
        assert_eq!(
            said(line, PlayMethod::Transcode).as_deref(),
            Some("2025 · 4K HDR10 · converted by the server")
        );
    }

    #[test]
    fn the_notice_shows_while_the_server_converts_and_a_moment_after_the_picture() {
        let mut f = Fixture::new();
        f.after(1, vec![failed()]);
        // The window's fallback asks for the conversion.
        assert!(notice_shown(PlayMethod::Transcode, true, &f.session, f.now));
        f.convert(Duration::from_secs(60));
        f.after(1, vec![PlayerEvent::Loaded]);
        assert!(notice_shown(f.method, false, &f.session, f.now));
        f.after(4, vec![PlayerEvent::FirstFrame]);
        assert!(notice_shown(f.method, false, &f.session, f.now));
        f.after(5, vec![]);
        assert!(!notice_shown(f.method, false, &f.session, f.now));
    }

    #[test]
    fn a_direct_play_shows_no_notice_and_a_failed_conversion_hides_it() {
        let mut f = Fixture::new();
        f.after(1, vec![PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        assert!(!notice_shown(f.method, false, &f.session, f.now));
        f.convert(Duration::from_secs(60));
        f.after(1, vec![failed()]);
        assert!(!notice_shown(f.method, false, &f.session, f.now));
    }

    /// The film playing directly, its picture shown, at `seconds`.
    fn playing_at(
        server: &Arc<FakeServer>,
        seconds: f64,
    ) -> (Fallback, Session, FakePlayer, Instant) {
        let (mut fallback, mut session, player) = playing(server);
        session.offer_quality();
        let mut now = Instant::now();
        let events = vec![
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Position(seconds),
        ];
        assert_eq!(
            tick(&mut fallback, &mut session, &player, &mut now, 1, events),
            None
        );
        (fallback, session, player, now)
    }

    fn loads(player: &FakePlayer) -> Vec<(String, f64)> {
        player
            .calls
            .borrow()
            .iter()
            .filter_map(|call| match call {
                Call::Load { url, start, .. } => Some((url.clone(), *start)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_quality_cap_restarts_the_conversion_at_the_same_position() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player, _) = playing_at(&server, 4500.);
        let cap = Quality::Cap(4_000_000);
        let Some(Reopen::Convert(request)) = fallback.choose(cap, &mut session) else {
            panic!("not converted");
        };
        assert_eq!(request.start, Duration::from_secs(4500));
        assert_eq!(request.max_bitrate, Some(4_000_000));
        assert_eq!(fallback.choose(cap, &mut session), None, "asked once");
        let opened = Reopen::Convert(request).open(&*server);
        let id = fallback.id();
        let report = fallback
            .answered(id, &mut session, opened, Instant::now())
            .expect("plays");
        assert_eq!(report.method, PlayMethod::Transcode);
        assert_eq!(report.position, Duration::from_secs(4500));
        let converted = server.state.lock().unwrap().converted.clone();
        assert_eq!(loads(&player), [(converted, 4500.)]);
        assert_eq!(session.quality(), cap);
        assert_eq!(
            session.playback_info()[0],
            ("Playback", "Converted by the server".to_string())
        );
        assert_eq!(
            session.subtitle.as_deref(),
            Some("2025 · 4K HDR10 · converted by the server")
        );
    }

    #[test]
    fn back_to_original_restarts_direct_play_at_the_same_position() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player, mut now) = playing_at(&server, 4500.);
        let reopen = fallback
            .choose(Quality::Cap(4_000_000), &mut session)
            .unwrap();
        let id = fallback.id();
        fallback.answered(id, &mut session, reopen.open(&*server), Instant::now());
        let events = vec![
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Position(4600.),
        ];
        tick(&mut fallback, &mut session, &player, &mut now, 1, events);

        let reopen = fallback.choose(Quality::Original, &mut session);
        let Some(Reopen::Direct(film, source)) = reopen.clone() else {
            panic!("{reopen:?}");
        };
        assert_eq!(
            (&film, &source),
            (&fallback.plan.film, &fallback.plan.source)
        );
        let report = fallback
            .answered(
                id,
                &mut session,
                reopen.unwrap().open(&*server),
                Instant::now(),
            )
            .expect("plays");
        assert_eq!(report.method, PlayMethod::DirectPlay);
        assert_eq!(report.position, Duration::from_secs(4600));
        let stream = server.state.lock().unwrap().stream.clone();
        assert_eq!(loads(&player).last(), Some(&(stream, 4600.)));
        assert_eq!(session.quality(), Quality::Original);
        assert_eq!(
            session.playback_info()[0],
            ("Playback", "Direct play".to_string())
        );
        assert_eq!(
            session.subtitle.as_deref(),
            Some("2025 · 4K HDR10 · direct play")
        );
    }

    #[test]
    fn original_after_a_failed_direct_play_converts_without_a_cap() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player) = playing(&server);
        session.offer_quality();
        let mut now = Instant::now();
        let request = tick(
            &mut fallback,
            &mut session,
            &player,
            &mut now,
            1,
            vec![failed()],
        )
        .expect("converted");
        let id = fallback.id();
        fallback.answered(id, &mut session, server.transcode(&request), Instant::now());
        let events = vec![PlayerEvent::Loaded, PlayerEvent::FirstFrame];
        tick(&mut fallback, &mut session, &player, &mut now, 1, events);
        let reopen = fallback.choose(Quality::Cap(2_000_000), &mut session);
        fallback.answered(
            id,
            &mut session,
            reopen.unwrap().open(&*server),
            Instant::now(),
        );
        let Some(Reopen::Convert(request)) = fallback.choose(Quality::Original, &mut session)
        else {
            panic!("the file does not play as it is");
        };
        assert_eq!(request.max_bitrate, None);
    }

    #[test]
    fn a_quality_the_server_refuses_leaves_the_film_playing() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player, _) = playing_at(&server, 4500.);
        server.state.lock().unwrap().transcode_fails = true;
        let reopen = fallback
            .choose(Quality::Cap(4_000_000), &mut session)
            .unwrap();
        let id = fallback.id();
        assert_eq!(
            fallback.answered(id, &mut session, reopen.open(&*server), Instant::now()),
            None
        );
        assert_eq!(session.error(), None);
        assert!(loads(&player).is_empty());
        assert_eq!(session.quality(), Quality::Original);
        assert!(
            fallback
                .choose(Quality::Cap(2_000_000), &mut session)
                .is_some()
        );
    }

    /// The film resumed at 72 min, as the window plays it, on its own player.
    fn playing(server: &Arc<FakeServer>) -> (Fallback, Session, FakePlayer) {
        let f = Fixture::new();
        let player = FakePlayer::default();
        let session = Session::new(
            Box::new(FakePlayer {
                calls: player.calls.clone(),
                queued: player.queued.clone(),
                info: player.info.clone(),
                slow: false,
                load_fails: player.load_fails.clone(),
            }),
            Box::new(FakeDisplay::default()),
            "Le Dernier Hiver".into(),
            Some("2025 · 4K HDR10 · direct play".into()),
        );
        let opened = server.play(&f.plan.film, &f.plan.source).unwrap();
        let fallback = Fallback::new(server.clone(), f.plan, &opened);
        (fallback, session, player)
    }

    /// The window's tick, `seconds` later, with what the player says then.
    fn tick(
        fallback: &mut Fallback,
        session: &mut Session,
        player: &FakePlayer,
        now: &mut Instant,
        seconds: u64,
        events: Vec<PlayerEvent>,
    ) -> Option<TranscodeRequest> {
        *now += Duration::from_secs(seconds);
        player.queued.borrow_mut().extend(events);
        session.update(*now);
        fallback.tick(session, *now)
    }

    #[test]
    fn a_server_that_refuses_to_convert_is_asked_once_and_its_refusal_shown() {
        let server = Arc::new(FakeServer::new(3, false));
        server.state.lock().unwrap().transcode_fails = true;
        let (mut fallback, mut session, player) = playing(&server);
        let mut now = Instant::now();
        let mut asked = 0;
        let request = tick(
            &mut fallback,
            &mut session,
            &player,
            &mut now,
            1,
            vec![failed()],
        )
        .expect("converted");
        asked += 1;
        assert!(
            fallback.notice_shown(&session, now),
            "while the server is asked"
        );
        assert_eq!(
            tick(&mut fallback, &mut session, &player, &mut now, 1, vec![]),
            None
        );
        let answer = server.transcode(&request);
        assert!(answer.is_err());
        let id = fallback.id();
        assert_eq!(
            fallback.answered(id, &mut session, answer, Instant::now()),
            None
        );
        assert_eq!(
            session.error(),
            Some(&PlayerError("the server cannot convert it".into()))
        );
        assert!(!fallback.notice_shown(&session, now));
        for _ in 0..100 {
            asked += usize::from(
                tick(&mut fallback, &mut session, &player, &mut now, 1, vec![]).is_some(),
            );
        }
        assert_eq!(asked, 1);
        assert_eq!(server.state.lock().unwrap().record.transcodes.len(), 1);
        assert!(session.error().is_some(), "the refusal stays shown");
    }

    #[test]
    fn an_answer_after_the_viewer_left_does_nothing() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut left, mut first, player) = playing(&server);
        let mut now = Instant::now();
        let request =
            tick(&mut left, &mut first, &player, &mut now, 1, vec![failed()]).expect("converted");
        let asked = left.id();
        drop((left, first));

        let (mut fallback, mut session, player) = playing(&server);
        tick(
            &mut fallback,
            &mut session,
            &player,
            &mut now,
            1,
            vec![PlayerEvent::Loaded],
        );
        let answer = server.transcode(&request);
        assert_eq!(
            fallback.answered(asked, &mut session, answer, Instant::now()),
            None
        );
        assert!(player.calls.borrow().is_empty(), "the other film plays on");
        assert_eq!(session.error(), None);
        assert!(!fallback.notice_shown(&session, now));
    }

    #[test]
    fn a_conversion_that_plays_is_reported_as_before() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player) = playing(&server);
        let mut now = Instant::now();
        let request = tick(
            &mut fallback,
            &mut session,
            &player,
            &mut now,
            1,
            vec![failed()],
        )
        .expect("converted");
        let resume = Duration::from_secs(72 * 60);
        let answer = server.transcode(&request);
        let id = fallback.id();
        let report = fallback
            .answered(id, &mut session, answer, Instant::now())
            .expect("plays");
        assert_eq!(
            report,
            PlaybackReport {
                session: PlaySessionId(TRANSCODE_SESSION.into()),
                film: request.film.clone(),
                source: request.source.clone(),
                position: resume,
                paused: false,
                audio_track: request.audio_track,
                subtitle_track: fallback.plan.subtitle_index,
                method: PlayMethod::Transcode,
            }
        );
        let converted = server.state.lock().unwrap().converted.clone();
        let loads: Vec<_> = player
            .calls
            .borrow()
            .iter()
            .filter_map(|call| match call {
                Call::Load { url, start, .. } => Some((url.clone(), *start)),
                _ => None,
            })
            .collect();
        assert_eq!(loads, [(converted, resume.as_secs_f64())]);
        assert_eq!(
            session.subtitle.as_deref(),
            Some("2025 · 4K HDR10 · converted by the server")
        );
        assert_eq!(session.error(), None, "the first error is not shown");
        tick(
            &mut fallback,
            &mut session,
            &player,
            &mut now,
            1,
            vec![PlayerEvent::Loaded],
        );
        assert!(fallback.notice_shown(&session, now));
        assert_eq!(
            tick(
                &mut fallback,
                &mut session,
                &player,
                &mut now,
                2,
                vec![failed()]
            ),
            None,
            "never converted twice"
        );
    }

    #[test]
    fn a_quality_chosen_starts_where_the_viewer_is_when_the_server_answers() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player, mut now) = playing_at(&server, 4500.);
        let reopen = fallback
            .choose(Quality::Cap(4_000_000), &mut session)
            .unwrap();
        // The viewer seeks while the server is asked.
        let events = vec![PlayerEvent::Position(4620.)];
        tick(&mut fallback, &mut session, &player, &mut now, 1, events);
        let id = fallback.id();
        let report = fallback
            .answered(id, &mut session, reopen.open(&*server), now)
            .expect("plays");
        let converted = server.state.lock().unwrap().converted.clone();
        assert_eq!(loads(&player), [(converted, 4620.)]);
        assert_eq!(report.position, Duration::from_secs(4620));
    }

    fn track(kind: TrackKind, id: i64, external: bool, selected: bool) -> Track {
        Track {
            id,
            kind,
            title: None,
            language: None,
            codec: None,
            channels: None,
            forced: false,
            external,
            selected,
        }
    }

    #[test]
    fn the_tracks_chosen_in_the_panel_survive_a_quality_change() {
        let server = Arc::new(FakeServer::new(3, false));
        let detail = server.film(&ItemId(FILM.into())).unwrap();
        // French audio and the full French subtitles, as the film page started it.
        let mut choices = Choices::new(&detail.versions);
        choices.audio = Some(1);
        choices.subtitle = Some(4);
        let plan = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let opened = server.play(&plan.film, &plan.source).unwrap();
        let (_, mut session, player) = playing(&server);
        let mut fallback = Fallback::new(server.clone(), plan, &opened);
        session.offer_quality();
        let mut now = Instant::now();
        // English audio and the external English subtitles, picked by hand in the panel.
        let picked = vec![
            track(TrackKind::Video, 1, false, true),
            track(TrackKind::Audio, 1, false, false),
            track(TrackKind::Audio, 2, false, true),
            track(TrackKind::Subtitle, 1, false, false),
            track(TrackKind::Subtitle, 2, false, false),
            track(TrackKind::Subtitle, 3, true, true),
        ];
        let events = vec![
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Position(4500.),
            PlayerEvent::Tracks(picked),
        ];
        tick(&mut fallback, &mut session, &player, &mut now, 1, events);

        let Some(Reopen::Convert(request)) = fallback.choose(Quality::Cap(4_000_000), &mut session)
        else {
            panic!("not converted");
        };
        assert_eq!(request.audio_track, Some(2), "still English");
        assert_eq!(
            request.subtitle_track, None,
            "an external file stays a file"
        );
        let id = fallback.id();
        let report = fallback
            .answered(
                id,
                &mut session,
                Reopen::Convert(request).open(&*server),
                now,
            )
            .expect("plays");
        assert_eq!(
            (report.audio_track, report.subtitle_track),
            (Some(2), Some(5))
        );
        let Some(Call::Load { subtitle, .. }) = player.calls.borrow().last().cloned() else {
            panic!("not loaded");
        };
        assert_eq!(subtitle, crate::player::FirstSubtitle::External(0));

        // The conversion holds the one audio track asked; back to the file as it is.
        let converted = vec![
            track(TrackKind::Audio, 1, false, true),
            track(TrackKind::Subtitle, 1, true, true),
        ];
        let events = vec![
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Tracks(converted),
        ];
        tick(&mut fallback, &mut session, &player, &mut now, 1, events);
        let reopen = fallback.choose(Quality::Original, &mut session).unwrap();
        let report = fallback
            .answered(id, &mut session, reopen.open(&*server), now)
            .expect("plays");
        assert_eq!(
            (report.audio_track, report.subtitle_track),
            (Some(2), Some(5))
        );
        let Some(Call::Load {
            audio, subtitle, ..
        }) = player.calls.borrow().last().cloned()
        else {
            panic!("not loaded");
        };
        assert_eq!(audio, Some(2), "mpv's second audio track: English");
        assert_eq!(subtitle, crate::player::FirstSubtitle::External(0));
    }

    #[test]
    fn original_shows_no_conversion_notice_while_it_waits() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player, mut now) = playing_at(&server, 4500.);
        let reopen = fallback
            .choose(Quality::Cap(4_000_000), &mut session)
            .unwrap();
        assert!(
            fallback.notice_shown(&session, now),
            "a conversion is asked"
        );
        let id = fallback.id();
        fallback.answered(id, &mut session, reopen.open(&*server), now);
        let events = vec![PlayerEvent::Loaded, PlayerEvent::FirstFrame];
        tick(&mut fallback, &mut session, &player, &mut now, 1, events);
        tick(&mut fallback, &mut session, &player, &mut now, 10, vec![]);
        assert!(!fallback.notice_shown(&session, now));

        fallback.choose(Quality::Original, &mut session).unwrap();
        assert!(!fallback.notice_shown(&session, now), "the file as it is");
    }

    #[test]
    fn a_refused_quality_says_so_for_a_moment() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, _, now) = playing_at(&server, 4500.);
        assert!(!fallback.refusal_shown(now));
        server.state.lock().unwrap().transcode_fails = true;
        let reopen = fallback
            .choose(Quality::Cap(4_000_000), &mut session)
            .unwrap();
        let id = fallback.id();
        fallback.answered(id, &mut session, reopen.open(&*server), now);
        assert!(fallback.refusal_shown(now));
        assert!(fallback.refusal_shown(now + NOTICE_FOR - Duration::from_millis(1)));
        assert!(!fallback.refusal_shown(now + NOTICE_FOR));
        server.state.lock().unwrap().transcode_fails = false;
        fallback.choose(Quality::Cap(2_000_000), &mut session);
        assert!(!fallback.refusal_shown(now), "another try");
    }

    #[test]
    fn a_quality_that_does_not_load_is_not_marked_chosen() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player, now) = playing_at(&server, 4500.);
        player.load_fails.set(true);
        let reopen = fallback
            .choose(Quality::Cap(4_000_000), &mut session)
            .unwrap();
        let id = fallback.id();
        assert_eq!(
            fallback.answered(id, &mut session, reopen.open(&*server), now),
            None
        );
        assert!(session.error().is_some());
        assert_eq!(session.quality(), Quality::Original);
    }

    #[test]
    fn original_after_a_failed_direct_play_is_named_without_direct() {
        let server = Arc::new(FakeServer::new(3, false));
        let (mut fallback, mut session, player) = playing(&server);
        assert_eq!(
            session.quality_label(Quality::Original),
            "Original (direct)"
        );
        let mut now = Instant::now();
        tick(
            &mut fallback,
            &mut session,
            &player,
            &mut now,
            1,
            vec![failed()],
        );
        assert_eq!(session.quality_label(Quality::Original), "Original");
    }
}
