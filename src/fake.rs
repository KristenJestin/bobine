//! A media server in memory, with invented films: the tests drive the flows with it, and the
//! headless harness shows the screens with it. It records what it is asked (logins, reports,
//! watched marks) and never touches a network, a keyring or a file.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobine::provider::{
    AudioTrack, CastMember, Collection, Connector, Episode, ExternalSubtitle, Film, FilmDetail,
    FilmQuery, ImageKind, ImageRef, ItemId, Library, LibraryKind, MediaServer, MediaSource,
    MediaSourceId, Page, PlaySessionId, Playback, PlaybackReport, ProviderError, Season, Series,
    SeriesDetail, ServerInfo, StreamUrl, SubtitleTrack, VideoRange, VideoTrack,
};

pub const USER: &str = "camille";
pub const PASSWORD: &str = "marée haute";
pub const FILMS_LIBRARY: &str = "films";
/// The film every page of the tests opens: started, with two versions and three subtitles.
pub const FILM: &str = "film-0";
pub const PLAY_SESSION: &str = "session-1";

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
    "Abysse",
    "Ligne rouge",
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

/// The seasons of a series and their number of episodes: the first series has three, as in the
/// design, the others one.
fn season_sizes(series: usize) -> &'static [u32] {
    if series == 0 { &[3, 6, 5] } else { &[4] }
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

/// The episodes of every series. The first series: season 1 watched, season 2 watched up to
/// its third episode, its fourth stopped at 30 min of 48.
pub fn episodes() -> Vec<Episode> {
    let mut all = Vec::new();
    for (n, title) in SERIES_TITLES.iter().enumerate() {
        for (s, size) in season_sizes(n).iter().enumerate() {
            let season = s as u32 + 1;
            for number in 1..=*size {
                let id = format!("series-{n}-s{season}-e{number}");
                let watched = n == 0 && (season == 1 || (season == 2 && number <= 3));
                let resume =
                    (n == 0 && season == 2 && number == 4).then(|| Duration::from_secs(30 * 60));
                all.push(Episode {
                    item: Film {
                        title: EPISODE_TITLES[(number as usize + s) % EPISODE_TITLES.len()].into(),
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

    fn films(&self, _: &ItemId, query: &FilmQuery) -> Result<Page<Film>, ProviderError> {
        let films = &self.state().films;
        Ok(Page {
            items: films
                .iter()
                .skip(query.start as usize)
                .take(query.limit as usize)
                .cloned()
                .collect(),
            total: films.len() as u32,
        })
    }

    fn continue_watching(&self) -> Result<Vec<Film>, ProviderError> {
        Ok(self
            .state()
            .films
            .iter()
            .filter(|f| f.resume.is_some())
            .cloned()
            .collect())
    }

    fn film(&self, film: &ItemId) -> Result<FilmDetail, ProviderError> {
        let found = self.find(film)?;
        self.state().record.film_pages.push(film.clone());
        let others = self.state().films.clone();
        Ok(FilmDetail {
            overview: Some(
                "Trois frères traversent un plateau gelé pour rapporter le corps de leur père, \
                 avant que la route ne ferme pour l'hiver."
                    .into(),
            ),
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
            film: found,
        })
    }

    fn collection_of(&self, film: &ItemId) -> Result<Option<Collection>, ProviderError> {
        if film.0 != FILM {
            return Ok(None);
        }
        let mut first = self.find(&ItemId("film-6".into()))?;
        first.title = "Le Premier Hiver".into();
        first.year = Some(2019);
        first.watched = true;
        Ok(Some(Collection {
            id: ItemId("saga".into()),
            name: "Saga Hiver".into(),
            others: vec![first],
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
        };
        Ok(gradient(&image.item.0, width.max(1), height.max(1)))
    }

    fn logout(&self) -> Result<(), ProviderError> {
        self.state().kept = false;
        Ok(())
    }

    fn series(&self, _: &ItemId, query: &FilmQuery) -> Result<Page<Series>, ProviderError> {
        let state = self.state();
        Ok(Page {
            items: state
                .series
                .iter()
                .skip(query.start as usize)
                .take(query.limit as usize)
                .map(|series| Self::counted(&state, series))
                .collect(),
            total: state.series.len() as u32,
        })
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
            overview: Some(
                "Sous la glace de l'Arctique, l'équipage d'une station de recherche perd le \
                 contact avec la surface et découvre qu'il n'est pas seul."
                    .into(),
            ),
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
    /// seasons; the first when none was watched; none once the last was.
    fn next_up(&self, series: &ItemId) -> Result<Option<Episode>, ProviderError> {
        let state = self.state();
        let episodes: Vec<&Episode> = state
            .episodes
            .iter()
            .filter(|e| &e.series == series)
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
