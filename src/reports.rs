//! What the server is told of a playback: "playing" once the film is loaded, where the viewer is
//! every `PROGRESS_EVERY` and on each pause and seek, "stopped" when they leave or the film ends.
//! The calls block: one worker thread per playback sends them, in order, off the window's thread.
//! While the server cannot be reached, a download's playback writes them to the journal instead.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use bobine::provider::{MediaServer, PROGRESS_EVERY, PlaybackReport, ProviderError};

use crate::downloads::{Journal, OfflinePlay};
use crate::player::session::Session;
use crate::series_page::reached_the_end;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Started,
    Progress,
    Stopped,
    /// Stopped at the film's real end: it is watched.
    Ended,
}

/// Where the reports of a playback go.
pub enum To {
    Server(Arc<dyn MediaServer>),
    #[allow(
        dead_code,
        reason = "played from the shell once offline mode (#137) wires downloads in"
    )]
    /// The server cannot be reached: the journal keeps the play until it is told.
    Journal(Journal),
}

pub struct Reporting {
    /// The session, film, version and tracks: the position and pause change with each report.
    base: PlaybackReport,
    sender: Sender<(Kind, PlaybackReport)>,
    worker: JoinHandle<()>,
    started: bool,
    stopped: bool,
    last: Option<Instant>,
    paused: bool,
    seeks: u64,
}

impl Reporting {
    pub fn start(server: Arc<dyn MediaServer>, base: PlaybackReport) -> Self {
        Self::start_after(server, base, None)
    }

    #[allow(
        dead_code,
        reason = "played from the shell once offline mode (#137) wires downloads in"
    )]
    /// Reports a playback to `to`: the server, or the journal.
    pub fn to(to: To, base: PlaybackReport) -> Self {
        Self::start_to(to, base, None)
    }

    /// Reports another playback of the same film, once `previous` has sent all of its own: the
    /// "stopped" of an attempt that failed must not reach the server after the first report of
    /// the conversion that replaces it.
    pub fn start_after(
        server: Arc<dyn MediaServer>,
        base: PlaybackReport,
        previous: Option<JoinHandle<()>>,
    ) -> Self {
        Self::start_to(To::Server(server), base, previous)
    }

    fn start_to(to: To, base: PlaybackReport, previous: Option<JoinHandle<()>>) -> Self {
        let (sender, receiver) = channel::<(Kind, PlaybackReport)>();
        let worker = std::thread::spawn(move || {
            if let Some(previous) = previous {
                let _ = previous.join();
            }
            for (kind, report) in receiver {
                let sent = match &to {
                    To::Server(server) => match kind {
                        Kind::Started => server.report_started(&report),
                        Kind::Progress => server.report_progress(&report),
                        Kind::Stopped | Kind::Ended => server.report_stopped(&report),
                    },
                    To::Journal(journal) => write(journal, kind, &report),
                };
                // The film plays on: the next report says where the viewer is anyway.
                if let Err(error) = sent {
                    eprintln!("bobine: a playback report failed: {error}");
                }
            }
        });
        Self {
            base,
            sender,
            worker,
            started: false,
            stopped: false,
            last: None,
            paused: false,
            seeks: 0,
        }
    }

    fn send(&mut self, kind: Kind, position: f64, paused: bool, now: Instant) {
        let report = PlaybackReport {
            position: Duration::from_secs_f64(position.max(0.)),
            paused,
            ..self.base.clone()
        };
        // The worker only stops once this side is dropped.
        let _ = self.sender.send((kind, report));
        self.last = Some(now);
    }

    /// Where the viewer is: until mpv says, where the film was planned to start, so an early
    /// report never wipes the resume point.
    fn position(&self, session: &Session) -> f64 {
        if session.position_known() {
            session.position
        } else {
            self.base.position.as_secs_f64()
        }
    }

    /// Called after each update of the session; `ended` when the film reached its end.
    pub fn tick(&mut self, now: Instant, session: &Session, ended: bool) {
        if self.stopped || !session.loaded() {
            return;
        }
        if !self.started {
            self.started = true;
            self.paused = session.paused;
            self.seeks = session.seeks();
            self.send(Kind::Started, self.position(session), session.paused, now);
        }
        if ended {
            // Where it really ended: a dropped network stream also ends with EOF, and the server
            // marks the film watched by its own threshold.
            self.stopped = true;
            let position = self.position(session);
            let kind = match reached_the_end(position, session.duration) {
                true => Kind::Ended,
                false => Kind::Stopped,
            };
            self.send(kind, position, false, now);
            return;
        }
        let changed = session.paused != self.paused || session.seeks() != self.seeks;
        let due = self.last.is_none_or(|last| now - last >= PROGRESS_EVERY);
        if changed || due {
            self.paused = session.paused;
            self.seeks = session.seeks();
            self.send(Kind::Progress, self.position(session), session.paused, now);
        }
    }

    /// The viewer leaves: "stopped" where they are, if the film had started. The handle ends
    /// once every report has been sent.
    pub fn finish(mut self, session: &Session) -> JoinHandle<()> {
        if self.started && !self.stopped {
            let position = self.position(session);
            self.send(Kind::Stopped, position, false, Instant::now());
        }
        drop(self.sender);
        self.worker
    }
}

