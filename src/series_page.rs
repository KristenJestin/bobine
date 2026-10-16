//! What the series page shows and what its buttons start, apart from the window: the episode the
//! main button plays, the one after an episode, the end-of-episode countdown and the labels. The
//! version and track chips and the playback plan are the film page's.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bobine::provider::{
    Episode, ItemId, MediaServer, MediaSourceId, Playback, PlaylistEntry, ProviderError, Season,
    Series, SeriesDetail,
};

use crate::film_page::{self, Carried, Choices, Fact};
use crate::shell::Loadable;

/// How long the "Next episode" card counts down before the next episode plays.
pub const COUNTDOWN: Duration = Duration::from_secs(10);

/// An end this close to the duration is the end of the episode; further, the stream was cut.
const END_WITHIN: f64 = 10.;

/// `S2 · E4`, when the server numbers the episode.
pub fn code(episode: &Episode) -> Option<String> {
    Some(format!(
        "S{} · E{}",
        episode.season_number?, episode.number?
    ))
}

/// `S2 · E4 Resume from 12m` once started, else `Play S1 · E1`, after a play icon.
pub fn play_label(episode: &Episode) -> String {
    let play = film_page::play_label(&episode.item);
    match (code(episode), episode.item.resume) {
        (None, _) => play,
        (Some(code), Some(_)) => format!("{code} {play}"),
        (Some(code), None) => format!("{play} {code}"),
    }
}

