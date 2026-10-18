//! What the Home shows and what its buttons start, apart from the window: the lists of its rows,
//! what each card says, and the playback a "Play" opens for a film, a series or an episode.
//! The playback itself, its reports and its end are the film page's and the series page's.
//! A downloaded title plays from its file (`start_download`).

use std::sync::Arc;

use bobine::provider::{
    Episode, Feature, FeatureKind, FilmQuery, FilmSort, ItemId, LibraryKind, MediaServer, Page,
    PlaySessionId, Playback, ProviderError, Title,
};

use crate::downloads::{Downloads, Local, Snapshot};
use crate::fallback;
use crate::film_page::{self, Choices, Plan};
use crate::grid;
use crate::library_tools::Tools;
use crate::player::settings::Quality;
use crate::reports::To;
use crate::series_page;

/// What a play button of the Home starts.
#[derive(Debug, Clone, PartialEq)]
pub enum PlayTarget {
    /// A film, from where the viewer stopped.
    Film(ItemId),
    /// A series: its next episode.
    Series(ItemId),
    /// An episode a card shows, from where the viewer stopped.
    Episode(Box<Episode>),
}

impl PlayTarget {
    /// The button on a card of a row: the film, the episode, or the next episode of the series.
    pub fn of(title: &Title) -> Self {
        match title {
            Title::Film(film) => Self::Film(film.id.clone()),
            Title::Series(series) => Self::Series(series.id.clone()),
            Title::Episode(episode) => Self::Episode(Box::new(episode.clone())),
        }
    }
}

/// What the "Play" under the header's title starts.
pub fn play_target(feature: &Feature) -> PlayTarget {
    match feature.kind {
        FeatureKind::Film => PlayTarget::Film(feature.id.clone()),
        FeatureKind::Series => PlayTarget::Series(feature.id.clone()),
    }
}

/// Why nothing started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    Server(ProviderError),
    /// No version of the film, no episode in the series: nothing to open.
    NothingToPlay,
}

impl std::convert::From<ProviderError> for StartError {
    fn from(error: ProviderError) -> Self {
        Self::Server(error)
    }
}

/// What the player is given, once the server has opened the playback.
pub struct Started {
    pub plan: Plan,
    /// What the player shows: the title, and the line under it.
    pub title: String,
    pub subtitle: String,
    /// What the player's "My list" button puts in the list: the film, or the series.
    pub list: ItemId,
    /// What the desktop's media controls show.
    pub desktop: crate::mpris::Title,
    pub opened: Result<Playback, ProviderError>,
}

/// Opens the playback of `target`, off the window's thread: it blocks on the server.
pub fn start(
    server: &dyn MediaServer,
    target: &PlayTarget,
    quality: Quality,
) -> Result<Started, StartError> {
    start_from(server, target, film_page::From::Resume, quality)
}

/// `start`, from where the viewer stopped or from the beginning.
pub fn start_from(
    server: &dyn MediaServer,
    target: &PlayTarget,
    from: film_page::From,
    quality: Quality,
) -> Result<Started, StartError> {
    match target {
        PlayTarget::Film(film) => {
            let detail = server.film(film)?;
            let choices = Choices::new(&detail.versions);
            let plan = film_page::plan(&detail.film, &detail.versions, &choices, from)
                .ok_or(StartError::NothingToPlay)?;
            let mut parts: Vec<String> = detail.film.year.iter().map(i32::to_string).collect();
            parts.extend(
                choices
                    .source(&detail.versions)
                    .map(film_page::version_label),
            );
            parts.push("direct play".into());
            Ok(Started {
                title: plan.title.clone(),
                subtitle: parts.join(" · "),
                list: plan.film.clone(),
                desktop: crate::mpris::Title::film(&detail.film),
                opened: fallback::open_first(server, &plan, quality),
                plan,
            })
        }
        PlayTarget::Series(series) => {
            let episode =
                series_page::next_to_play(server, series)?.ok_or(StartError::NothingToPlay)?;
            start_from(
                server,
                &PlayTarget::Episode(Box::new(episode)),
                from,
                quality,
            )
        }
        PlayTarget::Episode(episode) => {
            let choices = Choices::new(&episode.versions);
            let plan = film_page::plan(&episode.item, &episode.versions, &choices, from)
                .ok_or(StartError::NothingToPlay)?;
            let (title, mut subtitle) = series_page::player_title(episode);
            if let Some(source) = choices.source(&episode.versions) {
                subtitle.push_str(&format!(" · {}", film_page::version_label(source)));
            }
            // No card for the episode after: it counts down over a series page, and the Home has
            // none. The Home shows it under "Next up" when it is back.
            Ok(Started {
                opened: fallback::open_first(server, &plan, quality),
                plan,
                title,
                subtitle,
                list: episode.series.clone(),
                desktop: crate::mpris::Title::episode(episode),
            })
        }
    }
}

/// The playback session of a download played while the server cannot be reached: the journal
/// does not keep it.
const OFFLINE_SESSION: &str = "offline";

/// Whether the server can be reached for the playback of a download.
#[allow(
    dead_code,
    reason = "played from the shell once offline mode (#137) wires downloads in"
)]
pub enum Reach {
    Online(Arc<dyn MediaServer>),
    /// Nothing is asked of the server.
    Offline,
}

