//! A media server in memory, with invented films: the tests drive the flows with it, and the
//! headless harness shows the screens with it. It records what it is asked (logins, reports,
//! watched marks) and never touches a network, a keyring or a file.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobine::provider::{
    AudioTrack, CastMember, Collection, CollectionCard, CollectionDetail, Connector, Episode,
    ExternalSubtitle, Favourite, Feature, FeatureKind, Film, FilmDetail, FilmQuery, FilmSort,
    FilterChoices, ImageKind, ImageRef, ItemId, Library, LibraryKind, MediaServer, MediaSource,
    MediaSourceId, Page, PersonHit, PersonTitles, PlaySessionId, Playback, PlaybackReport,
    ProviderError, SearchResults, Season, Series, SeriesDetail, SeriesHit, ServerInfo, StreamUrl,
    SubtitleTrack, Title, VideoRange, VideoTrack,
};

pub const USER: &str = "camille";
pub const PASSWORD: &str = "marée haute";
pub const FILMS_LIBRARY: &str = "films";
/// The film every page of the tests opens: started, with two versions and three subtitles.
pub const FILM: &str = "film-0";
pub const PLAY_SESSION: &str = "session-1";
/// The series every series page of the tests opens: three seasons, the first watched, the second
/// stopped in its fourth episode.
pub const SERIES: &str = "series-0";

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
    /// The queries searched, in the order they came.
    pub searches: Vec<String>,
    /// Every call that put an item in the list (`true`) or took it out, answered or failed.
    pub favourite_calls: Vec<(ItemId, bool)>,
    /// How many times the list of favourites was asked.
    pub list_loads: u32,
}

/// What the fake server holds, shared by the connector and its sessions.
pub struct State {
    /// The token the "keyring" keeps: the session restored at the next start.
    pub kept: bool,
    pub films: Vec<Film>,
    pub series: Vec<Series>,
    /// Every episode of every series, in order: series, season, number.
    pub episodes: Vec<Episode>,
    /// What `play` gives as the stream: a local file in the harness.
    pub stream: String,
    pub record: Record,
    /// How long an image takes to come: a real server makes the fetches overlap.
    pub image_delay: std::time::Duration,
    /// The first time each person's photo is asked, the server fails (as a busy server that
    /// times out while it fetches the photo from elsewhere).
    pub photos_fail_once: bool,
    pub people: Vec<PersonHit>,
    /// How long a search takes to answer: the headless harness shows the palette waiting.
    pub search_delay: std::time::Duration,
    /// What is in the user's list.
    pub favourites: Vec<ItemId>,
    /// Changing the list fails (a server that does not answer).
    pub favourite_fails: bool,
    /// How long changing the list takes, so that a double click or the optimistic state shows.
    pub favourite_delay: std::time::Duration,
}

#[derive(Clone)]
pub struct FakeServer {
    pub state: Arc<Mutex<State>>,
    info: ServerInfo,
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
                poster: image(&id, ImageKind::Primary),
                backdrop: image(&id, ImageKind::Backdrop),
                id: ItemId(id),
            }
        })
        .collect()
}

/// A series as a search or a person's page lists it.
fn hit(series: &Series) -> SeriesHit {
    SeriesHit {
        id: series.id.clone(),
        title: series.title.clone(),
        year: series.year,
        poster: series.poster.clone(),
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
                        poster: image(&id, ImageKind::Primary),
                        backdrop: None,
                        id: ItemId(id),
                    },
                    series: ItemId(format!("series-{n}")),
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
        Self {
            state: Arc::new(Mutex::new(State {
                kept,
                films: films(count),
                series: series(),
                episodes: episodes(),
                stream: "file:///nowhere.mkv".into(),
                record: Record::default(),
                image_delay: std::time::Duration::ZERO,
                photos_fail_once: false,
                people: people(),
                search_delay: std::time::Duration::ZERO,
                favourites: ["film-2", "film-7", "series-1", "series-4"]
                    .map(|id| ItemId(id.into()))
                    .to_vec(),
                favourite_fails: false,
                favourite_delay: std::time::Duration::ZERO,
            })),
            info: ServerInfo {
                id: "server-1".into(),
                name: "Salon".into(),
                version: "10.11.0".into(),
            },
        }
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
        let unwatched = state
            .episodes
            .iter()
            .filter(|e| e.series == series.id && !e.item.watched)
            .count() as u32;
        Series {
            unwatched,
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

    fn find(&self, film: &ItemId) -> Result<Film, ProviderError> {
        self.state()
            .films
            .iter()
            .find(|f| &f.id == film)
            .cloned()
            .ok_or(ProviderError::NotFound)
    }

    fn session(&self) -> Arc<dyn MediaServer> {
        Arc::new(self.clone())
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
        Ok(self.state().kept.then(|| self.session()))
    }
}

impl MediaServer for FakeServer {
    fn server(&self) -> &ServerInfo {
        &self.info
    }

    fn user_name(&self) -> &str {
        USER
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
                name: "Films".into(),
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
        Ok(FilmDetail {
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
            favourite: self.state().favourites.contains(&found.id),
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
        let versions = match self.episode(film) {
            Some(episode) => episode.versions,
            None => self.film(film)?.versions,
        };
        let found = versions
            .iter()
            .find(|s| &s.id == source)
            .ok_or(ProviderError::NotFound)?;
        let mut state = self.state();
        state.record.plays.push((film.clone(), source.clone()));
        Ok(Playback {
            session: PlaySessionId(PLAY_SESSION.into()),
            film: film.clone(),
            source: source.clone(),
            stream: StreamUrl::new(state.stream.clone()),
            external_subtitles: found
                .subtitles
                .iter()
                .filter(|s| s.external)
                .map(|s| ExternalSubtitle {
                    index: s.index,
                    url: StreamUrl::new(format!("file:///nowhere-{}.srt", s.index)),
                })
                .collect(),
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
        // A season marks all its episodes.
        let marked = episodes
            .filter(|e| &e.item.id == film || &e.season == film)
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
            let fails =
                state.photos_fail_once && image.item.0.starts_with("person-") && !asked_before;
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
        self.state().kept = false;
        Ok(())
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
            series: Self::counted(&state, found),
            favourite: state.favourites.contains(series),
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
                .map(|(_, s)| hit(s))
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
                    poster: image(&collection.id.0, ImageKind::Primary),
                    id: collection.id,
                    name: collection.name,
                }
            })
            .collect();
        cards.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(cards)
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
        Ok(())
    }
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
        backdrop: image(&id, ImageKind::Backdrop),
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
}
