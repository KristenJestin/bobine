//! The port to a media server: what the rest of Bobine asks of one, in plain data, whatever the
//! server is. No type of a particular server leaves its implementation.
//!
//! Every call blocks until the server answers: the window runs them on its background executor,
//! never on the thread that draws.

mod keyring;
mod secret;

pub use keyring::{SecretServiceStore, TokenStore};
pub use secret::{StreamUrl, Token};

#[cfg(test)]
pub use keyring::MemoryStore;

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
}

/// A signed-in session on a server.
pub trait MediaServer: Send + Sync {
    /// The server, as it named itself at login.
    fn server(&self) -> &ServerInfo;
    /// The name of the signed-in user.
    fn user_name(&self) -> &str;

    fn libraries(&self) -> Result<Vec<Library>, ProviderError>;
    fn films(&self, library: &ItemId, query: &FilmQuery) -> Result<Page<Film>, ProviderError>;
    /// The films the user started and did not finish, the latest first.
    fn continue_watching(&self) -> Result<Vec<Film>, ProviderError>;
    fn film(&self, film: &ItemId) -> Result<FilmDetail, ProviderError>;
    /// The collection the film belongs to, with its other films. Apart from `film` because the
    /// server gives no direct link from a film to its collections: the lookup costs several
    /// requests, so a screen asks it once the page shows and hides the row if it fails.
    fn collection_of(&self, film: &ItemId) -> Result<Option<Collection>, ProviderError>;

    /// Opens a playback session of one version of a film, played directly from its file.
    fn play(&self, film: &ItemId, source: &MediaSourceId) -> Result<Playback, ProviderError>;
    fn report_started(&self, report: &PlaybackReport) -> Result<(), ProviderError>;
    fn report_progress(&self, report: &PlaybackReport) -> Result<(), ProviderError>;
    fn report_stopped(&self, report: &PlaybackReport) -> Result<(), ProviderError>;
    fn set_watched(&self, film: &ItemId, watched: bool) -> Result<(), ProviderError>;

    /// An image at the width asked for (the server scales it down, keeping its proportions).
    fn image(&self, image: &ImageRef, width: u32) -> Result<Vec<u8>, ProviderError>;

    /// Ends the session: the server forgets the token and the keyring drops it.
    fn logout(&self) -> Result<(), ProviderError>;

    /// The films, series and people whose name matches `query`, at most `per_group` of each.
    fn search(&self, query: &str, per_group: u32) -> Result<SearchResults, ProviderError>;
    /// The films and series a person took part in, the newest first.
    fn person_titles(&self, person: &ItemId) -> Result<PersonTitles, ProviderError>;
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
    ReleaseYear,
}

/// One page of the films of a library, in an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilmQuery {
    pub sort: FilmSort,
    pub descending: bool,
    /// The index of the first film of the page.
    pub start: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// How many there are in all, across every page.
    pub total: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageKind {
    /// A film's poster, a person's photo.
    Primary,
    Backdrop,
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

/// One version of a film: one file, with its tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSource {
    pub id: MediaSourceId,
    pub name: String,
    pub container: Option<String>,
    pub video: Option<VideoTrack>,
    pub audio: Vec<AudioTrack>,
    pub subtitles: Vec<SubtitleTrack>,
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
