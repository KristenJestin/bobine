//! The port to a media server: what the rest of Bobine asks of one, in plain data, whatever the
//! server is. No type of a particular server leaves its implementation.
//!
//! Every call blocks until the server answers: the window runs them on its background executor,
//! never on the thread that draws.

mod keyring;
mod secret;
mod trickplay;

pub use keyring::{SecretServiceStore, TokenStore};
pub use secret::{StreamUrl, Token};
pub use trickplay::{Cell, Trickplay, WANTED_WIDTH};

#[cfg(test)]
pub use keyring::MemoryStore;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// How often the player tells the server where the viewer is, besides each pause and seek.
pub const PROGRESS_EVERY: Duration = Duration::from_secs(10);

/// Signs in to a server, or finds the session kept from the last start.
pub trait Connector: Send + Sync {
    /// Asks the address whether a server of this kind answers there, before any password.
    fn probe(&self, address: &str) -> Result<ServerInfo, ProviderError>;

    /// Signs in. The password serves for this call only: the token the server gives back goes to
    /// the keyring, and the password is kept nowhere.
    fn login(
        &self,
        address: &str,
        user: &str,
        password: &str,
    ) -> Result<Arc<dyn MediaServer>, ProviderError>;

    /// The session of the last start, when its token is still in the keyring.
    fn restore(&self) -> Result<Option<Arc<dyn MediaServer>>, ProviderError>;

    /// The address of the server of the session signed in, or of the last one once it ended (it
    /// is kept so that the login that follows can start from it). None before any login, or
    /// when it cannot be read.
    fn last_address(&self) -> Option<String>;
}

/// A signed-in session on a server.
pub trait MediaServer: Send + Sync {
    /// The server, as it named itself at login.
    fn server(&self) -> &ServerInfo;
    /// The name of the signed-in user.
    fn user_name(&self) -> &str;
    /// The id of the signed-in user on the server: what tells two users of one server apart,
    /// where the name can change.
    fn user_id(&self) -> &str;

    fn libraries(&self) -> Result<Vec<Library>, ProviderError>;
    fn films(&self, library: &ItemId, query: &FilmQuery) -> Result<Page<Film>, ProviderError>;
    /// The genres and decades of a library's titles, to filter its grid by.
    fn filter_choices(&self, library: &ItemId) -> Result<FilterChoices, ProviderError>;
    /// The films and episodes the user started and did not finish, the latest first.
    fn continue_watching(&self) -> Result<Vec<Title>, ProviderError>;
    fn film(&self, film: &ItemId) -> Result<FilmDetail, ProviderError>;
    /// The collection the film belongs to, with its other films. Apart from `film` because the
    /// server gives no direct link from a film to its collections: the lookup costs several
    /// requests, so a screen asks it once the page shows and hides the row if it fails.
    fn collection_of(&self, film: &ItemId) -> Result<Option<Collection>, ProviderError>;

    /// Opens a playback session of one version of a film or an episode, played directly from
    /// its file.
    fn play(&self, film: &ItemId, source: &MediaSourceId) -> Result<Playback, ProviderError>;
    fn report_started(&self, report: &PlaybackReport) -> Result<(), ProviderError>;
    fn report_progress(&self, report: &PlaybackReport) -> Result<(), ProviderError>;
    fn report_stopped(&self, report: &PlaybackReport) -> Result<(), ProviderError>;
    /// Marks a film or an episode watched, or no longer; a season or a series, all its episodes.
    fn set_watched(&self, film: &ItemId, watched: bool) -> Result<(), ProviderError>;

    /// An image at the width asked for (the server scales it down, keeping its proportions).
    fn image(&self, image: &ImageRef, width: u32) -> Result<Vec<u8>, ProviderError>;

    /// Ends the session: the server forgets the token and the keyring drops it.
    fn logout(&self) -> Result<(), ProviderError>;

