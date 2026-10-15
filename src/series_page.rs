//! What the series page shows and what its buttons start, apart from the window: the episode the
//! main button plays, the one after an episode, the end-of-episode countdown and the labels. The
//! version and track chips and the playback plan are the film page's.

use std::time::{Duration, Instant};

use bobine::provider::{Episode, ItemId, MediaServer, ProviderError, Season, Series, SeriesDetail};

use crate::film_page;

/// How long the "Épisode suivant" card counts down before the next episode plays.
pub const COUNTDOWN: Duration = Duration::from_secs(10);

/// An end this close to the duration is the end of the episode; further, the stream was cut.
const END_WITHIN: f64 = 10.;

/// `S2 · É4`, when the server numbers the episode.
pub fn code(episode: &Episode) -> Option<String> {
    Some(format!(
        "S{} · É{}",
        episode.season_number?, episode.number?
    ))
}

/// `S2 · É4 Reprendre à 12 min` once started, else `Lecture S1 · É1`, after a play icon.
pub fn play_label(episode: &Episode) -> String {
    let play = film_page::play_label(&episode.item);
    match (code(episode), episode.item.resume) {
        (None, _) => play,
        (Some(code), Some(_)) => format!("{code} {play}"),
        (Some(code), None) => format!("{play} {code}"),
    }
}

/// `2021 – 2024`, `2023`.
pub fn years(series: &Series) -> Option<String> {
    let start = series.year?;
    Some(match series.end_year {
        Some(end) if end != start => format!("{start} – {end}"),
        _ => start.to_string(),
    })
}

/// The line under the title: years, rating, seasons, genres, age rating.
pub fn meta(detail: &SeriesDetail, seasons: usize) -> Vec<String> {
    let series = &detail.series;
    let mut parts: Vec<String> = Vec::new();
    parts.extend(years(series));
    parts.extend(
        series
            .community_rating
            .map(|rating| format!("★ {rating:.1}")),
    );
    if seasons > 0 {
        let noun = if seasons > 1 { "saisons" } else { "saison" };
        parts.push(format!("{seasons} {noun}"));
    }
    if !detail.genres.is_empty() {
        parts.push(detail.genres.join(", "));
    }
    parts.extend(detail.official_rating.clone());
    parts
}