/// A season's tab: `Season 2`, `Specials` for season 0, else its name.
pub fn season_label(season: &Season) -> String {
    match season.number {
        Some(0) => "Specials".into(),
        Some(n) => format!("Season {n}"),
        None => season.name.clone(),
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

/// The line under a series' name on a card: the years, or in Recently added (`fresh`), for a
/// season the server handed in its place, which season is new.
pub fn card_detail(series: &Series, fresh: bool) -> Option<String> {
    match &series.season {
        Some(season) if fresh => Some(format!("{} · new", season_label(season))),
        _ => years(series),
    }
}

/// The season a series page opens on: the one asked for (from a card of a new season), else
/// the one of the episode to play next, else the first.
pub fn opening_season(
    seasons: &[Season],
    wanted: Option<&ItemId>,
    next: Option<&Episode>,
) -> usize {
    let of_next = next.map(|next| &next.season);
    wanted
        .into_iter()
        .chain(of_next)
        .find_map(|id| seasons.iter().position(|s| &s.id == id))
        .unwrap_or(0)
}

/// The line under the title: years, rating, seasons, genres, age rating.
pub fn meta(detail: &SeriesDetail, seasons: usize) -> Vec<Fact> {
    let series = &detail.series;
    let mut before: Vec<String> = Vec::new();
    before.extend(years(series));
    before.extend(
        series
            .community_rating
            .map(|rating| format!("★ {rating:.1}")),
    );
    if seasons > 0 {
        let noun = if seasons > 1 { "seasons" } else { "season" };
        before.push(format!("{seasons} {noun}"));
    }
    Fact::line(before, &detail.genres, detail.official_rating.clone())
}

/// `48m · 18m left`: the runtime, and what is left once started.
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

/// What the player shows: the series, then `S2 · E4 · Le Signal`.
pub fn player_title(episode: &Episode) -> (String, String) {
    let subtitle = code(episode)
        .into_iter()
        .chain([episode.item.title.clone()])
        .collect::<Vec<_>>()
        .join(" · ");
    (episode.series_title.clone(), subtitle)
}

/// The episode the main button plays: "Next up" (the one started, else the one after the last
/// watched), else the first of the series, watched again: of its first season, not of its
/// specials (season 0), unless it has nothing else. Blocks on the server.
pub fn next_to_play(
    server: &dyn MediaServer,
    series: &ItemId,
) -> Result<Option<Episode>, ProviderError> {
    if let Some(next) = server.next_up(series)? {
        return Ok(Some(next));
    }
    let seasons = server.seasons(series)?;
    let Some(first) = seasons
        .iter()
        .find(|s| s.number.is_some_and(|n| n >= 1))
        .or(seasons.first())
    else {
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

/// Whether a season's episodes are asked of the server: when never asked, when they failed (its
/// tab chosen again tries again), or `again`.
pub fn asks_episodes(shown: Option<&Loadable<Vec<Episode>>>, again: bool) -> bool {
    again || matches!(shown, None | Some(Loadable::Failed(_)))
}

/// The episodes of `season` came back for `refresh`. `own`: it is the refresh's own question,
/// the only one whose answer releases the hold of a mark; a tab click asks under the newest
/// refresh too, and may land before it.
pub fn land_episodes(
    episodes: &mut HashMap<ItemId, Loadable<Vec<Episode>>>,
    refreshes: &mut Refreshes,
    refresh: u64,
    own: bool,
    season: ItemId,
    answer: Loadable<Vec<Episode>>,
) {
    let shown = if own {
        refreshes.landed(refresh)
    } else {
        refreshes.is_newest(refresh)
    };
    if shown {
        episodes.insert(season, answer);
    } else if matches!(episodes.get(&season), Some(Loadable::Loading)) {
        // Nothing newer may come for this season (the refresh can fail): forgotten, the next
        // click on its tab asks again. A list already shown stays drawn.
        episodes.remove(&season);
    }
}

/// A click on an episode, or the card's own countdown, starts it: whether it can, the page not
/// waiting. Only then does another episode chosen take the card away; a click the page ignores
/// leaves it counting down.
pub fn starts_episode<T>(waiting: bool, card: &mut UpNext<T>) -> bool {
    if waiting {
        return false;
    }
    card.cancel();
    true
}

/// The chips' choices once the episode the main button plays is known anew: kept while it stays
/// the same, else that episode's defaults.
pub fn choices_after(
    shown: Option<&Episode>,
    chosen: Option<Choices>,
    fresh: Option<&Episode>,
    carried: &Carried,
) -> Option<Choices> {
    let fresh = fresh?;
    match chosen {
        Some(chosen) if shown.is_some_and(|shown| shown.item.id == fresh.item.id) => Some(chosen),
        _ => Some(Choices::carrying(&fresh.versions, carried.clone())),
    }
}

/// What `episode` plays with: the chips' choices when it is the episode they were made for, else
/// its defaults.
pub fn choices_for(
    next: Option<&Episode>,
    chosen: Option<&Choices>,
    episode: &Episode,
    carried: &Carried,
) -> Choices {
    match (next, chosen) {
        (Some(next), Some(chosen)) if next.item.id == episode.item.id => chosen.clone(),
        _ => Choices::carrying(&episode.versions, carried.clone()),
    }
}

/// What playing an episode asks of the server, off the window's thread: the playback, and the
/// episode after it, kept for the card at its end. Without that one the end shows no card; the
/// playback goes on all the same.
pub fn open_episode(
    server: &dyn MediaServer,
    episode: &Episode,
    source: &MediaSourceId,
) -> (Result<Playback, ProviderError>, Option<Episode>) {
    let opened = server.play(&episode.item.id, source);
    (opened, following(server, episode).ok().flatten())
}

/// The episode the card offers once the player is left: the one after, only when the file ended
/// at its real end; none when the viewer left, or when the stream was cut.
pub fn offered_on_leaving<T>(
    ended: bool,
    next: Option<T>,
    position: f64,
    duration: f64,
) -> Option<T> {
    next.filter(|_| ended && reached_the_end(position, duration))
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

/// The refreshes of a series page: each is numbered when asked, and only the answers of the
/// newest are shown, so that an older one landing late does not overwrite it. After a mark, the
/// page holds until a refresh lands, so that a second click does not act on the list from
/// before the mark.
#[derive(Default)]
pub struct Refreshes {
    asked: u64,
    holding: bool,
}

impl Refreshes {
    /// A new refresh: its number, taken from `counter`, which counts over every page, so that
    /// no two pages ever share one.
    pub fn ask(&mut self, counter: &mut u64) -> u64 {
        *counter += 1;
        self.asked = *counter;
        self.asked
    }

    /// The newest refresh, the one whose answers are shown.
    pub fn newest(&self) -> u64 {
        self.asked
    }

    pub fn is_newest(&self, refresh: u64) -> bool {
        refresh == self.asked
    }

    /// A mark reached the server: the page holds until a refresh asked from now lands.
    pub fn hold(&mut self) {
        self.holding = true;
    }

    /// The last answer of `refresh` landed: whether it is shown. The newest releases the hold.
    pub fn landed(&mut self, refresh: u64) -> bool {
        let newest = self.is_newest(refresh);
        if newest {
            self.holding = false;
        }
        newest
    }

    pub fn holding(&self) -> bool {
        self.holding
    }
}

/// What the card offers once a playback ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// The episode after, on its series page.
    Episode(Episode),
    /// The entry after in the playlist being played: the card plays the one it shows, wherever
    /// the page moved it since.
    Entry(PlaylistEntry),
}

/// The "Next episode" card: counts down, then gives the episode to play, unless cancelled.
/// It belongs to the page it showed over: a series page, or a playlist's, at its depth among
/// those pages, with what it shows.
pub struct UpNext<T = Episode> {
    pending: Option<(T, Instant)>,
    page: Option<(usize, ItemId)>,
}

impl<T> Default for UpNext<T> {
    fn default() -> Self {
        Self {
            pending: None,
            page: None,
        }
    }
}

impl<T> UpNext<T> {
    pub fn start(&mut self, episode: T, page: (usize, ItemId), now: Instant) {
        self.pending = Some((episode, now));
        self.page = Some(page);
    }

    pub fn cancel(&mut self) {
        self.pending = None;
        self.page = None;
    }

    pub fn episode(&self) -> Option<&T> {
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

    /// The episode to play now: once the countdown is over, or at once when `now_asked`. `top`:
    /// the series page shown, `None` on another screen; the card is cancelled when it is no
    /// longer its page. While `busy`, the page cannot play yet: the card waits.
    pub fn take(
        &mut self,
        now: Instant,
        now_asked: bool,
        top: Option<(usize, &ItemId)>,
        busy: bool,
    ) -> Option<T> {
        let (_, since) = self.pending.as_ref()?;
        let page = self.page.as_ref().map(|(depth, id)| (*depth, id));
        if top != page {
            self.cancel();
            return None;
        }
        if busy || (!now_asked && now < *since + COUNTDOWN) {
            return None;
        }
        self.page = None;
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

    fn seasons() -> Vec<Season> {
        (1..=3)
            .map(|number| Season {
                id: ItemId(format!("season-{number}")),
                name: format!("Season {number}"),
                number: Some(number),
                unwatched: 0,
            })
            .collect()
    }

    fn episode_of(season: &str, server: &FakeServer) -> Episode {
        Episode {
            season: ItemId(season.into()),
            ..episode(server, 1, 1)
        }
    }

    #[test]
    fn a_series_page_opens_on_the_season_asked_for() {
        let server = FakeServer::new(1, true);
        let next = episode_of("season-1", &server);
        let wanted = ItemId("season-3".into());
        assert_eq!(opening_season(&seasons(), Some(&wanted), Some(&next)), 2);
    }

    #[test]
    fn a_series_page_opens_on_the_season_of_the_next_episode_when_none_is_asked() {
        let server = FakeServer::new(1, true);
        let next = episode_of("season-2", &server);
        assert_eq!(opening_season(&seasons(), None, Some(&next)), 1);
    }

    #[test]
    fn a_series_page_opens_on_the_first_season_when_the_one_asked_for_is_gone() {
        let wanted = ItemId("season-9".into());
        assert_eq!(opening_season(&seasons(), Some(&wanted), None), 0);
    }

    #[test]
    fn a_season_card_says_which_season_is_new_in_recently_added_only() {
        let mut series = crate::fake::series().remove(0);
        series.season = Some(seasons().remove(2));
        assert_eq!(
            card_detail(&series, true).as_deref(),
            Some("Season 3 · new")
        );
        assert_eq!(card_detail(&series, false), years(&series));
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
    fn a_series_watched_to_the_end_starts_again_from_its_first_season_not_its_specials() {
        let server = FakeServer::new(1, true);
        let with_specials = ItemId("series-1".into());
        let seasons = server.seasons(&with_specials).unwrap();
        assert_eq!(seasons[0].number, Some(0));
        server
            .state
            .lock()
            .unwrap()
            .episodes
            .iter_mut()
            .for_each(|e| e.item.watched = true);
        let next = next_to_play(&server, &with_specials).unwrap().unwrap();
        assert_eq!(next.item.id, ItemId("series-1-s1-e1".into()));
    }

    #[test]
    fn the_specials_tab_says_specials() {
        let server = FakeServer::new(1, true);
        let seasons = server.seasons(&ItemId("series-1".into())).unwrap();
        assert_eq!(season_label(&seasons[0]), "Specials");
        assert_eq!(season_label(&seasons[1]), "Season 1");
        let mut unnumbered = seasons[1].clone();
        unnumbered.number = None;
        unnumbered.name = "Hors-série".into();
        assert_eq!(season_label(&unnumbered), "Hors-série");
    }

    #[test]
    fn the_play_button_names_the_next_episode_and_where_it_resumes() {
        let server = FakeServer::new(1, true);
        assert_eq!(
            play_label(&episode(&server, 2, 4)),
            "S2 · E4 Resume from 30m"
        );
        assert_eq!(play_label(&episode(&server, 3, 1)), "Play S3 · E1");
        let mut unnumbered = episode(&server, 3, 1);
        unnumbered.number = None;
        assert_eq!(play_label(&unnumbered), "Play");
    }

    #[test]
    fn the_series_page_says_years_rating_seasons_and_genres() {
        let server = FakeServer::new(1, true);
        let detail = server.series_detail(&series()).unwrap();
        let text = |text: &str| Fact::Text(text.into());
        assert_eq!(
            meta(&detail, 3),
            [
                text("2023 – 2025"),
                text("★ 6.5"),
                text("3 seasons"),
                Fact::Genres(vec!["Science-fiction".into(), "Drame".into()]),
                text("12+")
            ]
        );
        assert_eq!(meta(&detail, 1)[2], text("1 season"));
        let mut ongoing = detail.series.clone();
        ongoing.end_year = None;
        assert_eq!(years(&ongoing).as_deref(), Some("2023"));
    }

    #[test]
    fn an_episode_line_says_its_runtime_and_what_is_left() {
        let server = FakeServer::new(1, true);
        assert_eq!(duration_line(&episode(&server, 2, 4)), "48m · 18m left");
        assert_eq!(duration_line(&episode(&server, 3, 1)), "48m");
    }

    #[test]
    fn a_series_poster_shows_its_unwatched_episodes() {
        let server = FakeServer::new(1, true);
        let page = server
            .series(
                &ItemId("series".into()),
                &crate::library_tools::Tools::default().query(0),
            )
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
        assert_eq!(subtitle, "S2 · E4 · Le Signal");
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
                info: player.info.clone(),
                ..Default::default()
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
                method: bobine::provider::PlayMethod::DirectPlay,
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

    /// The series page the card shows over: the first, of the series of the tests.
    fn page() -> (usize, ItemId) {
        (0, series())
    }

    fn top() -> Option<(usize, &'static ItemId)> {
        static SERIES_ID: std::sync::LazyLock<ItemId> =
            std::sync::LazyLock::new(|| ItemId(SERIES.into()));
        Some((0, &SERIES_ID))
    }

    #[test]
    fn auto_next_plays_the_next_episode_after_ten_seconds() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        assert_eq!(card.seconds_left(now), 10);
        assert_eq!(card.seconds_left(now + Duration::from_millis(9500)), 1);
        assert_eq!(card.fraction_left(now), 1.);
        assert_eq!(card.fraction_left(now + Duration::from_secs(5)), 0.5);
        assert_eq!(
            card.take(now + Duration::from_secs(9), false, top(), false),
            None
        );
        assert!(card.episode().is_some());
        let played = card.take(now + COUNTDOWN, false, top(), false).unwrap();
        assert_eq!(played.item.id, id(2, 5));
        assert!(card.episode().is_none());
        assert_eq!(card.take(now + COUNTDOWN * 2, false, top(), false), None);
    }

    #[test]
    fn auto_next_plays_at_once_when_asked() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        assert_eq!(
            card.take(now, true, top(), false).unwrap().item.id,
            id(2, 5)
        );
    }

    #[test]
    fn auto_next_is_cancelled() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        card.cancel();
        assert!(card.episode().is_none());
        assert_eq!(card.take(now + COUNTDOWN * 2, false, top(), false), None);
    }

    #[test]
    fn auto_next_is_cancelled_when_another_series_page_opens() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        let other = ItemId("series-1".into());
        // A similar series opened over the page: the card does not play there, now or later.
        assert_eq!(
            card.take(now + COUNTDOWN, false, Some((1, &other)), false),
            None
        );
        assert!(card.episode().is_none());
        assert_eq!(card.take(now + COUNTDOWN, true, top(), false), None);
    }

    #[test]
    fn auto_next_is_cancelled_when_its_page_is_left() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        assert_eq!(card.take(now, false, None, false), None);
        assert!(card.episode().is_none());
    }

    #[test]
    fn auto_next_waits_while_its_page_is_busy() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        assert_eq!(card.take(now + COUNTDOWN, false, top(), true), None);
        assert_eq!(card.take(now, true, top(), true), None);
        // Kept, and played once the page is free.
        assert!(card.episode().is_some());
        let played = card.take(now + COUNTDOWN * 2, false, top(), false).unwrap();
        assert_eq!(played.item.id, id(2, 5));
    }

    #[test]
    fn a_click_while_the_page_waits_keeps_the_card() {
        let server = FakeServer::new(1, true);
        let now = Instant::now();
        let mut card = UpNext::default();
        card.start(episode(&server, 2, 5), page(), now);
        assert!(!starts_episode(true, &mut card));
        assert_eq!(card.episode().map(|e| e.item.id.clone()), Some(id(2, 5)));
        // Once the page is free, another episode chosen takes the card away.
        assert!(starts_episode(false, &mut card));
        assert!(card.episode().is_none());
    }

    #[test]
    fn an_older_answer_does_not_overwrite_a_newer_one() {
        let (mut refreshes, mut counter) = (Refreshes::default(), 0);
        let first = refreshes.ask(&mut counter);
        let second = refreshes.ask(&mut counter);
        assert_ne!(first, second);
        assert_eq!(refreshes.newest(), second);
        // The second lands first; the first, late, is dropped.
        assert!(refreshes.is_newest(second));
        assert!(refreshes.landed(second));
        assert!(!refreshes.is_newest(first));
        assert!(!refreshes.landed(first));
    }

    #[test]
    fn an_answer_for_a_closed_page_never_reaches_a_reopened_one() {
        let mut counter = 0;
        // The page is asked, closed, and the same series opened again at the same depth.
        let mut closed = Refreshes::default();
        let old = closed.ask(&mut counter);
        let mut reopened = Refreshes::default();
        let new = reopened.ask(&mut counter);
        assert_ne!(old, new);
        // The old answer lands late on the reopened page: it is not its answer.
        assert!(!reopened.is_newest(old));
        assert!(!reopened.landed(old));
        assert!(reopened.landed(new));
    }

    #[test]
    fn a_mark_holds_the_page_until_the_refresh_after_it_lands() {
        let (mut refreshes, mut counter) = (Refreshes::default(), 0);
        let before = refreshes.ask(&mut counter);
        assert!(!refreshes.holding());
        refreshes.hold();
        assert!(refreshes.holding());
        let after = refreshes.ask(&mut counter);
        // A refresh asked before the mark is older: it does not release.
        assert!(!refreshes.landed(before));
        assert!(refreshes.holding());
        assert!(refreshes.landed(after));
        assert!(!refreshes.holding());
    }

    fn some_episodes() -> Loadable<Vec<Episode>> {
        Loadable::Ready(Vec::new())
    }

    #[test]
    fn only_the_refresh_releases_the_hold() {
        let (mut refreshes, mut counter) = (Refreshes::default(), 0);
        let mut episodes = HashMap::new();
        let season = ItemId("season-2".into());
        // A mark landed: the page holds, and the refresh after it is asked.
        refreshes.ask(&mut counter);
        refreshes.hold();
        let refresh = refreshes.ask(&mut counter);
        // A tab click's answer, for the newest refresh, lands first: it shows, the hold stays.
        land_episodes(
            &mut episodes,
            &mut refreshes,
            refresh,
            false,
            season.clone(),
            some_episodes(),
        );
        assert!(matches!(episodes.get(&season), Some(Loadable::Ready(_))));
        assert!(refreshes.holding());
        // The refresh's own answer releases it.
        land_episodes(
            &mut episodes,
            &mut refreshes,
            refresh,
            true,
            season,
            some_episodes(),
        );
        assert!(!refreshes.holding());
    }

    #[test]
    fn a_season_left_loading_by_a_dropped_answer_is_asked_again() {
        let (mut refreshes, mut counter) = (Refreshes::default(), 0);
        let mut episodes = HashMap::new();
        let season = ItemId("season-2".into());
        // A mark lands while another season's tab is clicked: the tab's episodes are asked
        // under refresh N, then the page holds and asks N + 1.
        let stale = refreshes.ask(&mut counter);
        episodes.insert(season.clone(), Loadable::Loading);
        refreshes.hold();
        refreshes.ask(&mut counter);
        // The tab's answer is dropped, and N + 1 then fails: nothing is left to fill the tab.
        land_episodes(
            &mut episodes,
            &mut refreshes,
            stale,
            false,
            season.clone(),
            some_episodes(),
        );
        assert!(asks_episodes(episodes.get(&season), false));
    }

    #[test]
    fn a_dropped_answer_leaves_the_list_shown_drawn() {
        let (mut refreshes, mut counter) = (Refreshes::default(), 0);
        let mut episodes = HashMap::new();
        let season = ItemId("season-2".into());
        let stale = refreshes.ask(&mut counter);
        episodes.insert(season.clone(), some_episodes());
        refreshes.ask(&mut counter);
        land_episodes(
            &mut episodes,
            &mut refreshes,
            stale,
            false,
            season.clone(),
            Loadable::Failed("The server is not responding."),
        );
        assert!(matches!(episodes.get(&season), Some(Loadable::Ready(_))));
    }

    #[test]
    fn a_failed_season_is_asked_again_when_its_tab_is_chosen() {
        assert!(asks_episodes(None, false));
        assert!(asks_episodes(
            Some(&Loadable::Failed("The server is not responding.")),
            false
        ));
        assert!(!asks_episodes(Some(&Loadable::Loading), false));
        assert!(!asks_episodes(Some(&Loadable::Ready(Vec::new())), false));
        assert!(asks_episodes(Some(&Loadable::Ready(Vec::new())), true));
    }

    #[test]
    fn playing_an_episode_keeps_the_next_one_for_its_end() {
        let server = FakeServer::new(1, true);
        let playing = episode(&server, 2, 6);
        let (opened, next) = open_episode(&server, &playing, &playing.versions[0].id);
        assert_eq!(opened.unwrap().film, id(2, 6));
        assert_eq!(next.unwrap().item.id, id(3, 1));
        let (_, none) = open_episode(&server, &episode(&server, 3, 5), &playing.versions[0].id);
        assert_eq!(none, None);
    }

    #[test]
    fn the_card_starts_after_the_player_closes_at_the_end() {
        let server = FakeServer::new(1, true);
        let next = Some(episode(&server, 2, 5));
        let offered = offered_on_leaving(true, next, 48. * 60. - 1., 48. * 60.);
        assert_eq!(offered.unwrap().item.id, id(2, 5));
        // A cut stream ends the file too, short of the end: no card.
        let next = Some(episode(&server, 2, 5));
        assert_eq!(offered_on_leaving(true, next, 40. * 60., 48. * 60.), None);
    }

    #[test]
    fn no_card_when_the_player_is_left_before_the_end() {
        let server = FakeServer::new(1, true);
        let next = Some(episode(&server, 2, 5));
        assert_eq!(
            offered_on_leaving(false, next, 48. * 60. - 1., 48. * 60.),
            None
        );
    }

    #[test]
    fn the_chips_apply_to_the_episode_about_to_play() {
        let server = FakeServer::new(1, true);
        let (next, other) = (episode(&server, 2, 4), episode(&server, 2, 5));
        let mut chosen = Choices::new(&next.versions);
        crate::shell::Chip::Audio.choose(&mut chosen, &next.versions, Some(7));
        assert_ne!(chosen, Choices::new(&next.versions));
        let none = Carried::default();
        assert_eq!(
            choices_for(Some(&next), Some(&chosen), &next, &none),
            chosen
        );
        // Another episode of the list plays with its own defaults.
        assert_eq!(
            choices_for(Some(&next), Some(&chosen), &other, &none),
            Choices::new(&other.versions)
        );
    }

    #[test]
    fn the_chips_keep_their_choice_while_the_next_episode_stays_the_same() {
        let server = FakeServer::new(1, true);
        let (next, other) = (episode(&server, 2, 4), episode(&server, 2, 5));
        let mut chosen = Choices::new(&next.versions);
        crate::shell::Chip::Audio.choose(&mut chosen, &next.versions, Some(7));
        let mut refreshed = next.clone();
        refreshed.item.resume = Some(Duration::from_secs(31 * 60));
        let none = Carried::default();
        assert_eq!(
            choices_after(Some(&next), Some(chosen.clone()), Some(&refreshed), &none),
            Some(chosen.clone())
        );
        assert_eq!(
            choices_after(Some(&next), Some(chosen), Some(&other), &none),
            Some(Choices::new(&other.versions))
        );
        assert_eq!(
            choices_after(None, None, Some(&next), &none),
            Some(Choices::new(&next.versions))
        );
        assert_eq!(choices_after(Some(&next), None, None, &none), None);
    }

    /// An episode of the fake server, in the film's versions: several audio and subtitle tracks.
    fn with_tracks(server: &FakeServer, number: u32) -> Episode {
        let mut episode = episode(server, 2, number);
        episode.versions = crate::fake::versions();
        episode
    }

    #[test]
    fn a_hand_picked_language_is_carried_to_the_next_episode() {
        let server = FakeServer::new(1, true);
        let (played, after) = (with_tracks(&server, 4), with_tracks(&server, 5));
        let mut chosen = Choices::new(&played.versions);
        // French audio and the English subtitles, where the server chose English audio and the
        // full French subtitles.
        crate::shell::Chip::Audio.choose(&mut chosen, &played.versions, Some(1));
        crate::shell::Chip::Subtitle.choose(&mut chosen, &played.versions, Some(5));
        let carried = chosen.carried();
        // The card at the end, or a play from the page.
        let next = choices_for(None, None, &after, &carried);
        assert_eq!((next.audio, next.subtitle), (Some(1), Some(5)));
        // The main button once the page is back.
        let next = choices_after(Some(&played), Some(chosen), Some(&after), &carried).unwrap();
        assert_eq!((next.audio, next.subtitle), (Some(1), Some(5)));
    }

    #[test]
    fn the_next_episode_without_that_language_takes_the_servers_indexes() {
        let server = FakeServer::new(1, true);
        let (played, mut after) = (with_tracks(&server, 4), with_tracks(&server, 5));
        let mut chosen = Choices::new(&played.versions);
        crate::shell::Chip::Audio.choose(&mut chosen, &played.versions, Some(1));
        crate::shell::Chip::Subtitle.choose(&mut chosen, &played.versions, Some(5));
        // The episode after has neither French audio nor English subtitles.
        let source = &mut after.versions[0];
        source.audio.retain(|track| track.index != 1);
        source.subtitles.retain(|track| track.index != 5);
        let next = choices_for(None, None, &after, &chosen.carried());
        assert_eq!((next.audio, next.subtitle), (Some(2), Some(4)));
    }

    #[test]
    fn tracks_the_server_chose_are_not_carried() {
        let server = FakeServer::new(1, true);
        let (played, mut after) = (with_tracks(&server, 4), with_tracks(&server, 5));
        let chosen = Choices::new(&played.versions);
        // The server chooses otherwise for the episode after.
        after.versions[0].default_audio = Some(1);
        after.versions[0].default_subtitle = Some(None);
        let next = choices_for(None, None, &after, &chosen.carried());
        assert_eq!((next.audio, next.subtitle), (Some(1), None));
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