/// A download's playback, ready for the player: where its reports go, and the download, whose
/// segments and thumbnails the player shows.
#[allow(
    dead_code,
    reason = "played from the shell once offline mode (#137) wires downloads in"
)]
pub struct StartedDownload {
    pub started: Started,
    pub reports: To,
    pub local: Local,
}

/// Opens the playback of the download of `item` from its file. Online, the server opens a playback
/// session for it as for any other and hears its reports; offline, the journal does.
#[allow(
    dead_code,
    reason = "played from the shell once offline mode (#137) wires downloads in"
)]
pub fn start_download(
    downloads: &Downloads,
    item: &ItemId,
    reach: Reach,
    from: film_page::From,
) -> Result<StartedDownload, StartError> {
    let local = downloads.local(item).ok_or(StartError::NothingToPlay)?;
    let (film, versions) = match &local.manifest.title {
        Snapshot::Film(detail) => (&detail.film, &detail.versions),
        Snapshot::Episode(episode, _) => (&episode.item, &episode.versions),
    };
    // Where the viewer stopped: as the server holds it, else as the last offline play left it,
    // else as it was when downloaded.
    let mut film = film.clone();
    if let Some(play) = downloads
        .journal()
        .plays()
        .unwrap()
        .iter()
        .find(|p| &p.item == item)
    {
        film.resume = Some(play.position).filter(|position| !position.is_zero());
    }
    if let Reach::Online(server) = &reach {
        match server.title(item) {
            Ok(Title::Film(fresh)) => film.resume = fresh.resume,
            Ok(Title::Episode(fresh)) => film.resume = fresh.item.resume,
            _ => {}
        }
    }
    let mut choices = Choices::new(versions);
    if let Some(version) = versions.iter().position(|v| v.id == local.manifest.source) {
        choices.set_version(versions, version);
    }
    let plan = film_page::plan(&film, versions, &choices, from).ok_or(StartError::NothingToPlay)?;
    let version = choices.source(versions).map(film_page::version_label);
    let (title, subtitle, list, desktop) = match &local.manifest.title {
        Snapshot::Film(detail) => {
            let mut parts: Vec<String> = film.year.iter().map(i32::to_string).collect();
            parts.extend(version);
            parts.push("downloaded".into());
            let desktop = crate::mpris::Title::film(&detail.film);
            (
                plan.title.clone(),
                parts.join(" · "),
                film.id.clone(),
                desktop,
            )
        }
        Snapshot::Episode(episode, _) => {
            let (title, mut subtitle) = series_page::player_title(episode);
            if let Some(version) = version {
                subtitle.push_str(&format!(" · {version}"));
            }
            let desktop = crate::mpris::Title::episode(episode);
            (title, subtitle, episode.series.clone(), desktop)
        }
    };
    let (opened, reports) = match reach {
        Reach::Online(server) => (
            server
                .play(item, &local.manifest.source)
                .map(|opened| local.playback(opened.session)),
            To::Server(server),
        ),
        Reach::Offline => (
            Ok(local.playback(PlaySessionId(OFFLINE_SESSION.into()))),
            To::Journal(downloads.journal()),
        ),
    };
    Ok(StartedDownload {
        started: Started {
            plan,
            title,
            subtitle,
            list,
            desktop,
            opened,
        },
        reports,
        local,
    })
}

/// The rows of the Home that list titles, and so have a full grid behind "See all".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListKind {
    /// Films and episodes started.
    Resume,
    /// The next episode of each series in progress.
    NextUp,
    /// What was added to one library lately.
    Latest(ItemId, LibraryKind),
}

impl ListKind {
    pub fn heading(&self) -> &'static str {
        match self {
            Self::Resume => "Continue watching",
            Self::NextUp => "Next up",
            Self::Latest(..) => "Recently added",
        }
    }

    /// Why the full list holds nothing, in one line.
    pub fn empty_reason(&self) -> &'static str {
        match self {
            Self::Resume => "Nothing started yet.",
            Self::NextUp => "No series in progress, so no next episode yet.",
            Self::Latest(..) => "Nothing was added to this library lately.",
        }
    }
}

/// Which of the Home's rows `rows` is offered on an empty list: what was started when there is
/// some, otherwise what a library gained; never `empty`, the full list that holds nothing (none
/// on My list and Collections, which are not rows of the Home).
pub fn suggested(empty: Option<&ListKind>, rows: &[ListKind]) -> Option<usize> {
    let offered = |wanted: fn(&ListKind) -> bool| {
        rows.iter()
            .position(|row| Some(row) != empty && wanted(row))
    };
    offered(|row| *row == ListKind::Resume)
        .or_else(|| offered(|row| matches!(row, ListKind::Latest(..))))
}

/// How many times the Home was asked: an answer to an older ask comes late, and is dropped.
#[derive(Debug, Default)]
pub struct Asks(u64);

impl Asks {
    /// A new ask: the one the answers must be of.
    pub fn next(&mut self) -> u64 {
        self.0 += 1;
        self.0
    }

    pub fn is_latest(&self, ask: u64) -> bool {
        self.0 == ask
    }
}

/// How many titles a row of the Home holds; "See all" shows them all.
pub const ROW: u32 = 16;

/// The most that "Continue watching" and "Next up" hold in their full list.
const WHOLE_LIST: u32 = grid::PAGE;

