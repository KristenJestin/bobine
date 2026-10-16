//! A media server in memory, with invented films: the tests drive the flows with it, and the
//! headless harness shows the screens with it. It records what it is asked (logins, reports,
//! watched marks) and never touches a network, a keyring or a file.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobine::provider::{
    AudioTrack, CastMember, Collection, Connector, ExternalSubtitle, Film, FilmDetail, FilmQuery,
    ImageKind, ImageRef, ItemId, Library, LibraryKind, MediaServer, MediaSource, MediaSourceId,
    Page, PersonHit, PersonTitles, PlaySessionId, Playback, PlaybackReport, ProviderError,
    SearchResults, SeriesHit, ServerInfo, StreamUrl, SubtitleTrack, VideoRange, VideoTrack,
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
    /// The queries searched, in the order they came.
    pub searches: Vec<String>,
}

/// What the fake server holds, shared by the connector and its sessions.
pub struct State {
    /// The token the "keyring" keeps: the session restored at the next start.
    pub kept: bool,
    pub films: Vec<Film>,
    /// What `play` gives as the stream: a local file in the harness.
    pub stream: String,
    pub record: Record,
    /// How long an image takes to come: a real server makes the fetches overlap.
    pub image_delay: std::time::Duration,
    /// The first time each person's photo is asked, the server fails (as a busy server that
    /// times out while it fetches the photo from elsewhere).
    pub photos_fail_once: bool,
    pub series: Vec<SeriesHit>,
    pub people: Vec<PersonHit>,
    /// How long a search takes to answer: the headless harness shows the palette waiting.
    pub search_delay: std::time::Duration,
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

const SERIES: [(&str, i32); 6] = [
    ("Hivernage", 2022),
    ("Station Nord", 2019),
    ("Les Veilleurs", 2024),
    ("Marée basse", 2016),
    ("Hiver blanc", 2011),
    ("La Passe", 2020),
];

/// Invented series; the last has no poster.
pub fn series() -> Vec<SeriesHit> {
    SERIES
        .iter()
        .enumerate()
        .map(|(n, (title, year))| {
            let id = format!("series-{n}");
            SeriesHit {
                title: (*title).into(),
                year: Some(*year),
                poster: (n + 1 < SERIES.len())
                    .then(|| image(&id, ImageKind::Primary))
                    .flatten(),
                id: ItemId(id),
            }
        })
        .collect()
}

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
                stream: "file:///nowhere.mkv".into(),
                record: Record::default(),
                image_delay: std::time::Duration::ZERO,
                photos_fail_once: false,
                series: series(),
                people: people(),
                search_delay: std::time::Duration::ZERO,
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
        let detail = self.film(film)?;
        let found = detail
            .versions
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
        let mut state = self.state();
        state.record.reports.push(Report::Stopped(report.clone()));
        // Like Jellyfin: past nine tenths of the film, it is watched and has no resume point.
        if let Some(film) = state.films.iter_mut().find(|f| f.id == report.film) {
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
        let mut state = self.state();
        state.record.watched.push((film.clone(), watched));
        if let Some(found) = state.films.iter_mut().find(|f| &f.id == film) {
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
                .cloned()
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
                .map(|(_, s)| s.clone())
                .collect(),
        })
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
