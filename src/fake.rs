//! A media server in memory, with invented films: the tests drive the flows with it, and the
//! headless harness shows the screens with it. It records what it is asked (logins, reports,
//! watched marks) and never touches a network, a keyring or a file.

use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobine::provider::{
    AudioTrack, CastMember, Collection, CollectionCard, CollectionDetail, Connector, DownloadFile,
    DownloadKind, DownloadPlan, EntryId, Episode, ExternalSubtitle, Favourite, Feature,
    FeatureKind, Fetched, Film, FilmDetail, FilmQuery, FilmSort, FilterChoices, HistorySource,
    ImageKind, ImageRef, ItemId, Library, LibraryKind, LocalTime, MediaServer, MediaSource,
    MediaSourceId, NewEpisode, Page, PersonHit, PersonTitles, Play, PlaySessionId, Playback,
    PlaybackReport, Playlist, PlaylistEntry, ProviderError, SearchResults, Season, Segment, Series,
    SeriesDetail, SeriesHit, ServerInfo, Sink, StreamUrl, SubtitleTrack, Title, TmdbIndex, Token,
    TokenStore, Trailers, TranscodeRequest, Trickplay, VideoRange, VideoTrack, WatchHistory,
    Watched, WatchedKind,
};

pub const USER: &str = "camille";
/// The tests' and the harness's now: Saturday 17 October 2026, at noon UTC.
pub const NOW: i64 = 1_792_238_400;
/// The id the fake server gives its user.
pub const USER_ID: &str = "user-1";
pub const PASSWORD: &str = "marée haute";
pub const FILMS_LIBRARY: &str = "films";
/// The film every page of the tests opens: started, with two versions and three subtitles.
pub const FILM: &str = "film-0";
pub const PLAY_SESSION: &str = "session-1";
/// The play session of a playback the server converts.
pub const TRANSCODE_SESSION: &str = "session-converted";
/// The series every series page of the tests opens: three seasons, the first watched, the second
/// stopped in its fourth episode.
pub const SERIES: &str = "series-0";
/// The TMDb ids of the film of the tests, of the next film, and of the series of the tests.
pub const IN_LIBRARY_FILM: u64 = 9001;
pub const OTHER_LIBRARY_FILM: u64 = 9002;
pub const IN_LIBRARY_SERIES: u64 = 7001;

/// A disk with as many bytes free as a test says: the tests never fill the real one.
#[cfg(test)]
pub struct FakeDisk(pub AtomicU64);

#[cfg(test)]
impl FakeDisk {
    pub fn with(free: u64) -> std::sync::Arc<FakeDisk> {
        std::sync::Arc::new(FakeDisk(AtomicU64::new(free)))
    }
}

#[cfg(test)]
impl crate::downloads::Disk for FakeDisk {
    fn free(&self, _path: &std::path::Path) -> std::io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

/// The system keyring in memory: the tests and the harness never reach the real one. Beside the
/// library's `MemoryStore`, which is `cfg(test)` of the library and so out of reach of this
/// binary and of its harness build.
#[derive(Default)]
pub struct MemoryKeyring {
    pub secrets: Mutex<HashMap<String, Token>>,
    /// The keyring cannot be reached: every call fails.
    pub unreachable: AtomicBool,
    /// The keyring is locked, and may not be unlocked: a token is not read.
    pub locked: AtomicBool,
}

impl MemoryKeyring {
    fn reach(&self) -> Result<(), ProviderError> {
        if self.unreachable.load(Ordering::SeqCst) {
            return Err(ProviderError::Keyring("locked".into()));
        }
        Ok(())
    }

    /// What the keyring holds under `account`, as text, for a test to compare.
    #[cfg(test)]
    pub fn secret(&self, account: &str) -> Option<String> {
        let secrets = self.secrets.lock().unwrap();
        secrets
            .get(account)
            .map(|token| token.expose_secret().to_string())
    }
}

impl TokenStore for MemoryKeyring {
    fn save(&self, account: &str, token: &Token) -> Result<(), ProviderError> {
        self.reach()?;
        self.secrets
            .lock()
            .unwrap()
            .insert(account.to_string(), token.clone());
        Ok(())
    }

    fn load(&self, account: &str) -> Result<Option<Token>, ProviderError> {
        self.reach()?;
        if self.locked.load(Ordering::SeqCst) {
            return Err(ProviderError::KeyringLocked);
        }
        Ok(self.secrets.lock().unwrap().get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), ProviderError> {
        self.reach()?;
        self.secrets.lock().unwrap().remove(account);
        Ok(())
    }
}

/// A report as the server got it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    Started(PlaybackReport),
    Progress(PlaybackReport),
    Stopped(PlaybackReport),
}

#[derive(Default)]
pub struct Record {
    pub logins: Vec<(String, String)>,
    pub reports: Vec<Report>,
    pub watched: Vec<(ItemId, bool)>,
    pub plays: Vec<(ItemId, MediaSourceId)>,
    pub images: Vec<(ImageRef, u32)>,
    pub film_pages: Vec<ItemId>,
    /// The items whose title was asked, in order.
    pub titles: Vec<ItemId>,
    /// The queries searched, in the order they came.
    pub searches: Vec<String>,
    /// Every call that put an item in the list (`true`) or took it out, answered or failed.
    pub favourite_calls: Vec<(ItemId, bool)>,
    /// How many times the list of favourites was asked.
    pub list_loads: u32,
    /// The items whose media segments were asked, in order.
    pub segment_asks: Vec<ItemId>,
    /// The tiles of trickplay asked, in the order they came.
    pub tiles: Vec<u32>,
    /// What the player asked the server to convert, in order.
    pub transcodes: Vec<TranscodeRequest>,
    /// Each logout, with how many playback reports the server had by then.
    pub logouts: Vec<usize>,
    /// Every change asked of a playlist, answered or failed.
    pub playlist_calls: Vec<PlaylistCall>,
    /// How many times the playlists were listed.
    pub playlists_listed: u32,
    /// The playlists read whole, in the order they were asked.
    pub playlist_reads: Vec<ItemId>,
    /// The titles whose trailer file was asked for, in order.
    pub trailer_asks: Vec<ItemId>,
    /// How many times the library was indexed by TMDb id.
    pub tmdb_indexes: u32,
    /// How many times the watch history was read.
    pub histories: u32,
    /// How many times the episodes added lately were asked.
    pub new_episode_asks: u32,
    /// Every file fetched, from which byte and with which validator, in the order asked.
    pub fetches: Vec<(DownloadFile, u64, Option<String>)>,
    /// The most fetches that ran at once.
    pub most_fetches_at_once: usize,
}

/// A change asked of a playlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistCall {
    Create(String, ItemId),
    Add(ItemId, ItemId),
    Remove(ItemId, EntryId),
}

/// A playlist as the fake server keeps it: its id, its name, and its entries with what each is.
#[derive(Clone, Debug)]
pub struct KeptPlaylist {
    pub id: ItemId,
    pub name: String,
    pub entries: Vec<(EntryId, ItemId)>,
}

/// The playlists the fake server starts with: films, and episodes in the second. A series put
/// in a playlist is put in as its episodes, as Jellyfin does, so none holds a series.
pub fn playlists() -> Vec<KeptPlaylist> {
    let kept = |id: &str, name: &str, items: &[&str]| KeptPlaylist {
        id: ItemId(id.into()),
        name: name.into(),
        entries: items
            .iter()
            .enumerate()
            .map(|(n, item)| (EntryId(format!("{id}-entry-{n}")), ItemId((*item).into())))
            .collect(),
    };
    vec![
        kept(
            "playlist-late",
            "Late night",
            &["film-5", "film-3", "film-8", "film-4", "film-9"],
        ),
        kept(
            "playlist-watchlist",
            "Watchlist",
            &["film-1", "series-1-s1-e2", "series-2-s1-e1", "film-0"],
        ),
    ]
}

/// A change made to what the fake server holds, at a moment a test chooses.
pub type Change = Box<dyn FnOnce(&mut State) + Send>;