/// `48 min · reste 18 min`: the runtime, and what is left once started.
pub fn duration_line(episode: &Episode) -> String {
    let item = &episode.item;
    item.runtime
        .map(film_page::short_duration)
        .into_iter()
        .chain(film_page::time_left(item))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The number on a series' poster: its episodes not watched yet.
pub fn badge(series: &Series) -> Option<String> {
    (series.unwatched > 0).then(|| series.unwatched.to_string())
}

/// What the player shows: the series, then `S2 · É4 · Le Signal`.
pub fn player_title(episode: &Episode) -> (String, String) {
    let subtitle = code(episode)
        .into_iter()
        .chain([episode.item.title.clone()])
        .collect::<Vec<_>>()
        .join(" · ");
    (episode.series_title.clone(), subtitle)
}

/// The episode the main button plays: Next Up (the one started, else the one after the last
/// watched), else the first of the series, watched again. Blocks on the server.
pub fn next_to_play(
    server: &dyn MediaServer,
    series: &ItemId,
) -> Result<Option<Episode>, ProviderError> {
    if let Some(next) = server.next_up(series)? {
        return Ok(Some(next));
    }
    let Some(first) = server.seasons(series)?.into_iter().next() else {
        return Ok(None);
    };
    Ok(server.episodes(series, &first.id)?.into_iter().next())
}

/// The episode after `episode`: the next of its season, else the first of the next season.
/// Blocks on the server.
pub fn following(
    server: &dyn MediaServer,
    episode: &Episode,
) -> Result<Option<Episode>, ProviderError> {
    let season = server.episodes(&episode.series, &episode.season)?;
    if let Some(at) = season.iter().position(|e| e.item.id == episode.item.id)
        && let Some(next) = season.into_iter().nth(at + 1)
    {
        return Ok(Some(next));
    }
    let seasons = server.seasons(&episode.series)?;
    let Some(next_season) = seasons
        .iter()
        .position(|s| s.id == episode.season)
        .and_then(|at| seasons.get(at + 1))
    else {
        return Ok(None);
    };
    Ok(server
        .episodes(&episode.series, &next_season.id)?
        .into_iter()
        .next())
}

/// Whether the player stopped at the end of the episode, and not on a stream cut short.
pub fn reached_the_end(position: f64, duration: f64) -> bool {
    duration > 0. && duration - position <= END_WITHIN
}

/// "✓" on a season: marks all its episodes watched, or, when they all are, no longer.
pub fn toggle_season_watched(
    server: &dyn MediaServer,
    season: &Season,
) -> Result<(), ProviderError> {
    server.set_watched(&season.id, season.unwatched > 0)
}

/// The "Épisode suivant" card: counts down, then gives the episode to play, unless cancelled.
#[derive(Default)]
pub struct UpNext {
    pending: Option<(Episode, Instant)>,
}

impl UpNext {
    pub fn start(&mut self, episode: Episode, now: Instant) {
        self.pending = Some((episode, now));
    }

    pub fn cancel(&mut self) {
        self.pending = None;
    }

    pub fn episode(&self) -> Option<&Episode> {
        self.pending.as_ref().map(|(episode, _)| episode)
    }

    /// Whole seconds left, rounded up: `10` at once, `1` in the last second.
    pub fn seconds_left(&self, now: Instant) -> u64 {
        let Some((_, since)) = &self.pending else {
            return 0;
        };
        let left = COUNTDOWN.saturating_sub(now.saturating_duration_since(*since));
        left.as_millis().div_ceil(1000) as u64
    }

    /// How much of the countdown is left, from 1 to 0: the card's bar.
    pub fn fraction_left(&self, now: Instant) -> f32 {
        let Some((_, since)) = &self.pending else {
            return 0.;
        };
        let gone = now.saturating_duration_since(*since).as_secs_f32();
        (1. - gone / COUNTDOWN.as_secs_f32()).clamp(0., 1.)
    }

    /// The episode to play now: once the countdown is over, or at once when `now_asked`.
    pub fn take(&mut self, now: Instant, now_asked: bool) -> Option<Episode> {
        let (_, since) = self.pending.as_ref()?;
        if !now_asked && now < *since + COUNTDOWN {
            return None;
        }
        self.pending.take().map(|(episode, _)| episode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeServer, Report, SERIES};
    use crate::film_page::{Choices, From};
    use crate::player::Player;
    use crate::player::PlayerEvent;
    use crate::player::fake::{Call, FakePlayer};
    use crate::player::session::Session;
    use crate::reports::Reporting;
    use crate::video::fake::FakeDisplay;
    use bobine::provider::PlaybackReport;
    use std::sync::Arc;

    fn series() -> ItemId {
        ItemId(SERIES.into())
    }

    fn id(season: u32, number: u32) -> ItemId {
        ItemId(format!("series-0-s{season}-e{number}"))
    }

    fn episode(server: &FakeServer, season: u32, number: u32) -> Episode {
        let state = server.state.lock().unwrap();
        state
            .episodes
            .iter()
            .find(|e| e.item.id == id(season, number))
            .cloned()
            .unwrap()
    }

    /// Changes an episode on the fake server, as if watched elsewhere.
    fn set(server: &FakeServer, season: u32, number: u32, watched: bool, resume: Option<u64>) {
        let mut state = server.state.lock().unwrap();
        let found = state
            .episodes
            .iter_mut()
            .find(|e| e.item.id == id(season, number))
            .unwrap();
        found.item.watched = watched;
        found.item.resume = resume.map(Duration::from_secs);
    }

    fn next(server: &FakeServer) -> ItemId {
        next_to_play(server, &series()).unwrap().unwrap().item.id
    }

    #[test]
    fn the_next_episode_is_the_one_started() {
        let server = FakeServer::new(1, true);
        assert_eq!(next(&server), id(2, 4));
    }

    #[test]
    fn the_next_episode_follows_the_last_watched_across_seasons() {
        let server = FakeServer::new(1, true);
        for number in 1..=6 {
            set(&server, 2, number, true, None);
        }
        assert_eq!(next(&server), id(3, 1));
    }

    #[test]
    fn a_series_never_started_begins_with_its_first_episode() {
        let server = FakeServer::new(1, true);
        let all: Vec<(u32, u32)> = (1..=3)
            .map(|n| (1, n))
            .chain((1..=6).map(|n| (2, n)))
            .collect();
        for (season, number) in all {
            set(&server, season, number, false, None);
        }
        assert_eq!(next(&server), id(1, 1));
    }

    #[test]
    fn a_series_watched_to_the_end_starts_again_from_its_first_episode() {
        let server = FakeServer::new(1, true);
        server
            .state
            .lock()
            .unwrap()
            .episodes
            .iter_mut()
            .for_each(|e| {
                e.item.watched = true;
                e.item.resume = None;
            });
        assert_eq!(next(&server), id(1, 1));
    }

    #[test]
    fn the_play_button_names_the_next_episode_and_where_it_resumes() {
        let server = FakeServer::new(1, true);
        assert_eq!(
            play_label(&episode(&server, 2, 4)),
            "S2 · É4 Reprendre à 30 min"
        );
        assert_eq!(play_label(&episode(&server, 3, 1)), "Lecture S3 · É1");
        let mut unnumbered = episode(&server, 3, 1);
        unnumbered.number = None;
        assert_eq!(play_label(&unnumbered), "Lecture");
    }

    #[test]
    fn the_series_page_says_years_rating_seasons_and_genres() {
        let server = FakeServer::new(1, true);
        let detail = server.series_detail(&series()).unwrap();
        assert_eq!(
            meta(&detail, 3),
            [
                "2023 – 2025",
                "★ 6.5",
                "3 saisons",
                "Science-fiction, Drame",
                "12+"
            ]
        );
        assert_eq!(meta(&detail, 1)[2], "1 saison");
        let mut ongoing = detail.series.clone();
        ongoing.end_year = None;
        assert_eq!(years(&ongoing).as_deref(), Some("2023"));
    }

    #[test]
    fn an_episode_line_says_its_runtime_and_what_is_left() {
        let server = FakeServer::new(1, true);
        assert_eq!(
            duration_line(&episode(&server, 2, 4)),
            "48 min · reste 18 min"
        );
        assert_eq!(duration_line(&episode(&server, 3, 1)), "48 min");
    }

    #[test]
    fn a_series_poster_shows_its_unwatched_episodes() {
        let server = FakeServer::new(1, true);
        let page = server
            .series(&ItemId("series".into()), &crate::grid::query(0))
            .unwrap();
        // Season 2 from its fourth episode, and season 3.
        assert_eq!(badge(&page.items[0]).as_deref(), Some("8"));
        let mut done = page.items[0].clone();
        done.unwatched = 0;
        assert_eq!(badge(&done), None);
    }

    #[test]
    fn the_player_title_names_the_series_season_and_episode() {
        let server = FakeServer::new(1, true);
        let (title, subtitle) = player_title(&episode(&server, 2, 4));
        assert_eq!(title, "Station Nord");
        assert_eq!(subtitle, "S2 · É4 · Le Signal");
    }

    #[test]
    fn an_episode_plays_at_its_resume_position() {
        let server = FakeServer::new(1, true);
        let episode = episode(&server, 2, 4);
        let choices = Choices::new(&episode.versions);
        let plan =
            film_page::plan(&episode.item, &episode.versions, &choices, From::Resume).unwrap();
        let playback = server.play(&plan.film, &plan.source).unwrap();
        let externals = plan.externals(&playback);
        let mut player = FakePlayer::default();
        player.load(plan.media(&playback, &externals)).unwrap();
        let Call::Load { start, .. } = player.calls.borrow()[0].clone() else {
            panic!("not a load");
        };
        assert_eq!(start, 30. * 60.);
        assert_eq!(
            server.state.lock().unwrap().record.plays,
            [(id(2, 4), episode.versions[0].id.clone())]
        );
    }

    /// Plays the episode on the fake server through the reports, from its resume point, to
    /// `stop_at` seconds; `ended` when the player said the file ended there.
    fn watch(server: &FakeServer, episode: &Episode, stop_at: f64, ended: bool) -> Vec<Report> {
        let player = FakePlayer::default();
        let mut session = Session::new(
            Box::new(FakePlayer {
                calls: player.calls.clone(),
                queued: player.queued.clone(),
            }),
            Box::new(FakeDisplay::default()),
            "Station Nord".into(),
            None,
        );
        let start = episode.item.resume.unwrap_or_default();
        let mut reporting = Reporting::start(
            Arc::new(server.clone()),
            PlaybackReport {
                session: bobine::provider::PlaySessionId("session-1".into()),
                film: episode.item.id.clone(),
                source: episode.versions[0].id.clone(),
                position: start,
                paused: false,
                audio_track: None,
                subtitle_track: None,
            },
        );
        let mut now = Instant::now();
        let mut step = |events: Vec<PlayerEvent>, now: Instant, session: &mut Session| {
            player.queued.borrow_mut().extend(events);
            let over = session.update(now);
            reporting.tick(now, session, over);
            over
        };
        step(
            vec![
                PlayerEvent::Loaded,
                PlayerEvent::Duration(48. * 60.),
                PlayerEvent::Position(start.as_secs_f64()),
            ],
            now,
            &mut session,
        );
        now += Duration::from_secs(11);
        step(
            vec![PlayerEvent::Position(start.as_secs_f64() + 11.)],
            now,
            &mut session,
        );
        now += Duration::from_secs(1);
        let mut events = vec![PlayerEvent::Position(stop_at)];
        if ended {
            events.push(PlayerEvent::EndOfFile);
        }
        let over = step(events, now, &mut session);
        assert_eq!(over, ended);
        assert_eq!(
            reached_the_end(session.position, session.duration),
            stop_at > 48. * 60. - 5.
        );
        reporting.finish(&session).join().unwrap();
        server.reports()
    }

    #[test]
    fn an_episode_reports_start_progress_and_stop_under_its_own_id() {
        let server = FakeServer::new(1, true);
        let episode = episode(&server, 2, 4);
        let reports = watch(&server, &episode, 1900., false);
        let kinds: Vec<(&str, u64)> = reports
            .iter()
            .map(|r| match r {
                Report::Started(r) => ("started", r.position.as_secs()),
                Report::Progress(r) => ("progress", r.position.as_secs()),
                Report::Stopped(r) => ("stopped", r.position.as_secs()),
            })
            .collect();
        assert_eq!(
            kinds,
            [("started", 1800), ("progress", 1811), ("stopped", 1900)]
        );
        assert!(reports.iter().all(|r| match r {
            Report::Started(r) | Report::Progress(r) | Report::Stopped(r) => r.film == id(2, 4),
        }));
    }

    #[test]
    fn leaving_an_episode_updates_its_resume_and_the_next_one() {
        let server = FakeServer::new(1, true);
        watch(&server, &episode(&server, 2, 4), 1900., false);
        let next = next_to_play(&server, &series()).unwrap().unwrap();
        assert_eq!(next.item.id, id(2, 4));
        assert_eq!(next.item.resume, Some(Duration::from_secs(1900)));

        watch(&server, &next, 48. * 60. - 1., true);
        assert!(episode(&server, 2, 4).item.watched);
        assert_eq!(
            next_to_play(&server, &series()).unwrap().unwrap().item.id,
            id(2, 5)
        );
        let seasons = server.seasons(&series()).unwrap();
        assert_eq!(seasons[1].unwatched, 2);
    }

    #[test]
    fn a_stream_cut_before_the_end_shows_no_next_episode_card() {
        assert!(!reached_the_end(40. * 60., 48. * 60.));
        assert!(!reached_the_end(100., 0.));
        assert!(reached_the_end(48. * 60. - 0.04, 48. * 60.));
        let server = FakeServer::new(1, true);
        // Cut at 40 min: no card, and the episode keeps where it stopped.
        watch(&server, &episode(&server, 2, 4), 40. * 60., true);
        assert!(!episode(&server, 2, 4).item.watched);
    }

    #[test]
    fn the_episode_after_one_is_the_next_of_its_season() {
        let server = FakeServer::new(1, true);
        let after = following(&server, &episode(&server, 2, 4))
            .unwrap()
            .unwrap();
        assert_eq!(after.item.id, id(2, 5));
    }

    #[test]
    fn the_episode_after_the_last_of_a_season_opens_the_next_season() {
        let server = FakeServer::new(1, true);
        let after = following(&server, &episode(&server, 2, 6))
            .unwrap()
            .unwrap();
        assert_eq!(after.item.id, id(3, 1));
        assert_eq!(following(&server, &episode(&server, 3, 5)).unwrap(), None);
    }

    #[test]
    fn auto_next_plays_the_next_episode_after_ten_seconds() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), now);
        assert_eq!(card.seconds_left(now), 10);
        assert_eq!(card.seconds_left(now + Duration::from_millis(9500)), 1);
        assert_eq!(card.fraction_left(now), 1.);
        assert_eq!(card.fraction_left(now + Duration::from_secs(5)), 0.5);
        assert_eq!(card.take(now + Duration::from_secs(9), false), None);
        assert!(card.episode().is_some());
        let played = card.take(now + COUNTDOWN, false).unwrap();
        assert_eq!(played.item.id, id(2, 5));
        assert!(card.episode().is_none());
        assert_eq!(card.take(now + COUNTDOWN * 2, false), None);
    }

    #[test]
    fn auto_next_plays_at_once_when_asked() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), now);
        assert_eq!(card.take(now, true).unwrap().item.id, id(2, 5));
    }

    #[test]
    fn auto_next_is_cancelled() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), now);
        card.cancel();
        assert!(card.episode().is_none());
        assert_eq!(card.take(now + COUNTDOWN * 2, false), None);
    }

    #[test]
    fn an_episode_is_marked_watched_and_unwatched() {
        let server = FakeServer::new(1, true);
        let started = episode(&server, 2, 4);
        let watched = film_page::toggle_watched(&server, &started.item).unwrap();
        assert!(watched.watched);
        assert!(episode(&server, 2, 4).item.watched);
        film_page::toggle_watched(&server, &watched).unwrap();
        assert!(!episode(&server, 2, 4).item.watched);
        assert_eq!(
            server.state.lock().unwrap().record.watched,
            [(id(2, 4), true), (id(2, 4), false)]
        );
    }

    #[test]
    fn a_whole_season_is_marked_watched_and_unwatched() {
        let server = FakeServer::new(1, true);
        let season = |n: usize| server.seasons(&series()).unwrap()[n].clone();
        toggle_season_watched(&server, &season(1)).unwrap();
        assert_eq!(season(1).unwatched, 0);
        assert!((1..=6).all(|n| episode(&server, 2, n).item.watched));
        // All watched: the same button marks them no longer watched.
        toggle_season_watched(&server, &season(1)).unwrap();
        assert_eq!(season(1).unwatched, 6);
        let season_id = ItemId("series-0-s2".into());
        assert_eq!(
            server.state.lock().unwrap().record.watched,
            [(season_id.clone(), true), (season_id, false)]
        );
    }
}