    /// The series of a library, one page of them, sorted like films.
    fn series(&self, library: &ItemId, query: &FilmQuery) -> Result<Page<Series>, ProviderError>;
    fn series_detail(&self, series: &ItemId) -> Result<SeriesDetail, ProviderError>;
    /// The seasons of a series, in order.
    fn seasons(&self, series: &ItemId) -> Result<Vec<Season>, ProviderError>;
    /// The episodes of a season, in order, each with its versions.
    fn episodes(&self, series: &ItemId, season: &ItemId) -> Result<Vec<Episode>, ProviderError>;
    /// The episode the user would watch next: the one started, else the one after the last
    /// watched, across seasons; the first when none was watched; none when all were.
    fn next_up(&self, series: &ItemId) -> Result<Option<Episode>, ProviderError>;
    /// The films, series and people whose name matches `query`, at most `per_group` of each.
    fn search(&self, query: &str, per_group: u32) -> Result<SearchResults, ProviderError>;
    /// The films and series a person took part in, the newest first.
    fn person_titles(&self, person: &ItemId) -> Result<PersonTitles, ProviderError>;
    /// What to watch next in every series in progress, at most `limit` episodes: the one
    /// started, else the one after the last watched. Episodes started count here too, so the
    /// caller leaves out those it already lists as started.
    fn next_episodes(&self, limit: u32) -> Result<Vec<Episode>, ProviderError>;
    /// What was added to a library lately, the newest first, at most `limit` titles.
    fn latest(&self, library: &ItemId, limit: u32) -> Result<Vec<Title>, ProviderError>;
    /// A few recent titles the user has not watched, films and series, each with a backdrop:
    /// what the top of the Home shows in turn. At most `limit`.
    fn featured(&self, limit: u32) -> Result<Vec<Feature>, ProviderError>;

    /// Every collection of the user, by name. One call, not pages: a server holds tens of them.
    fn collections(&self) -> Result<Vec<CollectionCard>, ProviderError>;
    /// The poster of the earliest film of a collection that has one, to wear for a card with no
    /// picture of its own. None when no film has a poster, and on any failure (the server not
    /// answering, an expired session included): a bare card is no failure, and the next call
    /// that needs the session says so.
    fn collection_poster(&self, collection: &ItemId) -> Option<ImageRef>;
    /// A collection and its films, oldest release first.
    fn collection(&self, collection: &ItemId) -> Result<CollectionDetail, ProviderError>;
    /// The films and series the user put in their list, by title.
    fn favourites(&self) -> Result<Vec<Favourite>, ProviderError>;
    /// Puts a film or a series in the user's list, or takes it out.
    fn set_favourite(&self, item: &ItemId, favourite: bool) -> Result<(), ProviderError>;

    /// How the trickplay thumbnails of one version are laid out, at the size to show; none when
    /// the server made none for it.
    fn trickplay(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
    ) -> Result<Option<Trickplay>, ProviderError>;
    /// One tile of those thumbnails: a JPEG holding up to `columns` by `rows` of them.
    fn trickplay_tile(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
        trickplay: &Trickplay,
        tile: u32,
    ) -> Result<Vec<u8>, ProviderError>;
    /// A new playback of a version the player could not play as it is: the server converts it,
    /// from `request.start`, with the tracks asked.
    fn transcode(&self, request: &TranscodeRequest) -> Result<Playback, ProviderError>;
    /// The stretches of a film or an episode the server marked (intro, credits…), by start. A
    /// server that does not know them, or an item without any, answers none: never an error.
    fn media_segments(&self, item: &ItemId) -> Result<Vec<Segment>, ProviderError>;
    /// The films and series of every library that have a TMDb id, by that id: one call, for
    /// the request server's titles to be told in the library.
    fn tmdb_index(&self) -> Result<TmdbIndex, ProviderError>;
}

/// The library's films and series by their TMDb id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmdbIndex {
    pub films: HashMap<u64, ItemId>,
    pub series: HashMap<u64, ItemId>,
}

/// What a marked stretch of a film or an episode is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Intro,
    /// The credits.
    Outro,
    /// What happened before, at the start of an episode.
    Recap,
    Preview,
}