/// What the fake server holds, shared by the connector and its sessions.
pub struct State {
    /// The token the "keyring" keeps: the session restored at the next start.
    pub kept: bool,
    /// The address of the last session's server, kept after a logout.
    pub last_address: Option<String>,
    pub films: Vec<Film>,
    pub series: Vec<Series>,
    /// Every episode of every series, in order: series, season, number.
    pub episodes: Vec<Episode>,
    /// What `play` gives as the stream: a local file in the harness.
    pub stream: String,
    /// What `transcode` gives as the stream, as the server's conversion would be.
    pub converted: String,
    /// `play` fails with this once it was asked: a test sees what was asked to play, and no
    /// player starts.
    pub play_error: Option<ProviderError>,
    pub record: Record,
    /// How long an image takes to come: a real server makes the fetches overlap.
    pub image_delay: std::time::Duration,
    /// No image comes: the list loads, the pictures fail as unreachable.
    pub images_fail: bool,
    /// The first time each person's photo is asked, the server fails (as a busy server that
    /// times out while it fetches the photo from elsewhere).
    pub photos_fail_once: bool,
    /// `collection_poster` fails with this, and each collection it was asked for is kept.
    pub collection_poster_error: Option<ProviderError>,
    pub collection_posters_asked: Vec<ItemId>,
    /// The server refuses to convert anything.
    pub transcode_fails: bool,
    pub people: Vec<PersonHit>,
    /// How long a search takes to answer: the headless harness shows the palette waiting.
    pub search_delay: std::time::Duration,
    /// What is in the user's list.
    pub favourites: Vec<ItemId>,
    /// Changing the list fails (a server that does not answer).
    pub favourite_fails: bool,
    /// How long changing the list takes, so that a double click or the optimistic state shows.
    pub favourite_delay: std::time::Duration,
    /// The media segments every item has (none: a server that does not know them).
    pub segments: Vec<Segment>,
    /// The trickplay layout every version has; none: the server made no trickplay images.
    pub trickplay: Option<Trickplay>,
    /// The JPEGs the tiles are, in turn: tile 0 is the first, tile 3 the first again if three.
    pub tiles: Vec<Vec<u8>>,
    /// How long a tile takes to come.
    pub tile_delay: std::time::Duration,
    /// How long a stopped report takes to reach the server, so that one sent later can overtake
    /// it.
    pub stop_delay: std::time::Duration,
    /// How long the server takes to answer a logout, so that the login waiting for it shows.
    pub logout_delay: std::time::Duration,
    /// The logout fails with this, after the server forgot the session (a keyring that cannot
    /// be reached).
    pub logout_error: Option<ProviderError>,
    pub playlists: Vec<KeptPlaylist>,
    /// Changing a playlist fails with this (a server that refuses it).
    pub playlist_refusal: Option<ProviderError>,
    /// Taking out these entries is refused, as a playlist the user may not change.
    pub refused_entries: Vec<EntryId>,
    /// Reading these playlists is refused, as one shared and then taken away.
    pub unreadable_playlists: Vec<ItemId>,
    /// How many playlists were made so far, for the ids of the next.
    pub playlists_made: usize,
    /// The id of the user the next login or restore gives a session to.
    pub user_id: String,
    /// The trailers every film and every series has.
    pub trailers: Trailers,
    /// What `local_trailer` gives as the stream.
    pub trailer_stream: String,
    /// `local_trailer` fails with this once it was asked.
    pub trailer_error: Option<ProviderError>,
    /// The TMDb ids of the films and series that have one.
    pub tmdb: Vec<(ItemId, u64)>,
    /// The TMDb collection of the films that have a TMDb id.
    pub tmdb_collection: Option<u64>,
    /// `tmdb_index` fails with this while it is set.
    pub tmdb_index_error: Option<ProviderError>,
    /// What the user watched: nothing, unless a test or the harness says otherwise.
    pub history: WatchHistory,
    /// `watch_history` fails with this while it is set.
    pub history_error: Option<ProviderError>,
    /// The episodes the server added, by id, and when (ticks of 100 ns since 1970).
    pub added: Vec<(ItemId, i64)>,
    /// What changes on the server once the first page of new episodes is read, before the next.
    pub between_pages: Option<Change>,
    /// No answer comes: the Home's lists, the search and the titles fail as unreachable.
    pub unreachable: bool,
    /// How many bytes each downloaded file is.
    pub download_size: u64,
    /// What the server's files are now: a new one is a file replaced, which a resume does not
    /// continue.
    pub download_version: String,
    /// Once a media file reached this many bytes, the hook runs, then the transfer goes on: a
    /// test sees the disk in the middle of a download. Once.
    pub mid_download: Option<(u64, Box<dyn FnOnce() + Send>)>,
    /// Once a media file reached this many bytes, the sink's disk is full. Once.
    pub disk_full_after: Option<u64>,
    /// How long each piece of a file takes to come, so that two downloads would overlap.
    pub fetch_delay: std::time::Duration,
    /// How many fetches run now.
    pub fetching: usize,
    /// The films the server lets nobody download.
    pub not_downloadable: Vec<ItemId>,
}

impl State {
    fn reachable(&self) -> Result<(), ProviderError> {
        if self.unreachable {
            return Err(ProviderError::Unreachable("timed out".into()));
        }
        Ok(())
    }
}

/// The TMDb id `tmdb` gives `id`.
fn state_tmdb(tmdb: &[(ItemId, u64)], id: &ItemId) -> Option<u64> {
    tmdb.iter()
        .find(|(item, _)| item == id)
        .map(|(_, tmdb)| *tmdb)
}

/// Where the fake server pretends to be.
pub const SERVER_ADDRESS: &str = "https://media.example.org";

#[derive(Clone)]
pub struct FakeServer {
    pub state: Arc<Mutex<State>>,
    info: ServerInfo,
    /// Who the session is: read from the state when it opens.
    user_id: String,
}

const TITLES: [&str; 16] = [
    "Le Dernier Hiver",
    "Les Marées",
    "Orbite basse",
    "Racines",
    "Rouge Vif",
    "Sable",
    "Le Quai",
    "Néon",
    "Fièvre",
    "Brume",
    "Lagune",
    "L'Atelier",
    "Minuit",
    "Braise",
    "Canopée",
    "Velours",
];

/// The genres of the invented titles: each title has one, by its number.
const GENRES: [&str; 4] = ["Drame", "Science-fiction", "Thriller", "Comédie"];

fn genre_of(id: &ItemId) -> &'static str {
    let number =
        id.0.rsplit('-')
            .next()
            .and_then(|n| n.parse::<usize>().ok());
    GENRES[number.unwrap_or(0) % GENRES.len()]
}

/// A title in lowercase, accents kept: the way the fake Jellyfin server orders names, as raw
/// strings.
fn folded(title: &str) -> String {
    title.to_lowercase()
}

/// What a title is filtered and sorted by.
struct Facts {
    title: String,
    year: Option<i32>,
    rating: Option<f32>,
    watched: bool,
    genre: &'static str,
}

/// The titles that pass `query`'s filters, in its order, and how many they are; `before` keeps
/// the ones that sort before a letter. Newest first is how `items` already come.
fn select<T: Clone>(items: &[T], query: &FilmQuery, facts: impl Fn(&T) -> Facts) -> (Vec<T>, u32) {
    use std::cmp::Ordering;
    let filters = &query.filters;
    let mut kept: Vec<(usize, Facts, &T)> = items
        .iter()
        .enumerate()
        .map(|(n, item)| (items.len() - n, facts(item), item))
        .filter(|(_, f, _)| {
            filters.genre.as_deref().is_none_or(|g| f.genre == g)
                && filters
                    .decade
                    .is_none_or(|d| f.year.is_some_and(|y| (d..d + 10).contains(&y)))
                && (!filters.unwatched || !f.watched)
                && query
                    .before
                    .is_none_or(|letter| folded(&f.title) < letter.to_lowercase().to_string())
        })
        .collect();
    let total = kept.len() as u32;
    kept.sort_by(|(added_a, a, _), (added_b, b, _)| {
        let by = match query.sort {
            FilmSort::DateAdded | FilmSort::LastAdded => added_a.cmp(added_b),
            FilmSort::Title => Ordering::Equal,
            FilmSort::ReleaseYear => a.year.cmp(&b.year),
            FilmSort::Rating => a.rating.partial_cmp(&b.rating).unwrap_or(Ordering::Equal),
        };
        by.then_with(|| folded(&a.title).cmp(&folded(&b.title)))
    });
    if query.descending {
        kept.reverse();
    }
    let page = kept
        .into_iter()
        .skip(query.start as usize)
        .take(query.limit as usize)
        .map(|(_, _, item)| item.clone())
        .collect();
    (page, total)
}

fn image(item: &str, kind: ImageKind) -> Option<ImageRef> {
    Some(ImageRef {
        item: ItemId(item.into()),
        kind,
        tag: "t1".into(),
    })
}

/// `count` invented films, newest first. The first is started at 1 h 12 of 2 h 11; a few others
/// are started or watched.
pub fn films(count: usize) -> Vec<Film> {
    (0..count)
        .map(|n| {
            let id = format!("film-{n}");
            let title = match n / TITLES.len() {
                0 => TITLES[n].to_string(),
                round => format!("{} {}", TITLES[n % TITLES.len()], round + 1),
            };
            Film {
                title,
                year: Some(2025 - (n % 8) as i32),
                community_rating: Some(6. + (n * 7 % 25) as f32 / 10.),
                // The film of the tests runs 2 h 11, as in the design.
                runtime: Some(Duration::from_secs(match n {
                    0 => 131 * 60,
                    _ => 5400 + (n as u64 % 5) * 900,
                })),
                watched: n % 5 == 3,
                resume: match n {
                    0 => Some(Duration::from_secs(72 * 60)),
                    2 => Some(Duration::from_secs(40 * 60)),
                    5 => Some(Duration::from_secs(20 * 60)),
                    _ => None,
                },
                favourite: false,
                poster: image(&id, ImageKind::Primary),
                backdrop: image(&id, ImageKind::Backdrop),
                id: ItemId(id),
            }
        })
        .collect()
}