/// One page of the full list behind "See all". Resume and next up come whole in the first
/// page (a person has a few dozen at most); the latest of a library are paged like its grid.
pub fn fetch(
    server: &dyn MediaServer,
    kind: &ListKind,
    page: u32,
) -> Result<Page<Title>, ProviderError> {
    let whole = |titles: Vec<Title>| {
        let total = titles.len() as u32;
        Page {
            items: if page == 0 { titles } else { Vec::new() },
            total,
        }
    };
    match kind {
        ListKind::Resume => Ok(whole(server.continue_watching()?)),
        ListKind::NextUp => {
            let started = server.continue_watching()?;
            let next = next_up_row(server.next_episodes(WHOLE_LIST)?, &started);
            Ok(whole(next.into_iter().map(Title::Episode).collect()))
        }
        ListKind::Latest(library, LibraryKind::Series) => {
            let found = server.series(library, &latest_query(LibraryKind::Series, page))?;
            Ok(Page {
                items: found.items.into_iter().map(Title::Series).collect(),
                total: found.total,
            })
        }
        ListKind::Latest(library, kind) => {
            let found = server.films(library, &latest_query(*kind, page))?;
            Ok(Page {
                items: found.items.into_iter().map(Title::Film).collect(),
                total: found.total,
            })
        }
    }
}

/// The order of the full list of what a library gained: the one of its row. The row follows
/// when episodes came in, so a series that gained one now is first, not the one added long ago.
pub fn latest_query(kind: LibraryKind, page: u32) -> FilmQuery {
    let query = Tools::default().query(page);
    match kind {
        LibraryKind::Series => FilmQuery {
            sort: FilmSort::LastAdded,
            ..query
        },
        _ => query,
    }
}

/// What "Next up" lists: the next episodes, less those already under "Continue watching"
/// (the server counts an episode started as the next of its series).
pub fn next_up_row(next: Vec<Episode>, started: &[Title]) -> Vec<Episode> {
    next.into_iter()
        .filter(|episode| !started.iter().any(|title| title.id() == &episode.item.id))
        .collect()
}

/// What a card writes under its picture: the title, then a line of detail.
pub fn card_lines(title: &Title) -> (String, Option<String>) {
    match title {
        Title::Film(film) => (film.title.clone(), film.year.map(|year| year.to_string())),
        Title::Series(series) => (series.title.clone(), series_page::years(series)),
        Title::Episode(episode) => (
            episode.series_title.clone(),
            Some(
                series_page::code(episode)
                    .into_iter()
                    .chain([episode.item.title.clone()])
                    .collect::<Vec<_>>()
                    .join(" · "),
            ),
        ),
    }
}

/// The film a card shows, or the episode's own item: what has a runtime and a resume point.
fn item(title: &Title) -> Option<&bobine::provider::Film> {
    match title {
        Title::Film(film) => Some(film),
        Title::Episode(episode) => Some(&episode.item),
        Title::Series(_) => None,
    }
}

/// `18m left` on a card of something started.
pub fn time_left(title: &Title) -> Option<String> {
    film_page::time_left(item(title)?)
}

/// How far the viewer went in what the card shows, from 0 to 1.
pub fn progress(title: &Title) -> Option<f32> {
    film_page::progress(item(title)?)
}

/// Which cards of a row are near enough to the window to be drawn with their picture: those in
/// view and one more each side, so that a card scrolled into view is already there. `scrolled`:
/// how far the row went sideways; `inset`: the room before the first card.
pub fn cards_in_view(
    scrolled: f32,
    viewport: f32,
    card: f32,
    gap: f32,
    inset: f32,
    count: usize,
) -> std::ops::Range<usize> {
    let step = card + gap;
    let first = ((scrolled - inset) / step).floor() as i64 - 1;
    let last = ((scrolled + viewport - inset) / step).ceil() as i64 + 1;
    (first.max(0) as usize).min(count)..(last.max(0) as usize).min(count)
}