/// A stretch of a film or an episode the server marked, from `start` to `end` in its time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub kind: SegmentKind,
    pub start: Duration,
    pub end: Duration,
}

/// A film, a series or an episode, as a row of the Home lists it.
#[derive(Debug, Clone, PartialEq)]
pub enum Title {
    Film(Film),
    Series(Series),
    Episode(Episode),
}

impl Title {
    pub fn id(&self) -> &ItemId {
        match self {
            Title::Film(film) => &film.id,
            Title::Series(series) => &series.id,
            Title::Episode(episode) => &episode.item.id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKind {
    Film,
    Series,
}

/// A title the top of the Home shows: what its large header says of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Feature {
    pub kind: FeatureKind,
    pub id: ItemId,
    pub title: String,
    pub year: Option<i32>,
    pub community_rating: Option<f32>,
    /// The age rating, `PG-13`, `TV-MA`…
    pub official_rating: Option<String>,
    /// A film's; a series has none.
    pub runtime: Option<Duration>,
    pub overview: Option<String>,
    pub backdrop: Option<ImageRef>,
    /// The title drawn as an image, when the server has one.
    pub logo: Option<ImageRef>,
    /// Where the viewer stopped in a film they started.
    pub resume: Option<Duration>,
}

/// A series as a list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub id: ItemId,
    pub title: String,
    /// The year it began.
    pub year: Option<i32>,
    /// The year it ended, when it did.
    pub end_year: Option<i32>,
    pub community_rating: Option<f32>,
    /// How many of its episodes the user has not watched.
    pub unwatched: u32,
    /// Every one of its episodes is watched: unknown for a series found through one of its
    /// episodes or seasons, which says nothing of it.
    pub watched: Option<bool>,
    /// In the user's list: unknown like `watched`.
    pub favourite: Option<bool>,
    pub poster: Option<ImageRef>,
    pub backdrop: Option<ImageRef>,
    /// The season the server handed in place of the series (a new season in Recently added):
    /// the card says it, and the page opens on it.
    pub season: Option<Season>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesDetail {
    pub series: Series,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    pub official_rating: Option<String>,
    pub cast: Vec<CastMember>,
    pub similar: Vec<Series>,
    /// In the user's list.
    pub favourite: bool,
    /// Its id on TMDb, when the server knows it.
    pub tmdb: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Season {
    pub id: ItemId,
    pub name: String,
    pub number: Option<u32>,
    /// How many of its episodes the user has not watched.
    pub unwatched: u32,
}

/// An episode, with what it shares with a film in `item`: its id, title, runtime, watched state,
/// resume point, and its still as the poster.
#[derive(Debug, Clone, PartialEq)]
pub struct Episode {
    pub item: Film,
    pub series: ItemId,
    pub series_title: String,
    pub season: ItemId,
    pub season_number: Option<u32>,
    pub number: Option<u32>,
    pub overview: Option<String>,
    pub versions: Vec<MediaSource>,
}

/// What a search found, group by group.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SearchResults {
    pub films: Vec<Film>,
    pub series: Vec<SeriesHit>,
    pub people: Vec<PersonHit>,
}

/// A series as a search or a list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesHit {
    pub id: ItemId,
    pub title: String,
    pub year: Option<i32>,
    pub poster: Option<ImageRef>,
    /// In the user's list, and all watched: unknown for a series found through one of its
    /// episodes, which says nothing of it.
    pub favourite: Option<bool>,
    pub watched: Option<bool>,
}

/// A person as a search shows them: cast or crew, whatever the title.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonHit {
    pub id: ItemId,
    pub name: String,
    pub photo: Option<ImageRef>,
}

/// What a person took part in.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PersonTitles {
    pub films: Vec<Film>,
    pub series: Vec<SeriesHit>,
}

/// What a server says of itself before anyone signs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub id: String,
    pub name: String,
    pub version: String,
}

/// The id of an item on the server: a library, a film, a person, a collection.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemId(pub String);

/// The id of one version (file) of a film.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaSourceId(pub String);

/// The id that ties the reports of one playback together.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlaySessionId(pub String);

