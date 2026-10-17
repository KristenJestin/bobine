//! The port to a request server (Seerr, which Jellyseerr and Overseerr became): what the rest of
//! Bobine asks of one, in plain data. Apart from the media server: it is optional, and a title
//! it knows is named by its TMDb id, not by an item of the library.
//!
//! Every call blocks until the server answers: the window runs them on its background executor,
//! never on the thread that draws.

use std::fmt;
use std::time::Duration;

/// A film or a series on TMDb.
pub type TmdbId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MediaKind {
    Movie,
    Tv,
}

/// Where the request server stands on a title, whatever the library holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Nobody asked for it.
    Missing,
    /// Asked for, waiting for an approval.
    Pending,
    /// Approved, on its way.
    Processing,
    PartiallyAvailable,
    Available,
    /// Blocklisted on the server: it cannot be asked for.
    Blocked,
}

/// A poster, a backdrop or a photo on TMDb, by its path there (`/abc.jpg`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TmdbImage(pub String);

/// A film or a series as a row, a search or a collection shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Discovered {
    pub kind: MediaKind,
    pub tmdb: TmdbId,
    pub title: String,
    pub year: Option<i32>,
    pub rating: Option<f32>,
    pub overview: Option<String>,
    pub poster: Option<TmdbImage>,
    pub backdrop: Option<TmdbImage>,
    pub availability: Availability,
}

/// The rows of Discover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverRow {
    Trending,
    PopularMovies,
    PopularSeries,
    Upcoming,
}

impl DiscoverRow {
    pub const ALL: [DiscoverRow; 4] = [
        DiscoverRow::Trending,
        DiscoverRow::PopularMovies,
        DiscoverRow::PopularSeries,
        DiscoverRow::Upcoming,
    ];
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastPerson {
    pub name: String,
    pub character: Option<String>,
    pub photo: Option<TmdbImage>,
}

/// The TMDb collection a film belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionRef {
    pub id: TmdbId,
    pub name: String,
}

/// A season of a series, and whether it can still be asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonOffer {
    pub number: u32,
    pub name: String,
    pub episodes: u32,
    /// Available, partly available, or asked for already (and not declined): not offered.
    pub taken: bool,
}

/// What a request page shows of a title.
#[derive(Debug, Clone, PartialEq)]
pub struct TitleDetail {
    pub title: Discovered,
    pub genres: Vec<String>,
    /// A film's.
    pub runtime: Option<Duration>,
    pub cast: Vec<CastPerson>,
    /// A film's collection, when it is in one.
    pub collection: Option<CollectionRef>,
    /// A series' seasons, the specials left out.
    pub seasons: Vec<SeasonOffer>,
}

/// A TMDb collection and its films, oldest first.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionParts {
    pub id: TmdbId,
    pub name: String,
    pub overview: Option<String>,
    pub backdrop: Option<TmdbImage>,
    pub films: Vec<Discovered>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestStatus {
    Pending,
    Approved,
    Declined,
    /// Approved, but the server could not hand it on.
    Failed,
    /// Done: the title came.
    Completed,
}

/// A request of the viewer, with the title it asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct MyRequest {
    pub title: Discovered,
    pub status: RequestStatus,
}

/// What to ask for: a film, or seasons of a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Movie(TmdbId),
    Tv(TmdbId, Vec<u32>),
}

/// Whom the requests go as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActingAs {
    /// The request server's user that is the signed-in viewer of the media server.
    Viewer(String),
    /// No user there is the viewer: the requests go as the owner of the API key.
    Owner(String),
}

/// What a test of the connection found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCheck {
    pub version: String,
    pub acting_as: ActingAs,
}

/// A request server reached with its key, on behalf of the signed-in viewer.
pub trait RequestServer: Send + Sync {
    /// The server's version and whom the requests go as: the key is refused here if it is
    /// wrong.
    fn check(&self) -> Result<ServerCheck, RequestError>;
    fn discover(&self, row: DiscoverRow) -> Result<Vec<Discovered>, RequestError>;
    /// Films and series on TMDb whose name matches `query`; people left out.
    fn search(&self, query: &str) -> Result<Vec<Discovered>, RequestError>;
    fn title(&self, kind: MediaKind, id: TmdbId) -> Result<TitleDetail, RequestError>;
    fn recommendations(&self, kind: MediaKind, id: TmdbId)
    -> Result<Vec<Discovered>, RequestError>;
    fn collection(&self, id: TmdbId) -> Result<CollectionParts, RequestError>;
    /// The viewer's requests, the latest first.
    fn my_requests(&self) -> Result<Vec<MyRequest>, RequestError>;
    /// Whether the viewer may ask for no more titles of this kind for now.
    fn quota_reached(&self, kind: MediaKind) -> Result<bool, RequestError>;
    fn request(&self, ask: &Ask) -> Result<(), RequestError>;
    /// A picture of TMDb at about the width asked (TMDb has a few sizes).
    fn image(&self, image: &TmdbImage, width: u32) -> Result<Vec<u8>, RequestError>;
}

/// What came of asking for the films of a collection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollectionOutcome {
    pub requested: Vec<TmdbId>,
    /// The films not asked for, from the first one that failed.
    pub not_requested: Vec<TmdbId>,
    /// Why they were not.
    pub failure: Option<RequestError>,
}

/// A collection request on its way. Seerr has no request for a whole collection: its films are
/// asked for one after the other, as Seerr's own collection request does, each once the answer
/// for the one before came. It stops at the first failure, a quota reached before or during, so
/// that the films after it are said not requested rather than refused one by one.
#[derive(Debug, Default)]
pub struct CollectionRun {
    pub outcome: CollectionOutcome,
    tried: Vec<TmdbId>,
}