/// Whether a row of the page is near enough to the window for its pictures to be asked: the
/// page went `scrolled` down, a row stands `top` from its top and is `height` tall, and a
/// screenful more each side is near enough.
pub fn row_in_view(top: f32, height: f32, scrolled: f32, viewport: f32) -> bool {
    top < scrolled + 2. * viewport && top + height > scrolled - viewport
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FILMS_LIBRARY, FakeServer, SERIES};
    use std::time::Duration;

    /// The file as it is: the cap is the pages' business.
    fn start(server: &dyn MediaServer, target: &PlayTarget) -> Result<Started, StartError> {
        super::start(server, target, Quality::Original)
    }

    fn server() -> FakeServer {
        FakeServer::new(40, true)
    }

    fn episode(server: &FakeServer, id: &str) -> Episode {
        let state = server.state.lock().unwrap();
        state
            .episodes
            .iter()
            .find(|e| e.item.id.0 == id)
            .cloned()
            .unwrap()
    }

    fn started(server: &FakeServer) -> Vec<Title> {
        server.continue_watching().unwrap()
    }

    // An empty list.

    fn latest(library: &str) -> ListKind {
        ListKind::Latest(ItemId(library.into()), LibraryKind::Films)
    }

    #[test]
    fn an_empty_list_says_why_it_is_empty_in_one_line() {
        for kind in [ListKind::Resume, ListKind::NextUp, latest("films")] {
            let why = kind.empty_reason();
            assert!(!why.is_empty() && !why.contains('\n'), "{kind:?}");
        }
        assert_eq!(ListKind::Resume.empty_reason(), "Nothing started yet.");
    }

    #[test]
    fn the_suggestion_is_continue_watching_when_it_is_not_empty_and_recently_added_otherwise() {
        let rows = [ListKind::NextUp, latest("films"), ListKind::Resume];
        assert_eq!(suggested(Some(&ListKind::NextUp), &rows), Some(2));
        let rows = [ListKind::NextUp, latest("films"), latest("series")];
        assert_eq!(suggested(Some(&ListKind::NextUp), &rows), Some(1));
        assert_eq!(
            suggested(Some(&ListKind::NextUp), &[ListKind::NextUp]),
            None
        );
    }

    #[test]
    fn a_page_that_is_not_a_row_of_the_home_is_offered_any_row() {
        let rows = [ListKind::Resume, latest("films")];
        assert_eq!(suggested(None, &rows), Some(0));
        assert_eq!(suggested(None, &[latest("films")]), Some(0));
    }

    #[test]
    fn an_empty_list_never_suggests_itself() {
        let rows = [ListKind::Resume, latest("films"), latest("series")];
        assert_eq!(suggested(Some(&ListKind::Resume), &rows), Some(1));
        assert_eq!(suggested(Some(&latest("films")), &rows), Some(0));
        let rows = [latest("films"), latest("series")];
        assert_eq!(suggested(Some(&latest("films")), &rows), Some(1));
    }

    // What is asked of the server, and when.

    #[test]
    fn the_answer_to_an_older_ask_is_not_taken_for_the_newest() {
        let mut asks = Asks::default();
        let old = asks.next();
        let new = asks.next();
        // The server answers the second ask first, the first one comes late.
        assert!(asks.is_latest(new));
        assert!(!asks.is_latest(old));
    }

    #[test]
    fn the_answer_to_the_only_ask_is_the_newest() {
        let mut asks = Asks::default();
        let only = asks.next();
        assert!(asks.is_latest(only));
    }

    #[test]
    fn the_full_list_of_new_series_follows_the_row_by_when_episodes_came_in() {
        let (films, series) = (
            latest_query(LibraryKind::Films, 1),
            latest_query(LibraryKind::Series, 1),
        );
        assert_eq!(
            (series.sort, series.descending, series.start),
            (FilmSort::LastAdded, true, grid::PAGE)
        );
        assert_eq!((films.sort, films.descending), (FilmSort::DateAdded, true));
    }

    #[test]
    fn a_row_asks_for_the_pictures_in_view_and_one_card_each_side() {
        // 340 px cards, 16 px apart, 40 px before the first, in a 1440 px window.
        assert_eq!(cards_in_view(0., 1440., 340., 16., 40., 16), 0..5);
    }

    #[test]
    fn a_row_scrolled_sideways_asks_for_the_cards_now_in_view() {
        // Five cards gone: the sixth is the first in view.
        let scrolled = 40. + 5. * 356.;
        let range = cards_in_view(scrolled, 1440., 340., 16., 40., 16);
        assert_eq!(range, 4..11);
    }

    #[test]
    fn a_row_scrolled_to_its_end_asks_for_no_card_it_does_not_have() {
        assert_eq!(cards_in_view(5000., 1440., 340., 16., 40., 8), 8..8);
        assert_eq!(cards_in_view(0., 1440., 340., 16., 40., 3), 0..3);
        assert_eq!(cards_in_view(0., 1440., 340., 16., 40., 0), 0..0);
    }

    #[test]
    fn a_row_far_below_the_fold_asks_for_no_picture() {
        // 810 px window, page not scrolled: rows up to two screens down are near enough.
        assert!(row_in_view(600., 240., 0., 810.));
        assert!(row_in_view(1500., 240., 0., 810.));
        assert!(!row_in_view(1700., 240., 0., 810.));
        // Scrolled down, the rows above are not.
        assert!(!row_in_view(100., 240., 1500., 810.));
        assert!(row_in_view(900., 240., 1500., 810.));
    }

    // The buttons.

    #[test]
    fn the_button_of_a_film_card_plays_the_film_and_of_a_series_card_its_next_episode() {
        let server = server();
        let films = started(&server);
        let Title::Film(film) = &films[0] else {
            panic!("a film first");
        };
        assert_eq!(PlayTarget::of(&films[0]), PlayTarget::Film(film.id.clone()));
        let station = server
            .series(&ItemId(FILMS_LIBRARY.into()), &Tools::default().query(0))
            .unwrap();
        assert_eq!(
            PlayTarget::of(&Title::Series(station.items[0].clone())),
            PlayTarget::Series(station.items[0].id.clone())
        );
        let episode = episode(&server, "series-0-s2-e4");
        assert_eq!(
            PlayTarget::of(&Title::Episode(episode.clone())),
            PlayTarget::Episode(Box::new(episode))
        );
    }

    #[test]
    fn a_film_from_the_header_plays_from_where_the_viewer_stopped() {
        let server = server();
        let begun = start(&server, &PlayTarget::Film(ItemId(FILM.into()))).unwrap();
        assert_eq!(begun.plan.film, ItemId(FILM.into()));
        assert_eq!(begun.plan.start, Duration::from_secs(72 * 60));
        assert_eq!(begun.title, "Le Dernier Hiver");
        assert!(begun.opened.is_ok());
        // The first version, with the server told which one plays.
        assert_eq!(
            server.state.lock().unwrap().record.plays,
            [(ItemId(FILM.into()), begun.plan.source.clone())]
        );
    }

    #[test]
    fn a_film_never_started_plays_from_its_beginning() {
        let server = server();
        let begun = start(&server, &PlayTarget::Film(ItemId("film-1".into()))).unwrap();
        assert_eq!(begun.plan.start, Duration::ZERO);
    }

    #[test]
    fn what_the_player_shows_of_a_film_is_its_year_and_its_version() {
        let server = server();
        let begun = start(&server, &PlayTarget::Film(ItemId(FILM.into()))).unwrap();
        assert_eq!(begun.subtitle, "2025 · 4K HDR10 · direct play");
    }

    #[test]
    fn a_series_from_the_header_plays_its_next_episode() {
        let server = server();
        let begun = start(&server, &PlayTarget::Series(ItemId(SERIES.into()))).unwrap();
        assert_eq!(begun.plan.film, ItemId("series-0-s2-e4".into()));
        assert_eq!(begun.plan.start, Duration::from_secs(30 * 60));
        assert_eq!(begun.title, "Station Nord");
        assert!(
            begun.subtitle.starts_with("S2 · E4 · Le Signal"),
            "{}",
            begun.subtitle
        );
    }

    #[test]
    fn a_series_never_started_plays_from_its_first_episode() {
        let server = server();
        let begun = start(&server, &PlayTarget::Series(ItemId("series-5".into()))).unwrap();
        assert_eq!(begun.plan.film, ItemId("series-5-s1-e1".into()));
        assert_eq!(begun.plan.start, Duration::ZERO);
    }

    #[test]
    fn an_episode_of_a_row_plays_under_its_own_id_from_where_it_stopped() {
        let server = server();
        let episode = episode(&server, "series-0-s2-e4");
        let begun = start(&server, &PlayTarget::Episode(Box::new(episode))).unwrap();
        assert_eq!(begun.plan.film, ItemId("series-0-s2-e4".into()));
        assert_eq!(begun.plan.start, Duration::from_secs(30 * 60));
        assert_eq!(
            server.state.lock().unwrap().record.plays,
            [(ItemId("series-0-s2-e4".into()), begun.plan.source.clone())]
        );
    }

    #[test]
    fn a_series_with_no_episode_has_nothing_to_play() {
        let server = server();
        assert_eq!(
            start(
                &server,
                &PlayTarget::Series(ItemId("series-unknown".into()))
            )
            .err(),
            Some(StartError::NothingToPlay)
        );
    }

    #[test]
    fn an_episode_with_no_version_has_nothing_to_play() {
        let server = server();
        let mut bare = episode(&server, "series-0-s2-e4");
        bare.versions.clear();
        assert_eq!(
            start(&server, &PlayTarget::Episode(Box::new(bare))).err(),
            Some(StartError::NothingToPlay)
        );
    }

    #[test]
    fn a_film_the_server_does_not_know_says_so() {
        let server = server();
        assert_eq!(
            start(&server, &PlayTarget::Film(ItemId("film-nowhere".into()))).err(),
            Some(StartError::Server(ProviderError::NotFound))
        );
    }

    // The rows.

    #[test]
    fn reprendre_lists_films_and_episodes_started() {
        let server = server();
        let ids: Vec<String> = started(&server)
            .iter()
            .map(|title| title.id().0.clone())
            .collect();
        assert_eq!(ids, ["film-0", "film-2", "film-5", "series-0-s2-e4"]);
    }

    #[test]
    fn an_episode_started_is_under_reprendre_and_not_again_under_prochain_episode() {
        let server = server();
        let next = server.next_episodes(ROW).unwrap();
        assert!(next.iter().any(|e| e.item.id.0 == "series-0-s2-e4"));
        let row = next_up_row(next, &started(&server));
        let ids: Vec<&str> = row.iter().map(|e| e.item.id.0.as_str()).collect();
        assert_eq!(ids, ["series-2-s1-e3"]);
    }

    #[test]
    fn a_next_episode_not_started_stays_in_its_row() {
        let server = server();
        let next = server.next_episodes(ROW).unwrap();
        assert_eq!(next_up_row(next.clone(), &[]), next);
    }

    #[test]
    fn the_rows_are_named_in_english() {
        assert_eq!(ListKind::Resume.heading(), "Continue watching");
        assert_eq!(ListKind::NextUp.heading(), "Next up");
        assert_eq!(
            ListKind::Latest(ItemId("films".into()), LibraryKind::Films).heading(),
            "Recently added"
        );
    }

    #[test]
    fn the_full_list_of_reprendre_comes_in_one_page() {
        let server = server();
        let page = fetch(&server, &ListKind::Resume, 0).unwrap();
        assert_eq!(page.total, 4);
        assert_eq!(page.items.len(), 4);
        // Nothing after the first page.
        assert!(
            fetch(&server, &ListKind::Resume, 1)
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[test]
    fn the_full_list_of_prochain_episode_leaves_out_what_reprendre_has() {
        let server = server();
        let page = fetch(&server, &ListKind::NextUp, 0).unwrap();
        let ids: Vec<&str> = page.items.iter().map(|t| t.id().0.as_str()).collect();
        assert_eq!(ids, ["series-2-s1-e3"]);
        assert_eq!(page.total, 1);
    }

    #[test]
    fn the_full_list_of_a_films_library_is_paged_newest_first() {
        let server = server();
        let kind = ListKind::Latest(ItemId(FILMS_LIBRARY.into()), LibraryKind::Films);
        let first = fetch(&server, &kind, 0).unwrap();
        assert_eq!(first.total, 40);
        assert_eq!(first.items.len(), 40);
        assert!(matches!(&first.items[0], Title::Film(film) if film.id.0 == "film-0"));
        let big = FakeServer::new(250, true);
        let third = fetch(&big, &kind, 2).unwrap();
        assert_eq!(third.items.len(), 50);
        assert_eq!(third.total, 250);
    }

    #[test]
    fn the_full_list_of_a_series_library_is_series() {
        let server = server();
        let kind = ListKind::Latest(ItemId("series".into()), LibraryKind::Series);
        let page = fetch(&server, &kind, 0).unwrap();
        assert_eq!(page.total, 12);
        assert!(page.items.iter().all(|t| matches!(t, Title::Series(_))));
    }

    // The cards.

    #[test]
    fn a_film_card_says_its_title_and_its_year() {
        let server = server();
        let films = started(&server);
        assert_eq!(
            card_lines(&films[0]),
            ("Le Dernier Hiver".to_string(), Some("2025".to_string()))
        );
    }

    #[test]
    fn an_episode_card_says_its_series_then_its_season_episode_and_title() {
        let server = server();
        let title = Title::Episode(episode(&server, "series-0-s2-e4"));
        assert_eq!(
            card_lines(&title),
            (
                "Station Nord".to_string(),
                Some("S2 · E4 · Le Signal".to_string())
            )
        );
    }

    #[test]
    fn a_series_card_says_its_years() {
        let server = server();
        let page = server
            .series(&ItemId("series".into()), &Tools::default().query(0))
            .unwrap();
        let title = Title::Series(page.items[0].clone());
        let (heading, detail) = card_lines(&title);
        assert_eq!(heading, "Station Nord");
        assert_eq!(detail.as_deref(), Some("2023 – 2025"));
    }

    #[test]
    fn a_card_of_something_started_shows_how_far_and_how_long_is_left() {
        let server = server();
        // film-0: 1 h 12 of 2 h 11.
        let films = started(&server);
        assert_eq!(time_left(&films[0]).as_deref(), Some("59m left"));
        let shown = progress(&films[0]).unwrap();
        assert!((shown - 72. / 131.).abs() < 1e-4, "{shown}");
        // The episode: 30 of 48 min.
        let title = Title::Episode(episode(&server, "series-0-s2-e4"));
        assert_eq!(time_left(&title).as_deref(), Some("18m left"));
        assert_eq!(progress(&title), Some(0.625));
    }

    #[test]
    fn a_card_of_something_not_started_has_neither() {
        let server = server();
        let title = Title::Episode(episode(&server, "series-0-s3-e1"));
        assert_eq!((time_left(&title), progress(&title)), (None, None));
        let page = server
            .series(&ItemId("series".into()), &Tools::default().query(0))
            .unwrap();
        let title = Title::Series(page.items[0].clone());
        assert_eq!((time_left(&title), progress(&title)), (None, None));
    }
}

#[cfg(test)]
mod download_tests {
    //! A downloaded title plays from its file, with the fake player, the fake server and a
    //! temporary folder.

    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use bobine::provider::{
        ItemId, MediaSourceId, PlayMethod, PlaybackReport, Segment, SegmentKind, Trickplay,
    };

    use super::*;
    use crate::downloads::{Disk, Downloads};
    use crate::fake::{FILM, FakeServer, PLAY_SESSION, Record, Report};
    use crate::fallback::{DOWNLOAD_CANNOT_PLAY, Fallback};
    use crate::player::fake::{Call, FakePlayer};
    use crate::player::session::Session;
    use crate::player::thumbnails::{BUDGET, Tiles};
    use crate::player::{PlayerError, PlayerEvent, Seek};
    use crate::reports::Reporting;
    use crate::video::fake::FakeDisplay;

    struct Roomy;

    impl Disk for Roomy {
        fn free(&self, _path: &Path) -> std::io::Result<u64> {
            Ok(1 << 40)
        }
    }

    const TRICKPLAY: Trickplay = Trickplay {
        width: 320,
        height: 180,
        columns: 2,
        rows: 2,
        count: 6,
        interval: Duration::from_secs(10),
    };

    fn intro() -> Segment {
        Segment {
            kind: SegmentKind::Intro,
            start: Duration::from_secs(5),
            end: Duration::from_secs(65),
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        server: FakeServer,
        downloads: Downloads,
        folder: PathBuf,
        player: FakePlayer,
    }

    /// The film downloaded whole, its 4K version with its subtitle file, thumbnails and
    /// segments; then the server's record is cleared.
    fn downloaded() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let server = FakeServer::new(10, true);
        {
            let mut state = server.state.lock().unwrap();
            state.trickplay = Some(TRICKPLAY);
            state.tiles = vec![b"tile-a".to_vec(), b"tile-b".to_vec()];
            state.segments = vec![intro()];
        }
        let downloads =
            Downloads::open(dir.path(), Arc::new(server.clone()), Arc::new(Roomy)).unwrap();
        let film = server.film(&ItemId(FILM.into())).unwrap();
        downloads
            .queue_film(&film, &MediaSourceId("source-4k".into()))
            .unwrap();
        downloads.run();
        server.state.lock().unwrap().record = Record::default();
        let folder = downloads.folder(&ItemId(FILM.into()));
        Fixture {
            _dir: dir,
            server,
            downloads,
            folder,
            player: FakePlayer::default(),
        }
    }

    impl Fixture {
        fn start(&self, reach: Reach) -> StartedDownload {
            start_download(
                &self.downloads,
                &ItemId(FILM.into()),
                reach,
                film_page::From::Resume,
            )
            .unwrap()
        }

        fn online(&self) -> Reach {
            Reach::Online(Arc::new(self.server.clone()))
        }

        /// The player loads what `started` opened.
        fn session(&self, started: &Started) -> Session {
            let opened = started.opened.as_ref().unwrap();
            let externals = started.plan.externals(opened);
            let mut session = Session::new(
                Box::new(FakePlayer {
                    calls: self.player.calls.clone(),
                    queued: self.player.queued.clone(),
                    info: self.player.info.clone(),
                    ..Default::default()
                }),
                Box::new(FakeDisplay::default()),
                started.title.clone(),
                Some(started.subtitle.clone()),
            );
            session
                .restart(started.plan.media(opened, &externals))
                .unwrap();
            session
        }

        /// Plays `started`, reporting to where it says, through each step of player events;
        /// then the viewer leaves.
        fn play(&self, started: StartedDownload, steps: Vec<Vec<PlayerEvent>>) {
            let mut session = self.session(&started.started);
            let opened = started.started.opened.as_ref().unwrap();
            let mut reporting = Reporting::to(
                started.reports,
                PlaybackReport {
                    session: opened.session.clone(),
                    film: opened.film.clone(),
                    source: opened.source.clone(),
                    position: started.started.plan.start,
                    paused: false,
                    audio_track: started.started.plan.audio_index,
                    subtitle_track: started.started.plan.subtitle_index,
                    method: PlayMethod::DirectPlay,
                },
            );
            let mut now = Instant::now();
            for events in steps {
                now += Duration::from_secs(1);
                self.player.queued.borrow_mut().extend(events);
                let ended = session.update(now);
                reporting.tick(now, &session, ended);
            }
            reporting.finish(&session).join().unwrap();
        }

        fn loaded_url(&self) -> String {
            self.player
                .calls
                .borrow()
                .iter()
                .find_map(|call| match call {
                    Call::Load { url, .. } => Some(url.clone()),
                    _ => None,
                })
                .unwrap()
        }

        /// Nothing at all was asked of the server since the download.
        fn server_untouched(&self) {
            let state = self.server.state.lock().unwrap();
            let record = &state.record;
            assert!(record.plays.is_empty(), "{:?}", record.plays);
            assert!(record.reports.is_empty());
            assert!(record.transcodes.is_empty());
            assert!(record.segment_asks.is_empty());
            assert!(record.tiles.is_empty());
            assert!(record.images.is_empty());
            assert!(record.film_pages.is_empty());
            assert!(record.titles.is_empty(), "{:?}", record.titles);
        }
    }

    #[test]
    fn a_downloaded_film_plays_its_local_file_with_its_local_subtitles() {
        let f = downloaded();
        let started = f.start(Reach::Offline);
        let opened = started.started.opened.as_ref().unwrap();
        let _session = f.session(&started.started);
        let file = f.folder.join("media.mkv");
        assert!(file.is_file());
        assert_eq!(f.loaded_url(), file.to_string_lossy());
        // The version's subtitle file, its fifth track, from the folder and named from its track.
        let subtitle = f.folder.join("subtitles/5.srt");
        assert!(subtitle.is_file());
        let urls: Vec<&str> = opened
            .external_subtitles
            .iter()
            .map(|external| external.url.expose_for_player())
            .collect();
        assert_eq!(urls, [subtitle.to_string_lossy()]);
        let Call::Load { externals, .. } = f.player.calls.borrow()[0].clone() else {
            panic!("not a load");
        };
        assert_eq!(externals.len(), 1);
        assert_ne!(externals[0].0, "Subtitles", "named from its track");
        assert_eq!(opened.source, MediaSourceId("source-4k".into()));
        assert!(!opened.converted);
    }

    #[test]
    fn an_online_play_of_a_download_reports_to_the_server() {
        let f = downloaded();
        let started = f.start(f.online());
        let opened = started.started.opened.as_ref().unwrap();
        // The server opened a playback session, as for any other play; the file is the local one.
        assert_eq!(opened.session.0, PLAY_SESSION);
        assert_eq!(
            f.server.state.lock().unwrap().record.plays,
            [(ItemId(FILM.into()), MediaSourceId("source-4k".into()))]
        );
        f.play(
            started,
            vec![vec![PlayerEvent::Loaded, PlayerEvent::Position(600.)]],
        );
        assert_eq!(f.loaded_url(), f.folder.join("media.mkv").to_string_lossy());
        let reports = f.server.reports();
        assert!(matches!(
            reports.as_slice(),
            [Report::Started(_), Report::Stopped(stopped)]
                if stopped.position == Duration::from_secs(600)
                    && stopped.session.0 == PLAY_SESSION
                    && stopped.method == PlayMethod::DirectPlay
        ));
        assert!(f.downloads.journal().plays().unwrap().is_empty());
    }

    #[test]
    fn an_offline_play_writes_the_journal_and_asks_nothing_of_the_server() {
        let f = downloaded();
        let before = std::time::SystemTime::now();
        let started = f.start(Reach::Offline);
        f.play(
            started,
            vec![
                vec![PlayerEvent::Loaded, PlayerEvent::Position(590.)],
                vec![PlayerEvent::Position(600.)],
            ],
        );
        let plays = f.downloads.journal().plays().unwrap();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].item, ItemId(FILM.into()));
        assert_eq!(plays[0].position, Duration::from_secs(600));
        assert!(!plays[0].played);
        assert!(plays[0].at >= before);
        assert!(f.folder.parent().unwrap().join("plays.json").is_file());
        f.server_untouched();
    }

    #[test]
    fn the_end_of_an_offline_play_marks_it_watched_in_the_journal() {
        let f = downloaded();
        let started = f.start(Reach::Offline);
        f.play(
            started,
            vec![
                vec![
                    PlayerEvent::Loaded,
                    PlayerEvent::Duration(7860.),
                    PlayerEvent::Position(7855.),
                ],
                vec![PlayerEvent::EndOfFile],
            ],
        );
        let plays = f.downloads.journal().plays().unwrap();
        assert_eq!(plays.len(), 1);
        assert!(plays[0].played);
        assert_eq!(plays[0].position, Duration::ZERO);
        f.server_untouched();
    }

    #[test]
    fn an_offline_play_stopped_in_the_credits_is_watched() {
        let f = downloaded();
        let started = f.start(Reach::Offline);
        // 7200 of 7860 seconds is past 90 %, and the viewer leaves without the file ending.
        f.play(
            started,
            vec![vec![
                PlayerEvent::Loaded,
                PlayerEvent::Duration(7860.),
                PlayerEvent::Position(7200.),
            ]],
        );
        let plays = f.downloads.journal().plays().unwrap();
        assert_eq!(plays.len(), 1);
        assert!(plays[0].played);
        assert_eq!(plays[0].position, Duration::ZERO);
        assert_eq!(plays[0].play_count, 1);

        // Short of 90 %, it is only where the viewer stopped.
        let f = downloaded();
        let started = f.start(Reach::Offline);
        f.play(
            started,
            vec![vec![
                PlayerEvent::Loaded,
                PlayerEvent::Duration(7860.),
                PlayerEvent::Position(7000.),
            ]],
        );
        let plays = f.downloads.journal().plays().unwrap();
        assert!(!plays[0].played);
        assert_eq!(plays[0].position, Duration::from_secs(7000));
    }

    #[test]
    fn a_download_that_does_not_play_asks_for_no_conversion() {
        let f = downloaded();
        let started = f.start(f.online());
        let mut session = f.session(&started.started);
        let opened = started.started.opened.as_ref().unwrap();
        let mut fallback = Fallback::local(
            Arc::new(f.server.clone()),
            started.started.plan.clone(),
            opened,
        );
        let mut now = Instant::now();
        f.player
            .queued
            .borrow_mut()
            .push(PlayerEvent::Error(PlayerError("loading failed".into())));
        for _ in 0..3 {
            now += Duration::from_secs(61);
            session.update(now);
            assert_eq!(fallback.tick(&mut session, now), None);
        }
        assert_eq!(
            session.error(),
            Some(&PlayerError(DOWNLOAD_CANNOT_PLAY.into()))
        );
        // Nor does a quality chosen in the settings.
        assert!(
            fallback
                .choose(
                    crate::player::settings::Quality::Cap(4_000_000),
                    &mut session
                )
                .is_none()
        );
        assert!(f.server.state.lock().unwrap().record.transcodes.is_empty());
    }

    #[test]
    fn offline_thumbnails_come_from_the_downloaded_tiles() {
        let f = downloaded();
        let started = f.start(Reach::Offline);
        let trickplay = started.local.trickplay().expect("thumbnails downloaded");
        assert_eq!(trickplay, TRICKPLAY);
        let mut tiles: Tiles<Vec<u8>> = Tiles::new(trickplay, BUDGET);
        let tile = tiles.next_fetch(Some(1), Instant::now()).unwrap();
        assert_eq!(tile, 1);
        assert_eq!(started.local.tile(tile).unwrap(), b"tile-b");
        assert_eq!(started.local.tile(0).unwrap(), b"tile-a");
        f.server_untouched();
    }

    #[test]
    fn offline_skipping_uses_the_downloaded_segments() {
        let f = downloaded();
        let started = f.start(Reach::Offline);
        let mut session = f.session(&started.started);
        session.set_segments(started.local.segments().to_vec());
        f.player.queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::Duration(7860.),
            PlayerEvent::Position(10.),
        ]);
        session.update(Instant::now());
        assert!(session.skip_button().is_some(), "the intro offers a skip");
        assert!(session.press_skip().is_none());
        assert!(f.player.calls.borrow().contains(&Call::Seek(Seek::To(65.))));
        f.server_untouched();
    }
}