/// How the server sends a version: the file as it is, or converted by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayMethod {
    DirectPlay,
    Transcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Films,
    Series,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    pub id: ItemId,
    pub name: String,
    pub kind: LibraryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilmSort {
    Title,
    DateAdded,
    /// When the latest content came in: for a series, its newest episode.
    LastAdded,
    ReleaseYear,
    Rating,
}

/// What narrows a library's films or series: the server applies it, over the whole library.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    pub genre: Option<String>,
    /// The first year of a decade (`1990` for the nineties).
    pub decade: Option<i32>,
    /// Only what the user has not watched.
    pub unwatched: bool,
}

/// One page of the films of a library, in an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilmQuery {
    pub sort: FilmSort,
    pub descending: bool,
    pub filters: Filters,
    /// Only the titles that sort before this letter's: the `total` of such a query, with a
    /// `limit` of one, is the index of the first title of the letter.
    pub before: Option<char>,
    /// The index of the first film of the page.
    pub start: u32,
    pub limit: u32,
}

/// What a library offers to filter by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterChoices {
    pub genres: Vec<String>,
    /// The first year of each decade that holds a title, the latest first.
    pub decades: Vec<i32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// How many there are in all, across every page.
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageKind {
    /// A film's poster, a person's photo.
    Primary,
    Backdrop,
    /// A title drawn as an image, with a transparent background.
    Logo,
}

/// An image the server holds; the tag changes when the image does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub item: ItemId,
    pub kind: ImageKind,
    pub tag: String,
}

/// A film as a list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Film {
    pub id: ItemId,
    pub title: String,
    pub year: Option<i32>,
    pub community_rating: Option<f32>,
    pub runtime: Option<Duration>,
    pub watched: bool,
    /// Where the viewer stopped, when they did not finish it.
    pub resume: Option<Duration>,
    /// In the user's list.
    pub favourite: bool,
    pub poster: Option<ImageRef>,
    pub backdrop: Option<ImageRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilmDetail {
    pub film: Film,
    pub overview: Option<String>,
    pub genres: Vec<String>,
    /// The age rating, `PG-13`, `TV-MA`…
    pub official_rating: Option<String>,
    pub directors: Vec<String>,
    pub cast: Vec<CastMember>,
    pub similar: Vec<Film>,
    pub versions: Vec<MediaSource>,
    /// In the user's list.
    pub favourite: bool,
    /// Its id on TMDb, and the id of its TMDb collection, when the server knows them.
    pub tmdb: Option<u64>,
    pub tmdb_collection: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CastMember {
    pub id: ItemId,
    pub name: String,
    pub role: Option<String>,
    pub photo: Option<ImageRef>,
}

/// The collection a film belongs to, with its other films.
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: ItemId,
    pub name: String,
    pub others: Vec<Film>,
}

/// A collection as the grid lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionCard {
    pub id: ItemId,
    pub name: String,
    /// How many films it holds, when the server says.
    pub films: Option<u32>,
    pub poster: Option<ImageRef>,
}

/// A collection with its films, oldest release first.
#[derive(Debug, Clone, PartialEq)]
pub struct CollectionDetail {
    pub id: ItemId,
    pub name: String,
    pub overview: Option<String>,
    pub backdrop: Option<ImageRef>,
    pub films: Vec<Film>,
}

/// What the user's list holds: films and series together.
#[derive(Debug, Clone, PartialEq)]
pub enum Favourite {
    Film(Film),
    Series(Series),
}

/// One version of a film: one file, with its tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSource {
    pub id: MediaSourceId,
    pub name: String,
    pub container: Option<String>,
    pub video: Option<VideoTrack>,
    pub audio: Vec<AudioTrack>,
    pub subtitles: Vec<SubtitleTrack>,
    /// The audio track the server starts this version with for the user, from their settings;
    /// none when it did not say.
    pub default_audio: Option<i32>,
    /// The subtitle track it starts with: `Some(None)` for none, `None` when it did not say.
    pub default_subtitle: Option<Option<i32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoRange {
    Sdr,
    Hdr10,
    Hdr10Plus,
    Hlg,
    DolbyVision,
    Unknown,
}

