//! Direct play first; when the file does not play, once, the server's conversion. A file does
//! not play when its load ends with an error, or when no picture comes within a few seconds of
//! its opening. A file that showed a picture plays: an error later is not one a conversion
//! mends, and is shown as it is.

use std::time::{Duration, Instant};

use bobine::provider::{PlayMethod, Playback};

use crate::film_page::Plan;
use crate::player::PlayerError;
use crate::player::session::Session;

/// How long an open file may go without a picture: the seek to a resume point over the network
/// comes first.
pub const FIRST_PICTURE_WAIT: Duration = Duration::from_secs(10);

/// How long the conversion notice stays once the converted picture shows.
pub const NOTICE_FOR: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// Ask the server to convert the version, from there.
    Convert {
        from: Duration,
    },
    /// The conversion shows no picture either: the film cannot play.
    CannotPlay,
}

/// What to do now about a playback sent by `method`, planned to start at `planned`.
pub fn step(method: PlayMethod, session: &Session, planned: Duration, now: Instant) -> Step {
    if session.first_frame_at().is_some() {
        return Step::Nothing;
    }
    let failed = session.error().is_some();
    let late = session
        .loaded_at()
        .is_some_and(|loaded| now.duration_since(loaded) >= FIRST_PICTURE_WAIT);
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
pub fn restart(session: &mut Session, plan: &Plan, opened: &Playback) -> Result<(), PlayerError> {
    let externals = plan.externals(opened);
    session.restart(plan.media(opened, &externals))
}

/// The line under the title once the server converts: "lecture directe" becomes "conversion
/// par le serveur".
pub fn said(subtitle: Option<String>) -> Option<String> {
    subtitle.map(|line| line.replace("lecture directe", "conversion par le serveur"))
}

/// Whether "Conversion par le serveur" shows: while the server is asked, then until a moment
/// after the converted picture comes, unless the film cannot play.
pub fn notice_shown(method: PlayMethod, asking: bool, session: &Session, now: Instant) -> bool {
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

    use bobine::provider::{ItemId, MediaServer};

    use super::*;
    use crate::fake::{FILM, FakeServer};
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
            self.plan = self.plan.converted(from);
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
        assert_eq!(f.after(9, vec![]), Step::Nothing, "still within the wait");
        assert_eq!(
            f.after(1, vec![]),
            Step::Convert {
                from: Duration::from_secs(4330)
            }
        );
    }

    #[test]
    fn a_converted_stream_without_a_picture_cannot_play() {
        let mut f = Fixture::new();
        f.convert(Duration::from_secs(60));
        assert_eq!(f.after(1, vec![PlayerEvent::Loaded]), Step::Nothing);
        assert_eq!(f.after(10, vec![]), Step::CannotPlay);
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
        let line = Some("2025 · 4K HDR10 · lecture directe".to_string());
        assert_eq!(
            said(line).as_deref(),
            Some("2025 · 4K HDR10 · conversion par le serveur")
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
}