fn audio(index: i32, language: &str, codec: &str, channels: u32, default: bool) -> AudioTrack {
    AudioTrack {
        index,
        language: Some(language.into()),
        codec: Some(codec.into()),
        channels: Some(channels),
        title: None,
        default,
    }
}

fn subtitle(
    index: i32,
    language: &str,
    codec: &str,
    forced: bool,
    external: bool,
) -> SubtitleTrack {
    SubtitleTrack {
        index,
        language: Some(language.into()),
        codec: Some(codec.into()),
        title: None,
        forced,
        default: false,
        external,
    }
}

/// Two versions of the film of the tests: 4K HDR10 with two audio tracks, two embedded subtitles
/// and an external one; 1080p with one of each.
pub fn versions() -> Vec<MediaSource> {
    vec![
        MediaSource {
            id: MediaSourceId("source-4k".into()),
            name: "Le Dernier Hiver 4K".into(),
            container: Some("mkv".into()),
            video: Some(VideoTrack {
                codec: Some("hevc".into()),
                width: Some(3840),
                height: Some(1600),
                range: VideoRange::Hdr10,
            }),
            audio: vec![
                audio(1, "fre", "eac3", 6, true),
                audio(2, "eng", "truehd", 8, false),
            ],
            subtitles: vec![
                subtitle(3, "fre", "subrip", true, false),
                subtitle(4, "fre", "ass", false, false),
                subtitle(5, "eng", "subrip", false, true),
            ],
            // The server's choice for the user: English audio, the full French subtitles.
            default_audio: Some(2),
            default_subtitle: Some(Some(4)),
        },
        MediaSource {
            id: MediaSourceId("source-1080".into()),
            name: "Le Dernier Hiver 1080p".into(),
            container: Some("mp4".into()),
            video: Some(VideoTrack {
                codec: Some("h264".into()),
                width: Some(1920),
                height: Some(1080),
                range: VideoRange::Sdr,
            }),
            audio: vec![audio(1, "fre", "aac", 2, true)],
            subtitles: vec![subtitle(2, "fre", "subrip", false, false)],
            default_audio: Some(1),
            default_subtitle: Some(Some(2)),
        },
    ]
}

const SERIES_TITLES: [&str; 12] = [
    "Station Nord",
    "Les Cartographes",
    "Hivernage",
    "Hiver blanc",
    "Hors-champ",
    "Désert",
    "Le Phare",
    "Contre-jour",
    "Les Veilleurs",
    "Saison sèche",
    "Archipel",
    "Après la pluie",
];

const EPISODE_TITLES: [&str; 6] = [
    "La Fissure",
    "Le Signal",
    "Dérive",
    "Remontée",
    "Les Absents",
    "Silence radio",
];

/// The seasons of a series, by number, and their number of episodes: the first series has three,
/// as in the design, the second its specials (season 0) before its season, the others one.
fn season_sizes(series: usize) -> &'static [(u32, u32)] {
    match series {
        0 => &[(1, 3), (2, 6), (3, 5)],
        1 => &[(0, 2), (1, 4)],
        _ => &[(1, 4)],
    }
}

/// Invented series. Their unwatched counts are worked out from their episodes when asked.
pub fn series() -> Vec<Series> {
    SERIES_TITLES
        .iter()
        .enumerate()
        .map(|(n, title)| {
            let id = format!("series-{n}");
            let year = 2025 - (n % 6) as i32;
            Series {
                title: (*title).into(),
                year: Some(year - 2),
                end_year: (n % 3 != 1).then_some(year),
                community_rating: Some(6.5 + (n * 3 % 20) as f32 / 10.),
                unwatched: 0,
                watched: Some(false),
                favourite: Some(false),
                poster: image(&id, ImageKind::Primary),
                backdrop: image(&id, ImageKind::Backdrop),
                season: None,
                id: ItemId(id),
            }
        })
        .collect()
}

/// What an invented viewer watched up to `now`, in seconds from 1970: a few films and series, most in the evening,
/// a lot of them this year. Read from the played items, as the server's adapter reads them: the
/// last play of each title on its date, the plays before it at no known time.
pub fn watch_history(source: HistorySource, now: i64) -> WatchHistory {
    let films = films(8);
    let series = series();
    let watched = |id: &ItemId, name: &str, kind, poster: &Option<ImageRef>| Watched {
        id: Some(id.clone()),
        name: name.to_string(),
        kind,
        poster: poster.clone(),
        genres: vec![genre_of(id).to_string()],
    };
    let mut plays = Vec::new();
    for n in 0..120u32 {
        let (title, minutes) = if n % 3 == 0 {
            let film = &films[(n as usize * 5) % films.len()];
            let minutes = film.runtime.map_or(100, |runtime| runtime.as_secs() / 60);
            let title = watched(&film.id, &film.title, WatchedKind::Movie, &film.poster);
            (title, minutes)
        } else {
            let one = &series[(n as usize * 7) % series.len().min(5)];
            let title = watched(&one.id, &one.title, WatchedKind::Series, &one.poster);
            (title, 45)
        };
        // Back from now, two or three days at a time, in the evening mostly.
        let back = i64::from(n) * 5 / 2;
        let hour = [21, 20, 22, 19, 23, 14, 21, 10][n as usize % 8];
        let at = LocalTime {
            hour,
            minute: (n * 7) % 60,
            ..LocalTime::from_unix(now - back * 86_400)
        };
        plays.push(Play {
            title,
            at: Some(at),
            seconds: minutes * 60,
            count: 1,
        });
    }
    if source == HistorySource::PlayedItems {
        // The plays are the newest first: the first of each title is its last.
        let mut last: Vec<Play> = Vec::new();
        let mut before: Vec<Play> = Vec::new();
        for play in plays {
            if !last.iter().any(|kept| kept.title.id == play.title.id) {
                last.push(play);
                continue;
            }
            match before
                .iter_mut()
                .find(|kept| kept.title.id == play.title.id)
            {
                Some(kept) => {
                    kept.count += 1;
                    kept.seconds += play.seconds;
                }
                None => before.push(Play { at: None, ..play }),
            }
        }
        plays = last.into_iter().chain(before).collect();
    }
    WatchHistory { source, plays }
}

/// A series as a search or a person's page lists it.
fn hit(series: &Series) -> SeriesHit {
    SeriesHit {
        id: series.id.clone(),
        title: series.title.clone(),
        year: series.year,
        poster: series.poster.clone(),
        favourite: series.favourite,
        watched: series.watched,
    }
}

/// The episodes of every series. The first series: season 1 watched, season 2 watched up to
/// its third episode, its fourth stopped at 30 min of 48.
pub fn episodes() -> Vec<Episode> {
    let mut all = Vec::new();
    for (n, title) in SERIES_TITLES.iter().enumerate() {
        for (s, &(season, size)) in season_sizes(n).iter().enumerate() {
            for number in 1..=size {
                let id = format!("series-{n}-s{season}-e{number}");
                // The third series is begun too: its first two episodes are watched.
                let watched = (n == 0 && (season == 1 || (season == 2 && number <= 3)))
                    || (n == 2 && number <= 2);
                let resume =
                    (n == 0 && season == 2 && number == 4).then(|| Duration::from_secs(30 * 60));
                all.push(Episode {
                    item: Film {
                        title: EPISODE_TITLES[(number as usize + 3 * s) % EPISODE_TITLES.len()]
                            .into(),
                        year: None,
                        community_rating: None,
                        runtime: Some(Duration::from_secs(48 * 60)),
                        watched,
                        resume,
                        favourite: false,
                        poster: image(&id, ImageKind::Primary),
                        backdrop: None,
                        id: ItemId(id),
                    },
                    series: ItemId(format!("series-{n}")),
                    series_poster: image(&format!("series-{n}"), ImageKind::Primary),
                    series_title: (*title).into(),
                    season: ItemId(format!("series-{n}-s{season}")),
                    season_number: Some(season),
                    number: Some(number),
                    overview: Some(
                        "L'équipage capte un message venu d'une base abandonnée depuis vingt ans, \
                         et doit décider s'il remonte à la surface avant la tempête."
                            .into(),
                    ),
                    versions: versions().split_off(1),
                    downloadable: true,
                });
            }
        }
    }
    all
}

const FILM_OVERVIEW: &str = "Trois frères traversent un plateau gelé pour rapporter le corps de leur \
     père, avant que la route ne ferme pour l'hiver.";