/// A report as the journal keeps it: the item's last position, or watched when it ended at its
/// end.
fn write(journal: &Journal, kind: Kind, report: &PlaybackReport) -> Result<(), ProviderError> {
    let played = kind == Kind::Ended;
    journal
        .record(OfflinePlay {
            item: report.film.clone(),
            // As the server keeps it: a film watched has no resume point.
            position: if played {
                Duration::ZERO
            } else {
                report.position
            },
            played,
            at: SystemTime::now(),
        })
        .map_err(|error| ProviderError::Write(error.kind()))
}

/// What ends the reports of a playback the viewer leaves: the handle of its own reports, or,
/// when the conversion has not answered yet and so reports nothing, the one of the attempt that
/// failed, whose "stopped" is still on its way. A conversion's reports follow that one already.
pub fn last_reports(
    reporting: Option<Reporting>,
    stopping: Option<JoinHandle<()>>,
    session: &Session,
) -> Option<JoinHandle<()>> {
    reporting
        .map(|reporting| reporting.finish(session))
        .or(stopping)
}

/// Waits for the reports still to be sent, no longer than `bound`: a slow server must not keep
/// the window open. Whether they all went.
pub fn wait(worker: JoinHandle<()>, bound: Duration) -> bool {
    let end = Instant::now() + bound;
    while !worker.is_finished() {
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = worker.join();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FakeServer, Report};
    use crate::player::PlayerEvent;
    use crate::player::fake::FakePlayer;
    use crate::video::fake::FakeDisplay;
    use bobine::provider::{ItemId, MediaSourceId, PlaySessionId};

    struct Fixture {
        server: FakeServer,
        session: Session,
        player: FakePlayer,
        reporting: Reporting,
        now: Instant,
    }

    impl Fixture {
        fn new() -> Self {
            Self::starting_at(0)
        }

        /// A playback planned to start `start` seconds in, as a resume does.
        fn starting_at(start: u64) -> Self {
            let server = FakeServer::new(3, true);
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
            let reporting = Reporting::start(
                Arc::new(server.clone()),
                PlaybackReport {
                    session: PlaySessionId("session-1".into()),
                    film: ItemId(FILM.into()),
                    source: MediaSourceId("source-4k".into()),
                    position: Duration::from_secs(start),
                    paused: false,
                    audio_track: Some(2),
                    subtitle_track: Some(4),
                    method: bobine::provider::PlayMethod::DirectPlay,
                },
            );
            Self {
                server,
                session,
                player,
                reporting,
                now: Instant::now(),
            }
        }

        /// The player says `events`, `seconds` later.
        fn after(&mut self, seconds: u64, events: Vec<PlayerEvent>) {
            self.now += Duration::from_secs(seconds);
            self.player.queued.borrow_mut().extend(events);
            let ended = self.session.update(self.now);
            self.reporting.tick(self.now, &self.session, ended);
        }

        fn finish(self) -> Vec<Report> {
            self.reporting.finish(&self.session).join().unwrap();
            self.server.reports()
        }
    }

    fn summary(reports: &[Report]) -> Vec<(&'static str, u64, bool)> {
        reports
            .iter()
            .map(|report| match report {
                Report::Started(r) => ("started", r.position.as_secs(), r.paused),
                Report::Progress(r) => ("progress", r.position.as_secs(), r.paused),
                Report::Stopped(r) => ("stopped", r.position.as_secs(), r.paused),
            })
            .collect()
    }

    #[test]
    fn reports_sent_on_start_pause_seek_and_stop() {
        let mut f = Fixture::new();
        f.after(0, vec![PlayerEvent::Position(4320.)]);
        // Nothing before the film is loaded.
        f.after(0, vec![PlayerEvent::Loaded]);
        f.after(1, vec![PlayerEvent::Position(4321.)]);
        f.session.toggle_pause();
        f.after(1, vec![]);
        f.session.toggle_pause();
        f.session.skip(10.);
        f.after(1, vec![PlayerEvent::Position(4333.)]);
        f.after(1, vec![PlayerEvent::Position(4334.)]);
        let reports = f.finish();
        assert_eq!(
            summary(&reports),
            [
                ("started", 4320, false),
                ("progress", 4321, true),
                ("progress", 4333, false),
                ("stopped", 4334, false),
            ]
        );
        let Report::Started(started) = &reports[0] else {
            unreachable!()
        };
        assert_eq!(started.session, PlaySessionId("session-1".into()));
        assert_eq!(
            (started.audio_track, started.subtitle_track),
            (Some(2), Some(4))
        );
    }

    #[test]
    fn a_drag_along_the_seek_bar_sends_one_progress_report_at_release() {
        let mut f = Fixture::new();
        f.session.duration = 6000.;
        f.after(0, vec![PlayerEvent::Loaded, PlayerEvent::Position(100.)]);
        // The pointer is dragged along the bar, a tick every 100 ms.
        for step in 1..=20 {
            f.session.scrub_to_fraction(step as f32 / 40.);
            f.now += Duration::from_millis(100);
            f.reporting.tick(f.now, &f.session, false);
        }
        // The button is let go: the exact seek is the one the server hears of.
        f.session.seek_to_fraction(0.5);
        f.after(0, vec![PlayerEvent::Position(3000.)]);
        assert_eq!(
            summary(&f.finish()),
            [
                ("started", 100, false),
                ("progress", 3000, false),
                ("stopped", 3000, false),
            ]
        );
    }

    #[test]
    fn progress_is_reported_every_ten_seconds() {
        let mut f = Fixture::new();
        f.after(0, vec![PlayerEvent::Loaded, PlayerEvent::Position(0.)]);
        for second in 1..=25 {
            f.after(1, vec![PlayerEvent::Position(f64::from(second))]);
        }
        assert_eq!(
            summary(&f.finish()),
            [
                ("started", 0, false),
                ("progress", 10, false),
                ("progress", 20, false),
                ("stopped", 25, false),
            ]
        );
    }

    #[test]
    fn the_end_is_reported_once_and_marks_the_film_watched() {
        let mut f = Fixture::new();
        f.after(
            0,
            vec![
                PlayerEvent::Loaded,
                PlayerEvent::Duration(7860.),
                PlayerEvent::Position(7855.),
            ],
        );
        f.after(1, vec![PlayerEvent::EndOfFile]);
        let server = f.server.clone();
        assert_eq!(
            summary(&f.finish()),
            [("started", 7855, false), ("stopped", 7855, false)]
        );
        let state = server.state.lock().unwrap();
        let film = state.films.iter().find(|f| f.id.0 == FILM).unwrap();
        assert!(film.watched);
        assert_eq!(film.resume, None);
    }

    #[test]
    fn leaving_before_the_film_is_loaded_reports_nothing() {
        let mut f = Fixture::new();
        f.after(1, vec![PlayerEvent::Position(0.)]);
        assert_eq!(f.finish(), []);
    }

    #[test]
    fn leaving_keeps_the_resume_position_on_the_server() {
        let mut f = Fixture::new();
        f.after(0, vec![PlayerEvent::Loaded, PlayerEvent::Position(600.)]);
        let server = f.server.clone();
        f.finish();
        let state = server.state.lock().unwrap();
        let film = state.films.iter().find(|f| f.id.0 == FILM).unwrap();
        assert_eq!(film.resume, Some(Duration::from_secs(600)));
    }

    #[test]
    fn the_resume_position_is_never_wiped_by_an_early_report() {
        let mut f = Fixture::starting_at(4320);
        // Loaded comes before mpv's first time-pos: the viewer is where the film resumes.
        f.after(0, vec![PlayerEvent::Loaded]);
        let server = f.server.clone();
        assert_eq!(
            summary(&f.finish()),
            [("started", 4320, false), ("stopped", 4320, false)]
        );
        let state = server.state.lock().unwrap();
        let film = state.films.iter().find(|f| f.id.0 == FILM).unwrap();
        assert_eq!(film.resume, Some(Duration::from_secs(72 * 60)));
    }

    #[test]
    fn a_pause_before_the_first_position_reports_the_planned_start() {
        let mut f = Fixture::starting_at(4320);
        f.after(0, vec![PlayerEvent::Loaded]);
        f.session.toggle_pause();
        f.after(1, vec![]);
        assert_eq!(
            summary(&f.finish()),
            [
                ("started", 4320, false),
                ("progress", 4320, true),
                ("stopped", 4320, false)
            ]
        );
    }

    #[test]
    fn a_film_cut_off_is_stopped_where_it_was_not_at_its_duration() {
        let mut f = Fixture::new();
        f.after(
            0,
            vec![
                PlayerEvent::Loaded,
                PlayerEvent::Duration(7860.),
                PlayerEvent::Position(2400.),
            ],
        );
        // A dropped network stream ends with the reason EOF, far from the end.
        f.after(1, vec![PlayerEvent::EndOfFile]);
        let server = f.server.clone();
        assert_eq!(
            summary(&f.finish()),
            [("started", 2400, false), ("stopped", 2400, false)]
        );
        let state = server.state.lock().unwrap();
        let film = state.films.iter().find(|f| f.id.0 == FILM).unwrap();
        assert!(!film.watched);
        assert_eq!(film.resume, Some(Duration::from_secs(2400)));
    }

    #[test]
    fn closing_the_window_mid_film_sends_stopped_with_the_current_position() {
        let mut f = Fixture::starting_at(4320);
        f.after(0, vec![PlayerEvent::Loaded, PlayerEvent::Position(4400.)]);
        let server = f.server.clone();
        let worker = f.reporting.finish(&f.session);
        assert!(wait(worker, Duration::from_secs(2)));
        // Nothing joined the worker but `wait`: the report is there once it returned.
        assert_eq!(
            summary(&server.reports()),
            [("started", 4400, false), ("stopped", 4400, false)]
        );
    }

    #[test]
    fn the_reports_of_a_conversion_follow_the_stopped_of_the_attempt_that_failed() {
        let mut f = Fixture::new();
        f.server.state.lock().unwrap().stop_delay = Duration::from_millis(200);
        f.after(0, vec![PlayerEvent::Loaded]);
        let stopping = f.reporting.finish(&f.session);
        let mut conversion = Reporting::start_after(
            Arc::new(f.server.clone()),
            PlaybackReport {
                session: PlaySessionId("session-2".into()),
                film: ItemId(FILM.into()),
                source: MediaSourceId("source-4k".into()),
                position: Duration::ZERO,
                paused: false,
                audio_track: None,
                subtitle_track: None,
                method: bobine::provider::PlayMethod::Transcode,
            },
            Some(stopping),
        );
        conversion.tick(f.now, &f.session, false);
        conversion.finish(&f.session).join().unwrap();
        assert_eq!(
            summary(&f.server.reports()),
            [
                ("started", 0, false),
                ("stopped", 0, false),
                ("started", 0, false),
                ("stopped", 0, false),
            ]
        );
        let sessions: Vec<_> = f
            .server
            .reports()
            .iter()
            .map(|report| match report {
                Report::Started(r) | Report::Progress(r) | Report::Stopped(r) => {
                    r.session.0.clone()
                }
            })
            .collect();
        assert_eq!(
            sessions,
            ["session-1", "session-1", "session-2", "session-2"]
        );
    }

    /// The attempt that failed is being told stopped, the server taking 200 ms to answer.
    fn failed_attempt_stopping(f: &mut Fixture) -> JoinHandle<()> {
        f.server.state.lock().unwrap().stop_delay = Duration::from_millis(200);
        f.after(0, vec![PlayerEvent::Loaded]);
        let base = f.reporting.base.clone();
        let reporting = std::mem::replace(
            &mut f.reporting,
            Reporting::start(Arc::new(f.server.clone()), base),
        );
        reporting.finish(&f.session)
    }

    #[test]
    fn leaving_before_the_conversion_answers_still_waits_for_the_stopped_of_the_failed_attempt() {
        let mut f = Fixture::new();
        let stopping = failed_attempt_stopping(&mut f);
        let ends = last_reports(None, Some(stopping), &f.session).expect("a handle to wait for");
        assert!(wait(ends, Duration::from_secs(5)));
        assert_eq!(
            summary(&f.server.reports()),
            [("started", 0, false), ("stopped", 0, false)]
        );
    }

    #[test]
    fn leaving_during_a_conversion_ends_with_its_own_reports() {
        let mut f = Fixture::new();
        let stopping = failed_attempt_stopping(&mut f);
        let mut conversion = Reporting::start_after(
            Arc::new(f.server.clone()),
            f.reporting.base.clone(),
            Some(stopping),
        );
        conversion.tick(f.now, &f.session, false);
        let ends = last_reports(Some(conversion), None, &f.session).unwrap();
        assert!(wait(ends, Duration::from_secs(5)));
        assert_eq!(f.server.reports().len(), 4);
    }

    #[test]
    fn leaving_with_nothing_to_report_has_nothing_to_wait_for() {
        let f = Fixture::new();
        assert!(last_reports(None, None, &f.session).is_none());
    }

    #[test]
    fn waiting_for_the_reports_is_bounded() {
        let slow = std::thread::spawn(|| std::thread::sleep(Duration::from_secs(5)));
        let begun = Instant::now();
        assert!(!wait(slow, Duration::from_millis(100)));
        assert!(begun.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn waiting_for_reports_already_sent_returns_at_once() {
        let done = std::thread::spawn(|| {});
        assert!(wait(done, Duration::from_secs(2)));
    }

    #[test]
    fn skipping_the_credits_ends_the_film_at_its_end_and_marks_it_watched() {
        use bobine::provider::{Segment, SegmentKind};
        let mut f = Fixture::new();
        f.session.set_segments(vec![Segment {
            kind: SegmentKind::Outro,
            start: Duration::from_secs(7140),
            end: Duration::from_secs(7860),
        }]);
        f.after(
            0,
            vec![
                PlayerEvent::Loaded,
                PlayerEvent::Duration(7860.),
                PlayerEvent::Position(7200.),
            ],
        );
        assert!(f.session.press_skip().is_some());
        f.after(1, vec![PlayerEvent::Position(7201.)]);
        assert!(crate::series_page::reached_the_end(
            f.session.position,
            f.session.duration
        ));
        let server = f.server.clone();
        assert_eq!(
            summary(&f.finish()),
            [("started", 7200, false), ("stopped", 7860, false)]
        );
        let state = server.state.lock().unwrap();
        let film = state.films.iter().find(|f| f.id.0 == FILM).unwrap();
        assert!(film.watched);
        assert_eq!(film.resume, None);
    }
}