impl VideoRange {
    pub fn is_hdr(self) -> bool {
        !matches!(self, VideoRange::Sdr | VideoRange::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoTrack {
    pub codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub range: VideoRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrack {
    /// The index of the stream in the file, as the reports name it.
    pub index: i32,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub channels: Option<u32>,
    pub title: Option<String>,
    pub default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleTrack {
    /// The index of the stream in the file, as the reports name it.
    pub index: i32,
    pub language: Option<String>,
    pub codec: Option<String>,
    pub title: Option<String>,
    pub forced: bool,
    pub default: bool,
    /// A file beside the film rather than a track inside it.
    pub external: bool,
}

/// A playback session the server knows of, and what the player opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playback {
    pub session: PlaySessionId,
    pub film: ItemId,
    pub source: MediaSourceId,
    pub stream: StreamUrl,
    pub external_subtitles: Vec<ExternalSubtitle>,
}

/// What the server converts: a version, from where the viewer was, with the tracks they chose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodeRequest {
    pub film: ItemId,
    pub source: MediaSourceId,
    pub start: Duration,
    /// The server's indexes. The subtitle is an embedded one, which the server extracts into a
    /// file or draws into the picture: an external file is added by the player, as in direct
    /// play.
    pub audio_track: Option<i32>,
    pub subtitle_track: Option<i32>,
    /// Bits per second the conversion keeps under, chosen by the viewer; `None` for no cap but
    /// the server's own.
    pub max_bitrate: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalSubtitle {
    /// The subtitle track it is, by its index.
    pub index: i32,
    pub url: StreamUrl,
}

/// Where the viewer is in a playback, for the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackReport {
    pub session: PlaySessionId,
    pub film: ItemId,
    pub source: MediaSourceId,
    pub position: Duration,
    pub paused: bool,
    pub audio_track: Option<i32>,
    pub subtitle_track: Option<i32>,
    pub method: PlayMethod,
}

/// What went wrong, said without the token or any URL that carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    /// The address is not one a client can reach (`http://` or `https://` and a host).
    InvalidAddress,
    /// Something answers at the address, but not a server of this kind.
    NotAMediaServer,
    /// The user name or the password is wrong.
    WrongCredentials,
    /// The account is disabled or blocked on the server: a right password changes nothing.
    AccountDisabled,
    /// The server no longer accepts the token: the user signs in again.
    SessionExpired,
    /// The server refuses this user what was asked (parental control, access schedule): the
    /// session is fine, only this is closed to them.
    Forbidden,
    /// The server does not know what was asked.
    NotFound,
    /// No answer: network down, server stopped, timeout. What failed, in words.
    Unreachable(String),
    /// The server answered with an unexpected HTTP status.
    Server(u16),
    /// The server answered, but not what was expected.
    BadAnswer(String),
    Keyring(String),
    /// The local file that keeps the account.
    Storage(String),
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress => {
                f.write_str("the address must start with http:// or https:// and name a host")
            }
            Self::NotAMediaServer => f.write_str("no media server answers at this address"),
            Self::WrongCredentials => f.write_str("wrong user name or password"),
            Self::AccountDisabled => f.write_str("the server has disabled or blocked this account"),
            Self::SessionExpired => f.write_str("the server ended the session: sign in again"),
            Self::Forbidden => f.write_str("the server refused access to this"),
            Self::NotFound => f.write_str("the server does not know this item"),
            Self::Unreachable(what) => write!(f, "the server cannot be reached: {what}"),
            Self::Server(status) => write!(f, "the server answered with HTTP status {status}"),
            Self::BadAnswer(what) => write!(f, "the server gave an unexpected answer: {what}"),
            Self::Keyring(what) => write!(f, "keyring: {what}"),
            Self::Storage(what) => write!(f, "account file: {what}"),
        }
    }
}

impl std::error::Error for ProviderError {}