const SERIES_OVERVIEW: &str = "Sous la glace de l'Arctique, l'équipage d'une station de recherche perd \
     le contact avec la surface et découvre qu'il n'est pas seul.";

const CAST: [(&str, &str); 18] = [
    ("Paul Mercier", "Anton"),
    ("Léa Morand", "Hanna"),
    ("Yann Ribot", "Ilya"),
    ("Sara Diallo", "La guide"),
    ("Marc Olier", "Le père"),
    ("Nina Brel", "Vera"),
    ("Tom Achard", "Le passeur"),
    ("Iris Lemaire", "La douanière"),
    ("Hugo Vasseur", "Le chauffeur"),
    ("Chloé Arnaud", "Mila"),
    ("Bastien Roux", "Le pêcheur"),
    ("Anaïs Perrin", "La sœur"),
    ("Karim Benali", "Le guide"),
    ("Élise Fabre", "La voisine"),
    ("Victor Lenoir", "Le prêtre"),
    ("Manon Giraud", "L'enfant"),
    ("Jules Caron", "Le soldat"),
    ("Zoé Marchand", "La mère"),
];

/// The cast of the film of the tests, and its director (`person-18`).
pub fn people() -> Vec<PersonHit> {
    CAST.iter()
        .map(|(name, _)| *name)
        .chain(["Inès Vallard"])
        .enumerate()
        .map(|(n, name)| {
            let id = format!("person-{n}");
            PersonHit {
                name: name.into(),
                // Some people have no photo on the server.
                photo: (n % 4 != 3)
                    .then(|| image(&id, ImageKind::Primary))
                    .flatten(),
                id: ItemId(id),
            }
        })
        .collect()
}

impl FakeServer {
    /// A server with `count` films; `kept` when a session is kept from an earlier start.
    pub fn new(count: usize, kept: bool) -> Self {
        let favourites = ["film-2", "film-7", "series-1", "series-4"].map(|id| ItemId(id.into()));
        let mut films = films(count);
        for film in &mut films {
            film.favourite = favourites.contains(&film.id);
        }
        let mut series = series();
        for series in &mut series {
            series.favourite = Some(favourites.contains(&series.id));
        }
        Self {
            state: Arc::new(Mutex::new(State {
                kept,
                last_address: None,
                films,
                series,
                episodes: episodes(),
                stream: "file:///nowhere.mkv".into(),
                converted: "file:///nowhere-converted.m3u8".into(),
                play_error: None,
                record: Record::default(),
                image_delay: std::time::Duration::ZERO,
                images_fail: false,
                photos_fail_once: false,
                collection_poster_error: None,
                collection_posters_asked: Vec::new(),
                transcode_fails: false,
                people: people(),
                search_delay: std::time::Duration::ZERO,
                favourites: favourites.to_vec(),
                favourite_fails: false,
                favourite_delay: std::time::Duration::ZERO,
                segments: Vec::new(),
                trickplay: None,
                tiles: Vec::new(),
                tile_delay: std::time::Duration::ZERO,
                stop_delay: std::time::Duration::ZERO,
                logout_delay: std::time::Duration::ZERO,
                logout_error: None,
                playlists: playlists(),
                playlist_refusal: None,
                refused_entries: Vec::new(),
                unreadable_playlists: Vec::new(),
                playlists_made: 0,
                user_id: USER_ID.into(),
                tmdb: vec![
                    (ItemId(FILM.into()), IN_LIBRARY_FILM),
                    (ItemId("film-1".into()), OTHER_LIBRARY_FILM),
                    (ItemId(SERIES.into()), IN_LIBRARY_SERIES),
                ],
                tmdb_collection: None,
                tmdb_index_error: None,
                trailers: Trailers::default(),
                trailer_stream: "file:///nowhere-trailer.mp4".into(),
                trailer_error: None,
                history: WatchHistory {
                    source: HistorySource::PlayedItems,
                    plays: Vec::new(),
                },
                history_error: None,
                added: Vec::new(),
                between_pages: None,
                unreachable: false,
                download_size: 64,
                download_version: "v1".into(),
                mid_download: None,
                disk_full_after: None,
                fetch_delay: std::time::Duration::ZERO,
                fetching: 0,
                not_downloadable: Vec::new(),
            })),
            info: ServerInfo {
                id: "server-1".into(),
                name: "Salon".into(),
                version: "10.11.0".into(),
            },
            user_id: USER_ID.into(),
        }
    }

