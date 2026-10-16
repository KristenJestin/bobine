//! A request server in memory, with invented titles: the tests drive Discover and the request
//! pages with it, and the headless harness shows them with it. It records what it is asked and
//! never touches a network. Two of its films and one of its series are the fake media server's
//! (`crate::fake`), by TMDb id.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bobine::requests::{
    ActingAs, Ask, Availability, CastPerson, CollectionParts, CollectionRef, DiscoverRow,
    Discovered, MediaKind, MyRequest, RequestError, RequestServer, RequestStatus, SeasonOffer,
    ServerCheck, TitleDetail, TmdbId, TmdbImage,
};

use crate::fake::{IN_LIBRARY_FILM, IN_LIBRARY_SERIES, OTHER_LIBRARY_FILM, gradient};

pub const VERSION: &str = "2.7.3";
/// The collection of the film of the tests: two films in the library, two missing.
pub const COLLECTION: TmdbId = 500;
pub const WINTER_LIGHT: TmdbId = 9003;
pub const WINTERS_END: TmdbId = 9004;
/// A film awaiting approval.
pub const PAPER_MOONS: TmdbId = 9005;
/// A film nobody asked for.
pub const NORTHERN_LIGHTS: TmdbId = 9007;
/// A series nobody asked for: three seasons.
pub const COPPER_COAST: TmdbId = 7002;
/// A series with its first season available and its second requested.
pub const HARBOUR_WATCH: TmdbId = 7003;

#[derive(Default)]
pub struct Record {
    pub requests: Vec<Ask>,
    pub searches: Vec<String>,
    pub checks: u32,
    pub my_requests: u32,
}

pub struct State {
    pub record: Record,
    /// Every call fails with this: a server down, or a key refused.
    pub failure: Option<RequestError>,
    /// No Seerr user is the viewer: requests go as the key's owner.
    pub no_viewer: bool,
    /// How many more films may be asked for; `None`: no quota.
    pub movie_quota: Option<u32>,
    /// Asked for so far: pending from then on.
    pub requested: Vec<TmdbId>,
    /// How long each call takes, so that a page shows its waiting.
    pub delay: Duration,
}

#[derive(Clone)]
pub struct FakeSeerr {
    pub state: Arc<Mutex<State>>,
}

impl Default for FakeSeerr {
    fn default() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                record: Record::default(),
                failure: None,
                no_viewer: false,
                movie_quota: None,
                requested: Vec::new(),
                delay: Duration::ZERO,
            })),
        }
    }
}

fn title(
    kind: MediaKind,
    tmdb: TmdbId,
    name: &str,
    year: i32,
    availability: Availability,
) -> Discovered {
    Discovered {
        kind,
        tmdb,
        title: name.into(),
        year: Some(year),
        rating: Some(6.4 + (tmdb % 30) as f32 / 10.),
        overview: Some(format!(
            "An invented story told in {name}, made up for the tests and the screenshots."
        )),
        poster: Some(TmdbImage(format!("/poster-{tmdb}.jpg"))),
        backdrop: Some(TmdbImage(format!("/backdrop-{tmdb}.jpg"))),
        availability,
    }
}

/// Every title the fake knows.
fn catalogue() -> Vec<Discovered> {
    use Availability::*;
    use MediaKind::*;
    vec![
        title(Movie, IN_LIBRARY_FILM, "The Last Winter", 2025, Available),
        title(Movie, OTHER_LIBRARY_FILM, "The Tides", 2024, Available),
        title(Movie, WINTER_LIGHT, "Winter Light", 2019, Missing),
        title(Movie, WINTERS_END, "Winter's End", 2026, Missing),
        title(Movie, PAPER_MOONS, "Paper Moons", 2023, Pending),
        title(Movie, 9006, "Glass Rivers", 2025, Processing),
        title(Movie, NORTHERN_LIGHTS, "Northern Lights", 2022, Missing),
        title(Movie, 9008, "Salt and Iron", 2021, Missing),
        title(Movie, 9009, "The Quiet Field", 2026, Missing),
        title(Movie, 9010, "Lantern Bay", 2018, Missing),
        title(Tv, IN_LIBRARY_SERIES, "Signal Lost", 2023, Available),
        title(Tv, COPPER_COAST, "Copper Coast", 2024, Missing),
        title(Tv, HARBOUR_WATCH, "Harbour Watch", 2020, PartiallyAvailable),
        title(Tv, 7004, "The Long Field", 2022, Missing),
    ]
}

