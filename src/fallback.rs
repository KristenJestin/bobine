//! Direct play first; when the file does not play, once, the server's conversion. A file does
//! not play when its load ends with an error, or when no picture comes within a few seconds of
//! its opening, or when it never opens within a longer limit. A file that showed a picture
//! plays: an error later is not one a conversion mends, and is shown as it is.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use bobine::provider::{
    MediaServer, PlayMethod, Playback, PlaybackReport, ProviderError, TranscodeRequest,
};

use crate::film_page::Plan;
use crate::player::PlayerError;
use crate::player::session::Session;

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
/// by the server".
fn said(subtitle: Option<String>) -> Option<String> {
    subtitle.map(|line| line.replace("direct play", "converted by the server"))
}

/// A film of the server, which the server may convert when it does not play as it is: once.
pub struct Fallback {
    pub server: Arc<dyn MediaServer>,
    pub plan: Plan,
    method: PlayMethod,
    /// The server is being asked to convert from there.
    asking: Option<Duration>,
    /// Names this playback, so an answer for one the viewer left is told apart.
    id: u64,
}

impl Fallback {
    pub fn new(server: Arc<dyn MediaServer>, plan: Plan) -> Self {
        static PLAYBACKS: AtomicU64 = AtomicU64::new(0);
        Self {
            server,
            plan,
            method: PlayMethod::DirectPlay,
            asking: None,
            id: PLAYBACKS.fetch_add(1, Ordering::Relaxed),
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// At each tick: what to ask the server when the file does not play. The conversion is
    /// asked once: whether the server refuses it or it shows no picture, the film cannot play.
    pub fn tick(&mut self, session: &mut Session, now: Instant) -> Option<TranscodeRequest> {
        if self.asking.is_some() {
            return None;
        }
        match step(self.method, session, self.plan.start, now) {
            Step::Convert { from } => {
                self.method = PlayMethod::Transcode;
                self.asking = Some(from);
                // Its words go: the conversion says what it does instead.
                session.forget_error();
                Some(self.plan.transcode_request(from))
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
    ) -> Option<PlaybackReport> {
        if id != self.id {
            return None;
        }
        let from = self.asking.take()?;
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => {
                eprintln!("bobine: the server does not convert the film: {error}");
                session.fail(PlayerError("the server cannot convert it".into()));
                return None;
            }
        };
        self.plan = self.plan.converted(from, &opened);
        session.subtitle = said(session.subtitle.take());
        if let Err(error) = restart(session, &self.plan, &opened) {
            // mpv's error code in words: no address in it.
            eprintln!("bobine: the conversion does not play: {error}");
            session.fail(error);
            return None;
        }
        Some(PlaybackReport {
            session: opened.session,
            film: opened.film,
            source: opened.source,
            position: from,
            paused: false,
            audio_track: self.plan.audio_index,
            subtitle_track: self.plan.subtitle_index,
            method: PlayMethod::Transcode,
        })
    }

    /// Whether "Conversion par le serveur" shows.
    pub fn notice_shown(&self, session: &Session, now: Instant) -> bool {
        notice_shown(self.method, self.asking.is_some(), session, now)
    }
}

/// Whether "Conversion par le serveur" shows: while the server is asked, then until a moment
/// after the converted picture comes, unless the film cannot play.
fn notice_shown(method: PlayMethod, asking: bool, session: &Session, now: Instant) -> bool {
    if asking {
        return true;
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
    use crate::player::PlayerEvent;
    use crate::player::fake::{Call, FakePlayer};
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
            said(line).as_deref(),
            Some("2025 · 4K HDR10 · converted by the server")
        );
    }

    #[test]
    fn the_notice_shows_while_the_server_converts_and_a_moment_after_the_picture() {
        let mut f = Fixture::new();
        f.after(1, vec![failed()]);
        assert!(notice_shown(f.method, true, &f.session, f.now));
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

    /// The film resumed at 72 min, as the window plays it, on its own player.
    fn playing(server: &Arc<FakeServer>) -> (Fallback, Session, FakePlayer) {
        let f = Fixture::new();
        let player = FakePlayer::default();
        let session = Session::new(
            Box::new(FakePlayer {
                calls: player.calls.clone(),
                queued: player.queued.clone(),
                info: player.info.clone(),
            }),
            Box::new(FakeDisplay::default()),
            "Le Dernier Hiver".into(),
            Some("2025 · 4K HDR10 · direct play".into()),
        );
        let fallback = Fallback::new(server.clone(), f.plan);
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
        assert_eq!(fallback.answered(id, &mut session, answer), None);
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
        assert_eq!(fallback.answered(asked, &mut session, answer), None);
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
        let report = fallback.answered(id, &mut session, answer).expect("plays");
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
}