impl CollectionRun {
    /// The quota read before the first film: reached, or not read, nothing is asked for.
    pub fn quota(&mut self, answer: Result<bool, RequestError>, wanted: &[TmdbId]) {
        match answer {
            Ok(false) => {}
            Ok(true) => self.stop(RequestError::Quota, wanted),
            Err(why) => self.stop(why, wanted),
        }
    }

    /// The film to ask for next among those wanted, in their order; none once stopped.
    pub fn next(&self, wanted: &[TmdbId]) -> Option<TmdbId> {
        if self.outcome.failure.is_some() {
            return None;
        }
        wanted
            .iter()
            .copied()
            .find(|film| !self.tried.contains(film))
    }

    /// The answer for `film`, `wanted` the films wanted when it came.
    pub fn answered(&mut self, film: TmdbId, answer: Result<(), RequestError>, wanted: &[TmdbId]) {
        match answer {
            Ok(()) => self.outcome.requested.push(film),
            Err(why) => {
                self.outcome.not_requested.push(film);
                self.stop(why, wanted);
            }
        }
        self.tried.push(film);
    }

    fn stop(&mut self, why: RequestError, wanted: &[TmdbId]) {
        let left: Vec<TmdbId> = (wanted.iter().copied())
            .filter(|film| !self.tried.contains(film) && !self.outcome.not_requested.contains(film))
            .collect();
        self.outcome.not_requested.extend(left);
        self.outcome.failure = Some(why);
    }
}

/// The films of a collection that "Request collection" checks: neither in the library nor
/// asked for or had by the request server.
pub fn missing(
    films: &[Discovered],
    in_library: impl Fn(MediaKind, TmdbId) -> bool,
) -> Vec<TmdbId> {
    films
        .iter()
        .filter(|film| {
            film.availability == Availability::Missing && !in_library(film.kind, film.tmdb)
        })
        .map(|film| film.tmdb)
        .collect()
}

/// What went wrong, said without the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The address is not one a client can reach (`http://` or `https://` and a host).
    InvalidAddress,
    /// Something answers at the address, but not a request server.
    NotARequestServer,
    /// The server sent the call elsewhere: it is not followed, the key would go with it.
    Redirected,
    /// The server does not accept the API key.
    KeyRefused,
    /// The viewer has asked for as many titles as their quota allows for now.
    Quota,
    /// The server refuses the viewer this (no right to request).
    Forbidden,
    /// It was asked for already.
    AlreadyRequested,
    NotFound,
    /// No answer: network down, server stopped, timeout. What failed, in words.
    Unreachable(String),
    Server(u16),
    BadAnswer(String),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress => {
                f.write_str("the address must start with http:// or https:// and name a host")
            }
            Self::NotARequestServer => f.write_str("no Seerr server answers at this address"),
            Self::Redirected => f.write_str("the server answered with a redirect"),
            Self::KeyRefused => f.write_str("the server refused the API key"),
            Self::Quota => f.write_str("the request quota is reached"),
            Self::Forbidden => f.write_str("the server refused the request"),
            Self::AlreadyRequested => f.write_str("it was requested already"),
            Self::NotFound => f.write_str("the server does not know this title"),
            Self::Unreachable(what) => write!(f, "the server cannot be reached: {what}"),
            Self::Server(status) => write!(f, "the server answered with HTTP status {status}"),
            Self::BadAnswer(what) => write!(f, "the server gave an unexpected answer: {what}"),
        }
    }
}

impl std::error::Error for RequestError {}

/// The year of a date as TMDb writes it, `2024-05-01`.
pub fn year_of(date: &str) -> Option<i32> {
    date.get(..4)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a collection request to its end, `answer` saying how each film went.
    fn run(
        quota: Result<bool, RequestError>,
        wanted: &[TmdbId],
        answer: impl Fn(TmdbId) -> Result<(), RequestError>,
    ) -> (CollectionOutcome, Vec<TmdbId>) {
        let mut run = CollectionRun::default();
        let mut asked = Vec::new();
        run.quota(quota, wanted);
        while let Some(film) = run.next(wanted) {
            asked.push(film);
            run.answered(film, answer(film), wanted);
        }
        (run.outcome, asked)
    }

    #[test]
    fn a_collection_request_stops_at_the_quota_and_says_which_films_were_not_requested() {
        let (outcome, asked) = run(Ok(false), &[1, 2, 3, 4], |film| match film {
            3 => Err(RequestError::Quota),
            _ => Ok(()),
        });
        assert_eq!(outcome.requested, [1, 2]);
        assert_eq!(outcome.not_requested, [3, 4]);
        assert_eq!(outcome.failure, Some(RequestError::Quota));
        assert_eq!(asked, [1, 2, 3], "nothing after the quota");
    }

    #[test]
    fn a_quota_already_reached_requests_nothing() {
        let (outcome, asked) = run(Ok(true), &[1, 2], |_| Ok(()));
        assert!(outcome.requested.is_empty());
        assert_eq!(outcome.not_requested, [1, 2]);
        assert_eq!(outcome.failure, Some(RequestError::Quota));
        assert!(asked.is_empty());
    }

    #[test]
    fn a_year_is_read_from_a_tmdb_date() {
        assert_eq!(year_of("2024-05-01"), Some(2024));
        assert_eq!(year_of(""), None);
    }
}