impl FakeSeerr {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Fails as the state says, after its delay.
    fn reach(&self) -> Result<(), RequestError> {
        let (delay, failure) = {
            let state = self.state();
            (state.delay, state.failure.clone())
        };
        std::thread::sleep(delay);
        failure.map_or(Ok(()), Err)
    }

    /// The titles as the server shows them now: what was asked for is pending.
    fn titles(&self) -> Vec<Discovered> {
        let requested = self.state().requested.clone();
        catalogue()
            .into_iter()
            .map(|mut title| {
                if requested.contains(&title.tmdb) && title.availability == Availability::Missing {
                    title.availability = Availability::Pending;
                }
                title
            })
            .collect()
    }

    pub fn requests(&self) -> Vec<Ask> {
        self.state().record.requests.clone()
    }

    fn find(&self, kind: MediaKind, id: TmdbId) -> Result<Discovered, RequestError> {
        self.titles()
            .into_iter()
            .find(|title| title.kind == kind && title.tmdb == id)
            .ok_or(RequestError::NotFound)
    }

    fn of(&self, ids: &[TmdbId]) -> Vec<Discovered> {
        let titles = self.titles();
        ids.iter()
            .filter_map(|id| titles.iter().find(|t| t.tmdb == *id).cloned())
            .collect()
    }
}

impl RequestServer for FakeSeerr {
    fn check(&self) -> Result<ServerCheck, RequestError> {
        self.reach()?;
        let mut state = self.state();
        state.record.checks += 1;
        Ok(ServerCheck {
            version: VERSION.into(),
            acting_as: if state.no_viewer {
                ActingAs::Owner("Owner".into())
            } else {
                ActingAs::Viewer("camille".into())
            },
        })
    }

    fn discover(&self, row: DiscoverRow) -> Result<Vec<Discovered>, RequestError> {
        self.reach()?;
        Ok(match row {
            DiscoverRow::Trending => self.of(&[
                NORTHERN_LIGHTS,
                7004,
                IN_LIBRARY_FILM,
                9006,
                COPPER_COAST,
                9008,
            ]),
            DiscoverRow::PopularMovies => self.of(&[
                9010,
                OTHER_LIBRARY_FILM,
                PAPER_MOONS,
                9008,
                NORTHERN_LIGHTS,
                WINTER_LIGHT,
            ]),
            DiscoverRow::PopularSeries => {
                self.of(&[COPPER_COAST, HARBOUR_WATCH, IN_LIBRARY_SERIES, 7004])
            }
            DiscoverRow::Upcoming => self.of(&[9009, WINTERS_END, 9006]),
        })
    }

    fn search(&self, query: &str) -> Result<Vec<Discovered>, RequestError> {
        self.reach()?;
        self.state().record.searches.push(query.to_string());
        let query = query.to_lowercase();
        Ok(self
            .titles()
            .into_iter()
            .filter(|title| title.title.to_lowercase().contains(&query))
            .collect())
    }