    /// The same server under another id, as a second server would be.
    #[cfg(test)]
    pub fn on_server(mut self, id: &str) -> Self {
        self.info.id = id.into();
        self
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    #[cfg(test)]
    pub fn reports(&self) -> Vec<Report> {
        self.state().record.reports.clone()
    }

    /// A series as the server shows it now: its unwatched count follows its episodes.
    fn counted(state: &State, series: &Series) -> Series {
        let episodes = || state.episodes.iter().filter(|e| e.series == series.id);
        let unwatched = episodes().filter(|e| !e.item.watched).count() as u32;
        Series {
            unwatched,
            watched: Some(unwatched == 0 && episodes().next().is_some()),
            ..series.clone()
        }
    }

    fn episode(&self, id: &ItemId) -> Option<Episode> {
        self.state()
            .episodes
            .iter()
            .find(|e| &e.item.id == id)
            .cloned()
    }

    /// The subtitle files beside a version of a film or an episode.
    fn external_subtitles(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
    ) -> Result<Vec<ExternalSubtitle>, ProviderError> {
        let versions = match self.episode(film) {
            Some(episode) => episode.versions,
            None => self.film(film)?.versions,
        };
        let found = versions
            .iter()
            .find(|s| &s.id == source)
            .ok_or(ProviderError::NotFound)?;
        Ok(found
            .subtitles
            .iter()
            .filter(|s| s.external)
            .map(|s| ExternalSubtitle {
                index: s.index,
                url: StreamUrl::new(format!("file:///nowhere-{}.srt", s.index)),
            })
            .collect())
    }

    fn find(&self, film: &ItemId) -> Result<Film, ProviderError> {
        self.state()
            .films
            .iter()
            .find(|f| &f.id == film)
            .cloned()
            .ok_or(ProviderError::NotFound)
    }

    fn session(&self) -> Arc<dyn MediaServer> {
        Arc::new(Self {
            user_id: self.state().user_id.clone(),
            ..self.clone()
        })
    }

    /// What a playlist holds, as the server shows it now; what it no longer knows is left out.
    fn entries(state: &State, playlist: &KeptPlaylist) -> Vec<PlaylistEntry> {
        playlist
            .entries
            .iter()
            .filter_map(|(entry, item)| {
                let (title, poster) = if let Some(film) = state.films.iter().find(|f| &f.id == item)
                {
                    (Title::Film(film.clone()), film.poster.clone())
                } else {
                    let episode = state.episodes.iter().find(|e| &e.item.id == item)?;
                    let series = state.series.iter().find(|s| s.id == episode.series);
                    let poster = series.and_then(|series| series.poster.clone());
                    (Title::Episode(episode.clone()), poster)
                };
                Some(PlaylistEntry {
                    id: entry.clone(),
                    title,
                    poster,
                })
            })
            .collect()
    }

    /// A change of a playlist: recorded, and refused when the server refuses them.
    fn change_playlist(state: &mut State, call: PlaylistCall) -> Result<(), ProviderError> {
        if let PlaylistCall::Remove(_, entry) = &call
            && state.refused_entries.contains(entry)
        {
            state.record.playlist_calls.push(call);
            return Err(ProviderError::Forbidden);
        }
        state.record.playlist_calls.push(call);
        match &state.playlist_refusal {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    /// What putting `item` in a playlist puts in: a series' episodes, in order; else the item.
    fn put_in(state: &State, item: &ItemId) -> Vec<ItemId> {
        if state.series.iter().any(|series| &series.id == item) {
            let episodes = state.episodes.iter().filter(|e| &e.series == item);
            return episodes.map(|episode| episode.item.id.clone()).collect();
        }
        vec![item.clone()]
    }
}

impl Connector for FakeServer {
    fn probe(&self, address: &str) -> Result<ServerInfo, ProviderError> {
        if !(address.starts_with("http://") || address.starts_with("https://")) {
            return Err(ProviderError::InvalidAddress);
        }
        Ok(self.info.clone())
    }

    fn login(
        &self,
        address: &str,
        user: &str,
        password: &str,
    ) -> Result<Arc<dyn MediaServer>, ProviderError> {
        self.probe(address)?;
        self.state()
            .record
            .logins
            .push((address.to_string(), user.to_string()));
        if user != USER || password != PASSWORD {
            return Err(ProviderError::WrongCredentials);
        }
        self.state().kept = true;
        Ok(self.session())
    }

    fn restore(&self) -> Result<Option<Arc<dyn MediaServer>>, ProviderError> {
        // The guard is let go before `session` takes the state again.
        let kept = self.state().kept;
        Ok(kept.then(|| self.session()))
    }

    fn last_address(&self) -> Option<String> {
        let state = self.state();
        state
            .last_address
            .clone()
            .or_else(|| state.kept.then(|| SERVER_ADDRESS.to_string()))
    }
}

impl MediaServer for FakeServer {
    fn server(&self) -> &ServerInfo {
        &self.info
    }

    fn user_name(&self) -> &str {
        USER
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn libraries(&self) -> Result<Vec<Library>, ProviderError> {
        Ok(vec![
            Library {
                id: ItemId("series".into()),
                name: "Séries".into(),
                kind: LibraryKind::Series,
            },
            Library {
                id: ItemId(FILMS_LIBRARY.into()),
                name: "Movies".into(),
                kind: LibraryKind::Films,
            },
        ])
    }

    fn filter_choices(&self, _: &ItemId) -> Result<FilterChoices, ProviderError> {
        let mut genres: Vec<String> = GENRES.iter().map(|g| (*g).into()).collect();
        genres.sort();
        Ok(FilterChoices {
            genres,
            decades: vec![2020, 2010],
        })
    }

    fn films(&self, _: &ItemId, query: &FilmQuery) -> Result<Page<Film>, ProviderError> {
        let (items, total) = select(&self.state().films, query, |film| Facts {
            title: film.title.clone(),
            year: film.year,
            rating: film.community_rating,
            watched: film.watched,
            genre: genre_of(&film.id),
        });
        Ok(Page { items, total })
    }

    fn continue_watching(&self) -> Result<Vec<Title>, ProviderError> {
        let state = self.state();
        state.reachable()?;
        let films = state.films.iter().filter(|f| f.resume.is_some());
        let episodes = state.episodes.iter().filter(|e| e.item.resume.is_some());
        Ok(films
            .cloned()
            .map(Title::Film)
            .chain(episodes.cloned().map(Title::Episode))
            .collect())
    }

    fn film(&self, film: &ItemId) -> Result<FilmDetail, ProviderError> {
        let found = self.find(film)?;
        self.state().record.film_pages.push(film.clone());
        let others = self.state().films.clone();
        let tmdb = state_tmdb(&self.state().tmdb, &found.id);
        let trailers = self.state().trailers.clone();
        let downloadable = !self.state().not_downloadable.contains(&found.id);
        Ok(FilmDetail {
            tmdb,
            tmdb_collection: tmdb.and_then(|_| self.state().tmdb_collection),
            overview: Some(FILM_OVERVIEW.into()),
            genres: vec!["Drame".into(), "Aventure".into()],
            official_rating: Some("12+".into()),
            directors: vec!["Inès Vallard".into()],
            cast: CAST
                .iter()
                .enumerate()
                .map(|(n, (name, role))| CastMember {
                    id: ItemId(format!("person-{n}")),
                    name: (*name).into(),
                    role: Some((*role).into()),
                    // Some people have no photo on the server.
                    photo: (n % 4 != 3)
                        .then(|| image(&format!("person-{n}"), ImageKind::Primary))
                        .flatten(),
                })
                .collect(),
            similar: others
                .into_iter()
                .filter(|f| f.id != found.id)
                .skip(4)
                .take(8)
                .collect(),
            versions: if found.id.0 == FILM {
                versions()
            } else {
                versions().split_off(1)
            },
            downloadable,
            favourite: self.state().favourites.contains(&found.id),
            trailers,
            // The first film of the tests is a logo, the others are written, as on the Home.
            logo: (found.id.0 == FILM)
                .then(|| image(&found.id.0, ImageKind::Logo))
                .flatten(),
            film: found,
        })
    }

    fn collection_of(&self, film: &ItemId) -> Result<Option<Collection>, ProviderError> {
        if film.0 != FILM {
            return Ok(None);
        }
        let saga = self.collection(&ItemId("saga".into()))?;
        Ok(Some(Collection {
            id: saga.id,
            name: saga.name,
            others: saga.films.into_iter().filter(|f| &f.id != film).collect(),
        }))
    }

    fn play(&self, film: &ItemId, source: &MediaSourceId) -> Result<Playback, ProviderError> {
        let external_subtitles = self.external_subtitles(film, source)?;
        let mut state = self.state();
        state.record.plays.push((film.clone(), source.clone()));
        if let Some(error) = state.play_error.clone() {
            return Err(error);
        }
        Ok(Playback {
            session: PlaySessionId(PLAY_SESSION.into()),
            film: film.clone(),
            source: source.clone(),
            stream: StreamUrl::new(state.stream.clone()),
            external_subtitles,
            converted: false,
        })
    }

    fn report_started(&self, report: &PlaybackReport) -> Result<(), ProviderError> {
        self.state()
            .record
            .reports
            .push(Report::Started(report.clone()));
        Ok(())
    }

    fn report_progress(&self, report: &PlaybackReport) -> Result<(), ProviderError> {
        self.state()
            .record
            .reports
            .push(Report::Progress(report.clone()));
        Ok(())
    }

    fn report_stopped(&self, report: &PlaybackReport) -> Result<(), ProviderError> {
        let delay = self.state().stop_delay;
        std::thread::sleep(delay);
        let mut guard = self.state();
        let state = &mut *guard;
        state.record.reports.push(Report::Stopped(report.clone()));
        // Like Jellyfin: past nine tenths of the film, it is watched and has no resume point.
        let episodes = state.episodes.iter_mut().map(|e| &mut e.item);
        if let Some(film) = state
            .films
            .iter_mut()
            .chain(episodes)
            .find(|f| f.id == report.film)
        {
            let runtime = film.runtime.unwrap_or_default();
            if report.position.as_secs_f64() >= runtime.as_secs_f64() * 0.9 {
                film.watched = true;
                film.resume = None;
            } else {
                film.resume = Some(report.position);
            }
        }
        Ok(())
    }

    fn set_watched(&self, film: &ItemId, watched: bool) -> Result<(), ProviderError> {
        let mut guard = self.state();
        let state = &mut *guard;
        state.record.watched.push((film.clone(), watched));
        let episodes = state.episodes.iter_mut();
        // A season or a series marks all its episodes.
        let marked = episodes
            .filter(|e| &e.item.id == film || &e.season == film || &e.series == film)
            .map(|e| &mut e.item);
        for found in state
            .films
            .iter_mut()
            .filter(|f| &f.id == film)
            .chain(marked)
        {
            found.watched = watched;
            if watched {
                found.resume = None;
            }
        }
        Ok(())
    }

    fn image(&self, image: &ImageRef, width: u32) -> Result<Vec<u8>, ProviderError> {
        let (delay, fails) = {
            let mut state = self.state();
            let asked_before = state.record.images.iter().any(|(i, _)| i == image);
            state.record.images.push((image.clone(), width));
            let fails = state.images_fail
                || (state.photos_fail_once && image.item.0.starts_with("person-") && !asked_before);
            (state.image_delay, fails)
        };
        std::thread::sleep(delay);
        if fails {
            return Err(ProviderError::Unreachable("timed out".into()));
        }
        let height = match image.kind {
            ImageKind::Primary => width * 3 / 2,
            ImageKind::Backdrop => width * 9 / 16,
            ImageKind::Logo => width / 3,
        };
        Ok(gradient(&image.item.0, width.max(1), height.max(1)))
    }

    fn logout(&self) -> Result<(), ProviderError> {
        let delay = self.state().logout_delay;
        std::thread::sleep(delay);
        let mut state = self.state();
        state.kept = false;
        let reports = state.record.reports.len();
        state.record.logouts.push(reports);
        // As Jellyfin, only the logout writes it down; the server is the one the tests and the
        // harness invent.
        state.last_address = Some(SERVER_ADDRESS.to_string());
        state.logout_error.clone().map_or(Ok(()), Err)
    }

    fn series(&self, _: &ItemId, query: &FilmQuery) -> Result<Page<Series>, ProviderError> {
        let state = self.state();
        let counted: Vec<Series> = state
            .series
            .iter()
            .map(|series| Self::counted(&state, series))
            .collect();
        let (items, total) = select(&counted, query, |series| Facts {
            title: series.title.clone(),
            year: series.year,
            rating: series.community_rating,
            watched: series.unwatched == 0,
            genre: genre_of(&series.id),
        });
        Ok(Page { items, total })
    }

    fn series_detail(&self, series: &ItemId) -> Result<SeriesDetail, ProviderError> {
        let state = self.state();
        let found = state
            .series
            .iter()
            .find(|s| &s.id == series)
            .ok_or(ProviderError::NotFound)?;
        Ok(SeriesDetail {
            tmdb: state_tmdb(&state.tmdb, series),
            series: Self::counted(&state, found),
            favourite: state.favourites.contains(series),
            trailers: state.trailers.clone(),
            // As on the Home, the first series is a logo and the others are written.
            logo: (state.series.first().map(|s| &s.id) == Some(series))
                .then(|| image(&series.0, ImageKind::Logo))
                .flatten(),
            overview: Some(SERIES_OVERVIEW.into()),
            genres: vec!["Science-fiction".into(), "Drame".into()],
            official_rating: Some("12+".into()),
            cast: CAST
                .iter()
                .take(8)
                .enumerate()
                .map(|(n, (name, role))| CastMember {
                    id: ItemId(format!("person-{n}")),
                    name: (*name).into(),
                    role: Some((*role).into()),
                    photo: image(&format!("person-{n}"), ImageKind::Primary),
                })
                .collect(),
            similar: state
                .series
                .iter()
                .filter(|s| &s.id != series)
                .map(|s| Self::counted(&state, s))
                .collect(),
        })
    }

    fn seasons(&self, series: &ItemId) -> Result<Vec<Season>, ProviderError> {
        let state = self.state();
        let mut seasons: Vec<Season> = Vec::new();
        for episode in state.episodes.iter().filter(|e| &e.series == series) {
            if seasons.last().is_none_or(|s| s.id != episode.season) {
                seasons.push(Season {
                    id: episode.season.clone(),
                    name: format!("Saison {}", episode.season_number.unwrap_or(0)),
                    number: episode.season_number,
                    unwatched: 0,
                });
            }
            if !episode.item.watched {
                seasons.last_mut().expect("pushed above").unwatched += 1;
            }
        }
        Ok(seasons)
    }

    fn episodes(&self, series: &ItemId, season: &ItemId) -> Result<Vec<Episode>, ProviderError> {
        let state = self.state();
        Ok(state
            .episodes
            .iter()
            .filter(|e| &e.series == series && &e.season == season)
            .cloned()
            .collect())
    }

    fn title(&self, item: &ItemId) -> Result<Title, ProviderError> {
        self.state().record.titles.push(item.clone());
        self.state().reachable()?;
        if let Some(episode) = self.episode(item) {
            return Ok(Title::Episode(episode));
        }
        let state = self.state();
        if let Some(series) = state.series.iter().find(|series| &series.id == item) {
            return Ok(Title::Series(Self::counted(&state, series)));
        }
        let film = state.films.iter().find(|film| &film.id == item);
        film.cloned()
            .map(Title::Film)
            .ok_or(ProviderError::NotFound)
    }

    /// Like Jellyfin's Next Up: the episode started, else the one after the last watched, across
    /// seasons; the first when none was watched; none once the last was. Specials are left out.
    fn next_up(&self, series: &ItemId) -> Result<Option<Episode>, ProviderError> {
        let state = self.state();
        let episodes: Vec<&Episode> = state
            .episodes
            .iter()
            .filter(|e| &e.series == series && e.season_number != Some(0))
            .collect();
        if let Some(started) = episodes
            .iter()
            .find(|e| e.item.resume.is_some() && !e.item.watched)
        {
            return Ok(Some((*started).clone()));
        }
        let after = episodes
            .iter()
            .rposition(|e| e.item.watched)
            .map_or(0, |last| last + 1);
        Ok(episodes.get(after).map(|e| (*e).clone()))
    }
    fn search(&self, query: &str, per_group: u32) -> Result<SearchResults, ProviderError> {
        let delay = {
            let mut state = self.state();
            state.record.searches.push(query.to_string());
            state.search_delay
        };
        std::thread::sleep(delay);
        let state = self.state();
        state.reachable()?;
        let needle = query.to_lowercase();
        let matching = |name: &str| name.to_lowercase().contains(&needle);
        let limit = per_group as usize;
        Ok(SearchResults {
            films: state
                .films
                .iter()
                .filter(|f| matching(&f.title))
                .take(limit)
                .cloned()
                .collect(),
            series: state
                .series
                .iter()
                .filter(|s| matching(&s.title))
                .take(limit)
                .map(hit)
                .collect(),
            people: state
                .people
                .iter()
                .filter(|p| matching(&p.name))
                .take(limit)
                .cloned()
                .collect(),
        })
    }

    /// Like Jellyfin: the next episode of each series the user has begun, never one of a
    /// series they have not touched, started episodes included.
    fn next_episodes(&self, limit: u32) -> Result<Vec<Episode>, ProviderError> {
        let begun: Vec<ItemId> = {
            let state = self.state();
            state.reachable()?;
            state
                .series
                .iter()
                .filter(|series| {
                    state.episodes.iter().any(|e| {
                        e.series == series.id && (e.item.watched || e.item.resume.is_some())
                    })
                })
                .map(|series| series.id.clone())
                .collect()
        };
        let mut next = Vec::new();
        for series in begun.iter().take(limit as usize) {
            next.extend(self.next_up(series)?);
        }
        Ok(next)
    }

    /// Like Jellyfin asked for unplayed ones: the newest first, the played left out.
    fn new_episodes(&self, start: u32, limit: u32) -> Result<Page<NewEpisode>, ProviderError> {
        let mut state = self.state();
        state.record.new_episode_asks += 1;
        let mut found: Vec<NewEpisode> = state
            .added
            .iter()
            .filter_map(|(id, added)| {
                let episode = state.episodes.iter().find(|e| &e.item.id == id)?;
                Some(NewEpisode {
                    poster: image(&episode.series.0, ImageKind::Primary),
                    episode: episode.clone(),
                    added: *added,
                })
            })
            .filter(|new| !new.episode.item.watched)
            .collect();
        found.sort_by_key(|new| std::cmp::Reverse(new.added));
        let total = found.len() as u32;
        if start == 0
            && let Some(change) = state.between_pages.take()
        {
            change(&mut state);
        }
        Ok(Page {
            items: found
                .into_iter()
                .skip(start as usize)
                .take(limit as usize)
                .collect(),
            total,
        })
    }

    fn latest(&self, library: &ItemId, limit: u32) -> Result<Vec<Title>, ProviderError> {
        let state = self.state();
        let limit = limit as usize;
        Ok(match library.0.as_str() {
            FILMS_LIBRARY => state
                .films
                .iter()
                .take(limit)
                .cloned()
                .map(Title::Film)
                .collect(),
            _ => state
                .series
                .iter()
                .take(limit)
                .map(|series| Title::Series(Self::counted(&state, series)))
                .collect(),
        })
    }

    /// The films and the series the user has not finished, one of each in turn, the second
    /// with its title as a logo.
    fn featured(&self, limit: u32) -> Result<Vec<Feature>, ProviderError> {
        let state = self.state();
        let films = state
            .films
            .iter()
            .filter(|f| !f.watched)
            .map(|film| Feature {
                kind: FeatureKind::Film,
                id: film.id.clone(),
                title: film.title.clone(),
                year: film.year,
                community_rating: film.community_rating,
                official_rating: Some("12+".into()),
                runtime: film.runtime,
                overview: Some(FILM_OVERVIEW.into()),
                backdrop: film.backdrop.clone(),
                logo: None,
                resume: film.resume,
            });
        let series = state.series.iter().enumerate().map(|(n, series)| Feature {
            kind: FeatureKind::Series,
            id: series.id.clone(),
            title: series.title.clone(),
            year: series.year,
            community_rating: series.community_rating,
            official_rating: Some("16+".into()),
            runtime: None,
            overview: Some(SERIES_OVERVIEW.into()),
            backdrop: series.backdrop.clone(),
            // One title is a logo, the others are written: both are drawn.
            logo: (n == 0)
                .then(|| image(&series.id.0, ImageKind::Logo))
                .flatten(),
            resume: None,
        });
        let mut films = films.peekable();
        let mut series = series.peekable();
        let mut turn = true;
        let mut features = Vec::new();
        while features.len() < limit as usize && (films.peek().is_some() || series.peek().is_some())
        {
            let next = if turn { films.next() } else { series.next() };
            features.extend(next);
            turn = !turn;
        }
        Ok(features)
    }

    fn person_titles(&self, person: &ItemId) -> Result<PersonTitles, ProviderError> {
        let state = self.state();
        let n = state
            .people
            .iter()
            .position(|p| &p.id == person)
            .ok_or(ProviderError::NotFound)?;
        Ok(PersonTitles {
            films: state
                .films
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 6 == n % 6)
                .take(7)
                .map(|(_, f)| f.clone())
                .collect(),
            series: state
                .series
                .iter()
                .enumerate()
                .filter(|(i, _)| i % 2 == n % 2)
                .map(|(_, s)| hit(&Self::counted(&state, s)))
                .collect(),
        })
    }

    fn collections(&self) -> Result<Vec<CollectionCard>, ProviderError> {
        let state = self.state();
        let mut cards: Vec<CollectionCard> = (0..COLLECTION_NAMES.len())
            .map(|n| {
                let collection = collection(&state, n);
                CollectionCard {
                    films: Some(collection.films.len() as u32),
                    // Bare when it has no primary image, like Jellyfin's: `collection_poster`
                    // gives the first film's.
                    poster: has_own_picture(n)
                        .then(|| image(&collection.id.0, ImageKind::Primary))
                        .flatten(),
                    id: collection.id,
                    name: collection.name,
                }
            })
            .collect();
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(cards)
    }

    fn collection_poster(&self, id: &ItemId) -> Result<Option<ImageRef>, ProviderError> {
        {
            let mut state = self.state();
            state.collection_posters_asked.push(id.clone());
            if let Some(error) = state.collection_poster_error.clone() {
                return Err(error);
            }
        }
        Ok(self
            .collection(id)?
            .films
            .iter()
            .find_map(|film| film.poster.clone()))
    }

    fn collection(&self, id: &ItemId) -> Result<CollectionDetail, ProviderError> {
        let state = self.state();
        (0..COLLECTION_NAMES.len())
            .map(|n| collection(&state, n))
            .find(|collection| &collection.id == id)
            .ok_or(ProviderError::NotFound)
    }

    fn favourites(&self) -> Result<Vec<Favourite>, ProviderError> {
        let mut state = self.state();
        state.record.list_loads += 1;
        let mut found: Vec<Favourite> = state
            .films
            .iter()
            .filter(|f| state.favourites.contains(&f.id))
            .cloned()
            .map(Favourite::Film)
            .chain(
                state
                    .series
                    .iter()
                    .filter(|s| state.favourites.contains(&s.id))
                    .map(|s| Favourite::Series(Self::counted(&state, s))),
            )
            .collect();
        found.sort_by_key(|favourite| match favourite {
            Favourite::Film(film) => film.title.to_lowercase(),
            Favourite::Series(series) => series.title.to_lowercase(),
        });
        Ok(found)
    }

    fn set_favourite(&self, item: &ItemId, favourite: bool) -> Result<(), ProviderError> {
        let delay = self.state().favourite_delay;
        std::thread::sleep(delay);
        let mut state = self.state();
        state.record.favourite_calls.push((item.clone(), favourite));
        if state.favourite_fails {
            return Err(ProviderError::Unreachable("timed out".into()));
        }
        state.favourites.retain(|id| id != item);
        if favourite {
            state.favourites.push(item.clone());
        }
        for found in state.films.iter_mut().filter(|f| &f.id == item) {
            found.favourite = favourite;
        }
        for found in state.series.iter_mut().filter(|s| &s.id == item) {
            found.favourite = Some(favourite);
        }
        Ok(())
    }

    fn playlists(&self, first: Option<usize>) -> Result<Vec<Playlist>, ProviderError> {
        let mut state = self.state();
        state.record.playlists_listed += 1;
        let mut found: Vec<Playlist> = state
            .playlists
            .iter()
            .map(|playlist| {
                let mut entries = Self::entries(&state, playlist);
                let count = entries.len();
                entries.truncate(first.unwrap_or(count));
                Playlist {
                    id: playlist.id.clone(),
                    name: playlist.name.clone(),
                    entries,
                    count,
                }
            })
            .collect();
        found.sort_by_key(|playlist| playlist.name.to_lowercase());
        Ok(found)
    }

    fn playlist(&self, playlist: &ItemId) -> Result<Vec<PlaylistEntry>, ProviderError> {
        let mut state = self.state();
        state.record.playlist_reads.push(playlist.clone());
        if state.unreadable_playlists.contains(playlist) {
            return Err(ProviderError::Forbidden);
        }
        let found = state
            .playlists
            .iter()
            .find(|p| &p.id == playlist)
            .ok_or(ProviderError::NotFound)?;
        Ok(Self::entries(&state, found))
    }

    fn create_playlist(&self, name: &str, item: &ItemId) -> Result<(ItemId, usize), ProviderError> {
        let mut state = self.state();
        Self::change_playlist(&mut state, PlaylistCall::Create(name.into(), item.clone()))?;
        state.playlists_made += 1;
        let id = ItemId(format!("playlist-new-{}", state.playlists_made));
        let entries: Vec<_> = Self::put_in(&state, item)
            .into_iter()
            .enumerate()
            .map(|(n, item)| (EntryId(format!("{}-entry-{n}", id.0)), item))
            .collect();
        let count = entries.len();
        state.playlists.push(KeptPlaylist {
            id: id.clone(),
            name: name.into(),
            entries,
        });
        Ok((id, count))
    }

    fn add_to_playlist(&self, playlist: &ItemId, item: &ItemId) -> Result<usize, ProviderError> {
        let mut state = self.state();
        Self::change_playlist(
            &mut state,
            PlaylistCall::Add(playlist.clone(), item.clone()),
        )?;
        let added = Self::put_in(&state, item);
        let found = state
            .playlists
            .iter_mut()
            .find(|p| &p.id == playlist)
            .ok_or(ProviderError::NotFound)?;
        for item in &added {
            let entry = EntryId(format!(
                "{}-entry-{}",
                playlist.0,
                found.entries.len() + 100
            ));
            found.entries.push((entry, item.clone()));
        }
        Ok(added.len())
    }

    fn remove_from_playlist(
        &self,
        playlist: &ItemId,
        entry: &EntryId,
    ) -> Result<(), ProviderError> {
        let mut state = self.state();
        Self::change_playlist(
            &mut state,
            PlaylistCall::Remove(playlist.clone(), entry.clone()),
        )?;
        let found = state
            .playlists
            .iter_mut()
            .find(|p| &p.id == playlist)
            .ok_or(ProviderError::NotFound)?;
        found.entries.retain(|(id, _)| id != entry);
        Ok(())
    }

    fn trickplay(
        &self,
        _film: &ItemId,
        _source: &MediaSourceId,
    ) -> Result<Option<Trickplay>, ProviderError> {
        Ok(self.state().trickplay)
    }

    fn trickplay_tile(
        &self,
        _film: &ItemId,
        _source: &MediaSourceId,
        trickplay: &Trickplay,
        tile: u32,
    ) -> Result<Vec<u8>, ProviderError> {
        let delay = self.state().tile_delay;
        std::thread::sleep(delay);
        let mut state = self.state();
        state.record.tiles.push(tile);
        if tile >= trickplay.tiles() {
            return Err(ProviderError::NotFound);
        }
        let at = tile as usize % state.tiles.len().max(1);
        state.tiles.get(at).cloned().ok_or(ProviderError::NotFound)
    }

    fn transcode(&self, request: &TranscodeRequest) -> Result<Playback, ProviderError> {
        let mut external_subtitles = self.external_subtitles(&request.film, &request.source)?;
        // The fake's embedded subtitles are all text: the server extracts the one chosen.
        external_subtitles.extend(request.subtitle_track.map(|index| ExternalSubtitle {
            index,
            url: StreamUrl::new(format!("file:///nowhere-converted-{index}.srt")),
        }));
        let mut state = self.state();
        state.record.transcodes.push(request.clone());
        if state.transcode_fails {
            return Err(ProviderError::BadAnswer("converts nothing".into()));
        }
        Ok(Playback {
            session: PlaySessionId(TRANSCODE_SESSION.into()),
            film: request.film.clone(),
            source: request.source.clone(),
            stream: StreamUrl::new(state.converted.clone()),
            external_subtitles,
            converted: true,
        })
    }

    fn media_segments(&self, item: &ItemId) -> Result<Vec<Segment>, ProviderError> {
        let mut state = self.state();
        state.record.segment_asks.push(item.clone());
        Ok(state.segments.clone())
    }

    fn tmdb_index(&self) -> Result<TmdbIndex, ProviderError> {
        let mut state = self.state();
        state.record.tmdb_indexes += 1;
        if let Some(error) = state.tmdb_index_error.clone() {
            return Err(error);
        }
        let mut index = TmdbIndex::default();
        for (item, tmdb) in &state.tmdb {
            if state.series.iter().any(|series| &series.id == item) {
                index.series.insert(*tmdb, item.clone());
            } else {
                index.films.insert(*tmdb, item.clone());
            }
        }
        Ok(index)
    }

    fn local_trailer(&self, item: &ItemId) -> Result<StreamUrl, ProviderError> {
        let mut state = self.state();
        state.record.trailer_asks.push(item.clone());
        if let Some(error) = state.trailer_error.clone() {
            return Err(error);
        }
        Ok(StreamUrl::new(state.trailer_stream.clone()))
    }

    fn watch_history(&self) -> Result<WatchHistory, ProviderError> {
        let mut state = self.state();
        state.record.histories += 1;
        if let Some(error) = state.history_error.clone() {
            return Err(error);
        }
        Ok(state.history.clone())
    }

    fn download_plan(
        &self,
        item: &ItemId,
        source: &MediaSourceId,
    ) -> Result<DownloadPlan, ProviderError> {
        let versions = match self.episode(item) {
            Some(episode) => episode.versions,
            None => self.film(item)?.versions,
        };
        let version = versions
            .iter()
            .find(|s| &s.id == source)
            .ok_or(ProviderError::NotFound)?;
        let handle = format!("{}/{}", item.0, source.0);
        let media = DownloadFile {
            kind: DownloadKind::Media,
            handle: handle.clone(),
        };
        let subtitles = version
            .subtitles
            .iter()
            .filter(|s| s.external)
            .map(|s| DownloadFile {
                kind: DownloadKind::Subtitle {
                    index: s.index,
                    format: "srt".into(),
                },
                handle: handle.clone(),
            });
        let state = self.state();
        Ok(DownloadPlan {
            size: Some(state.download_size),
            version: state.download_version.clone(),
            container: version.container.clone(),
            files: std::iter::once(media).chain(subtitles).collect(),
        })
    }

    /// Serves the bytes `download_bytes` gives, from `from` when `validator` is the file's own.
    fn fetch(
        &self,
        file: &DownloadFile,
        from: u64,
        validator: Option<&str>,
        sink: &mut dyn Sink,
    ) -> Result<Fetched, ProviderError> {
        let (bytes, etag, delay) = {
            let mut state = self.state();
            state
                .record
                .fetches
                .push((file.clone(), from, validator.map(str::to_string)));
            state.fetching += 1;
            state.record.most_fetches_at_once =
                state.record.most_fetches_at_once.max(state.fetching);
            let bytes = match &file.kind {
                DownloadKind::Media => download_bytes(&file.handle, state.download_size),
                DownloadKind::Subtitle { index, .. } => {
                    format!("{index}\n00:00:01,000 --> 00:00:02,000\nBonjour\n").into_bytes()
                }
            };
            (
                bytes,
                format!("\"{}\"", state.download_version),
                state.fetch_delay,
            )
        };
        let resumed = from > 0 && validator == Some(etag.as_str());
        let fetched = Fetched {
            resumed,
            validator: Some(etag),
            length: Some(bytes.len() as u64),
        };
        let media = file.kind == DownloadKind::Media;
        let written = |e: std::io::Error| ProviderError::Write(e.kind());
        let sent = (|| {
            sink.start(&fetched).map_err(written)?;
            let mut at = if resumed { from as usize } else { 0 };
            for piece in bytes[at.min(bytes.len())..].chunks(4) {
                if media {
                    let full = self
                        .state()
                        .disk_full_after
                        .take_if(|after| at as u64 >= *after);
                    if full.is_some() {
                        return Err(ProviderError::Write(std::io::ErrorKind::StorageFull));
                    }
                    let hook = self
                        .state()
                        .mid_download
                        .take_if(|(after, _)| at as u64 >= *after);
                    if let Some((_, hook)) = hook {
                        hook();
                    }
                }
                std::thread::sleep(delay);
                sink.write_all(piece).map_err(written)?;
                at += piece.len();
            }
            sink.flush().map_err(written)?;
            Ok(fetched.clone())
        })();
        self.state().fetching -= 1;
        sent
    }
}

/// The bytes of the media file of a download handle, `size` of them: a test compares them.
pub fn download_bytes(handle: &str, size: u64) -> Vec<u8> {
    handle.bytes().cycle().take(size as usize).collect()
}

const COLLECTION_NAMES: [&str; 10] = [
    "Saga Hiver",
    "Trilogie du Phare",
    "Les Voyageurs",
    "Cycle des Marées",
    "Tétralogie Sable",
    "Mémoires d'Orbite",
    "Les Veilleurs du Nord",
    "Diptyque Brume",
    "Chroniques de Lagune",
    "Série Noire",
];

/// Every fourth collection has no picture of its own, to show the first film's poster in its
/// place.
fn has_own_picture(n: usize) -> bool {
    n % 4 != 3
}

/// The collection number `n`. The first, "Saga Hiver", holds the film of the tests with the
/// film before it; the others take films by their place in the list.
fn collection(state: &State, n: usize) -> CollectionDetail {
    let id = if n == 0 {
        "saga".to_string()
    } else {
        format!("collection-{n}")
    };
    let pick = |i: usize| state.films.get(i % state.films.len().max(1)).cloned();
    let mut films: Vec<Film> = if n == 0 {
        let first = pick(6).map(|film| Film {
            title: "Le Premier Hiver".into(),
            year: Some(2019),
            watched: true,
            ..film
        });
        pick(0).into_iter().chain(first).collect()
    } else {
        (0..2 + n % 3).filter_map(|k| pick(n * 3 + k)).collect()
    };
    films.sort_by_key(|film| film.year);
    CollectionDetail {
        name: COLLECTION_NAMES[n].into(),
        overview: Some(
            "Des films qui se répondent d'un bout à l'autre, réunis dans l'ordre où ils \
             sont sortis."
                .into(),
        ),
        backdrop: has_own_picture(n)
            .then(|| image(&id, ImageKind::Backdrop))
            .flatten(),
        films,
        id: ItemId(id),
    }
}

/// A BMP image: a diagonal gradient whose colours come from `seed`, so each film has its own.
pub fn gradient(seed: &str, width: u32, height: u32) -> Vec<u8> {
    let hash = seed.bytes().fold(2166136261u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(16777619)
    });
    let from = [
        hash as u8 / 2 + 30,
        (hash >> 8) as u8 / 2 + 20,
        (hash >> 16) as u8 / 2 + 40,
    ];
    let to = [8u8, 8, 12];
    let row = (width * 3).div_ceil(4) * 4;
    let size = 54 + row * height;
    let mut bmp = Vec::with_capacity(size as usize);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&size.to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    // Negative: the rows go top to bottom.
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&24u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    for y in 0..height {
        for x in 0..width {
            let t = (x as f32 / width as f32 + y as f32 / height as f32) / 2.;
            let mix = |c: usize| (f32::from(from[c]) * (1. - t) + f32::from(to[c]) * t) as u8;
            bmp.extend_from_slice(&[mix(2), mix(1), mix(0)]);
        }
        bmp.resize(bmp.len() + (row - width * 3) as usize, 0);
    }
    bmp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library_tools::{Sort, Tools};
    use bobine::provider::Filters;

    fn titles(page: &Page<Series>) -> Vec<&str> {
        page.items.iter().map(|s| s.title.as_str()).collect()
    }

    #[test]
    fn a_series_marked_watched_has_all_its_episodes_watched() {
        let server = FakeServer::new(3, true);
        let series = ItemId("series-0".into());
        let shown = |server: &FakeServer| {
            let state = server.state();
            FakeServer::counted(&state, &state.series[0])
        };
        server.set_watched(&series, true).unwrap();
        assert_eq!(
            (shown(&server).unwatched, shown(&server).watched),
            (0, Some(true))
        );
        server.set_watched(&series, false).unwrap();
        assert_eq!(shown(&server).watched, Some(false));
        assert!(shown(&server).unwatched > 0);
    }

    #[test]
    fn the_fake_filters_and_sorts_like_the_server() {
        let server = FakeServer::new(40, true);
        let library = ItemId(FILMS_LIBRARY.into());
        let drama = Tools {
            filters: Filters {
                genre: Some("Drame".into()),
                decade: Some(2020),
                unwatched: true,
            },
            sort: Sort::Rating,
        };
        let page = server.films(&library, &drama.query(0)).unwrap();
        assert!(page.total > 0 && page.total < 40);
        assert!(
            page.items
                .iter()
                .all(|f| !f.watched && f.year >= Some(2020))
        );
        assert!(
            page.items
                .windows(2)
                .all(|w| w[0].community_rating >= w[1].community_rating)
        );
    }

    #[test]
    fn the_fake_counts_the_titles_before_a_letter_in_its_own_order() {
        let server = FakeServer::new(1, true);
        let tools = Tools {
            sort: Sort::Title,
            ..Tools::default()
        };
        let id = ItemId("series".into());
        let all = server.series(&id, &tools.query(0)).unwrap();
        let all = titles(&all);
        for letter in 'A'..='Z' {
            let before = server
                .series(&id, &tools.letter_query(letter).unwrap())
                .unwrap()
                .total as usize;
            let sorts_before =
                |title: &&str| title.to_lowercase() < letter.to_lowercase().to_string();
            assert!(all[..before].iter().all(sorts_before), "{letter}");
            assert!(!all[before..].iter().any(sorts_before), "{letter}");
        }
    }

    #[test]
    fn the_played_items_hold_one_dated_play_of_a_title_and_the_rest_undated() {
        let history = watch_history(HistorySource::PlayedItems, NOW);
        let mut seen: Vec<&ItemId> = Vec::new();
        for play in &history.plays {
            let id = play.title.id.as_ref().unwrap();
            let dated = play.at.is_some();
            assert_eq!(dated, !seen.contains(&id), "{}", play.title.name);
            if dated {
                assert_eq!(play.count, 1, "{}", play.title.name);
                seen.push(id);
            }
        }
        assert!(history.plays.iter().any(|play| play.at.is_none()));
    }
}