    fn title(&self, kind: MediaKind, id: TmdbId) -> Result<TitleDetail, RequestError> {
        self.reach()?;
        let title = self.find(kind, id)?;
        let in_collection = [
            IN_LIBRARY_FILM,
            OTHER_LIBRARY_FILM,
            WINTER_LIGHT,
            WINTERS_END,
        ];
        let requested = self.state().requested.contains(&id);
        let seasons = match kind {
            MediaKind::Movie => Vec::new(),
            MediaKind::Tv => (1..=3)
                .map(|number| SeasonOffer {
                    number,
                    name: format!("Season {number}"),
                    episodes: 6 + number * 2,
                    taken: requested || (id == HARBOUR_WATCH && number <= 2),
                })
                .collect(),
        };
        Ok(TitleDetail {
            genres: vec!["Drama".into(), "Mystery".into()],
            runtime: (kind == MediaKind::Movie).then(|| Duration::from_secs(108 * 60)),
            cast: [
                "Ines Varga",
                "Tomas Reed",
                "Ada Brook",
                "Noor Hale",
                "Leif Olsen",
            ]
            .iter()
            .enumerate()
            .map(|(n, name)| CastPerson {
                name: (*name).into(),
                character: Some(["The keeper", "Captain", "Ada", "The doctor", "Leif"][n].into()),
                photo: (n != 3).then(|| TmdbImage(format!("/person-{n}.jpg"))),
            })
            .collect(),
            collection: in_collection.contains(&id).then(|| CollectionRef {
                id: COLLECTION,
                name: "Winter Saga".into(),
            }),
            seasons,
            title,
        })
    }

    fn recommendations(&self, kind: MediaKind, _: TmdbId) -> Result<Vec<Discovered>, RequestError> {
        self.reach()?;
        Ok(match kind {
            MediaKind::Movie => self.of(&[
                OTHER_LIBRARY_FILM,
                PAPER_MOONS,
                9006,
                NORTHERN_LIGHTS,
                9008,
                9010,
            ]),
            MediaKind::Tv => self.of(&[IN_LIBRARY_SERIES, COPPER_COAST, 7004]),
        })
    }

    fn collection(&self, id: TmdbId) -> Result<CollectionParts, RequestError> {
        self.reach()?;
        if id != COLLECTION {
            return Err(RequestError::NotFound);
        }
        Ok(CollectionParts {
            id,
            name: "Winter Saga".into(),
            overview: Some("Four invented films about one long winter, oldest first.".into()),
            backdrop: Some(TmdbImage("/winter-saga.jpg".into())),
            films: self.of(&[
                WINTER_LIGHT,
                OTHER_LIBRARY_FILM,
                IN_LIBRARY_FILM,
                WINTERS_END,
            ]),
        })
    }

    fn my_requests(&self) -> Result<Vec<MyRequest>, RequestError> {
        self.reach()?;
        self.state().record.my_requests += 1;
        let asked: Vec<TmdbId> = self.state().requested.iter().rev().copied().collect();
        let mut mine: Vec<MyRequest> = self
            .of(&asked)
            .into_iter()
            .map(|title| MyRequest {
                title,
                status: RequestStatus::Pending,
            })
            .collect();
        mine.extend(self.of(&[PAPER_MOONS]).into_iter().map(|title| MyRequest {
            title,
            status: RequestStatus::Pending,
        }));
        mine.extend(
            self.of(&[HARBOUR_WATCH, 9006])
                .into_iter()
                .map(|title| MyRequest {
                    title,
                    status: RequestStatus::Approved,
                }),
        );
        Ok(mine)
    }

    fn quota_reached(&self, kind: MediaKind) -> Result<bool, RequestError> {
        self.reach()?;
        Ok(kind == MediaKind::Movie && self.state().movie_quota == Some(0))
    }

    fn request(&self, ask: &Ask) -> Result<(), RequestError> {
        self.reach()?;
        let mut state = self.state();
        let id = match ask {
            Ask::Movie(id) => {
                match &mut state.movie_quota {
                    Some(0) => return Err(RequestError::Quota),
                    Some(left) => *left -= 1,
                    None => {}
                }
                *id
            }
            Ask::Tv(id, _) => *id,
        };
        state.record.requests.push(ask.clone());
        state.requested.push(id);
        Ok(())
    }

    fn image(&self, image: &TmdbImage, width: u32) -> Result<Vec<u8>, RequestError> {
        self.reach()?;
        // A poster is 2:3, a backdrop 16:9, a photo square, all made small: the harness draws
        // them, nothing reads their pixels.
        let (w, h) = if image.0.starts_with("/poster") {
            (width.min(200), width.min(200) * 3 / 2)
        } else if image.0.starts_with("/person") {
            (120, 120)
        } else {
            (width.min(640), width.min(640) * 9 / 16)
        };
        Ok(gradient(&image.0, w, h))
    }
}
