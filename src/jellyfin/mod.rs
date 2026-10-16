//! The Jellyfin implementation of the provider port, for Jellyfin 10.10 and later.

mod account;
mod dto;
mod http;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use crate::provider::{
    Collection, Connector, Episode, ExternalSubtitle, Film, FilmDetail, FilmQuery, FilmSort,
    FilterChoices, ImageKind, ImageRef, ItemId, Library, MediaServer, MediaSourceId, Page,
    PersonTitles, PlaySessionId, Playback, PlaybackReport, ProviderError, SearchResults, Season,
    Series, SeriesDetail, ServerInfo, StreamUrl, Token, TokenStore,
};
use account::{AccountFile, Login};
use dto::{
    AuthenticationResult, Item, PlaybackInfoResponse, PublicSystemInfo, QueryFilters, QueryResult,
};
use http::{Device, Http, encode, normalize_address};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;

/// How many similar titles a film's or a series' page shows.
const SIMILAR: u32 = 12;

/// What an episode is asked with: its synopsis, and its versions for the chips.
const EPISODE_FIELDS: &str = "Overview,MediaSources";

/// Signs in to Jellyfin servers. The account file keeps the device id and the last login; the
/// store keeps the token.
pub struct JellyfinConnector {
    path: PathBuf,
    store: Arc<dyn TokenStore>,
    device: Device,
}

/// The name the server shows for this device: the machine's.
fn device_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Linux".to_string())
}

impl JellyfinConnector {
    /// The connector of the account file at `path` (made, with a new device id, when missing).
    pub fn open(path: PathBuf, store: Arc<dyn TokenStore>) -> Result<Self, ProviderError> {
        let account = AccountFile::open(&path)?;
        let device = Device {
            id: account.device_id,
            name: device_name(),
        };
        Ok(Self {
            path,
            store,
            device,
        })
    }

    fn session(&self, login: Login, token: Token) -> Arc<dyn MediaServer> {
        let http = Http::new(login.address.clone(), self.device.clone(), Some(token));
        Arc::new(Jellyfin {
            http,
            server: ServerInfo {
                id: login.server_id.clone(),
                name: login.server_name.clone(),
                version: login.server_version.clone(),
            },
            login,
            path: self.path.clone(),
            store: Arc::clone(&self.store),
        })
    }
}

impl Connector for JellyfinConnector {
    fn probe(&self, address: &str) -> Result<ServerInfo, ProviderError> {
        let http = Http::new(normalize_address(address)?, self.device.clone(), None);
        let info: PublicSystemInfo = http
            .get("server information", "/System/Info/Public", &[])
            .map_err(|e| match e {
                ProviderError::Unreachable(_) => e,
                _ => ProviderError::NotAMediaServer,
            })?;
        if info
            .product_name
            .as_deref()
            .is_some_and(|p| p != "Jellyfin Server")
        {
            return Err(ProviderError::NotAMediaServer);
        }
        Ok(info.into_info())
    }

    fn login(
        &self,
        address: &str,
        user: &str,
        password: &str,
    ) -> Result<Arc<dyn MediaServer>, ProviderError> {
        let server = self.probe(address)?;
        let address = normalize_address(address)?;
        let http = Http::new(address.clone(), self.device.clone(), None);
        let body = json!({ "Username": user, "Pw": password });
        let answer: AuthenticationResult = http
            .post("login", "/Users/AuthenticateByName", &[], &body)
            .map_err(|e| match e {
                ProviderError::SessionExpired => ProviderError::WrongCredentials,
                ProviderError::Forbidden => ProviderError::AccountDisabled,
                _ => e,
            })?;
        let token = Token::new(answer.access_token);
        let login = Login {
            address,
            server_id: server.id,
            server_name: server.name,
            server_version: server.version,
            user_id: answer.user.id,
            user_name: answer.user.name,
        };

        let mut account = AccountFile::open(&self.path)?;
        if let Some(previous) = &account.login
            && previous.keyring_account() != login.keyring_account()
        {
            self.store.delete(&previous.keyring_account())?;
        }
        self.store.save(&login.keyring_account(), &token)?;
        account.login = Some(login.clone());
        account.save(&self.path)?;
        Ok(self.session(login, token))
    }

    fn restore(&self) -> Result<Option<Arc<dyn MediaServer>>, ProviderError> {
        let Some(login) = AccountFile::open(&self.path)?.login else {
            return Ok(None);
        };
        let token = self.store.load(&login.keyring_account())?;
        Ok(token.map(|token| self.session(login, token)))
    }
}

/// A signed-in session on a Jellyfin server.
pub struct Jellyfin {
    http: Http,
    login: Login,
    server: ServerInfo,
    path: PathBuf,
    store: Arc<dyn TokenStore>,
}

/// The format a subtitle file is asked in, from its codec as Jellyfin names it.
fn subtitle_format(codec: Option<&str>) -> String {
    match codec.map(str::to_ascii_lowercase).as_deref() {
        Some("subrip") | None => "srt".to_string(),
        Some("webvtt") => "vtt".to_string(),
        Some(other) => other.to_string(),
    }
}

/// Titles of a person asked at once, and the most of them ever asked.
const PERSON_TITLES_PAGE: usize = 100;
const PERSON_TITLES_CAP: usize = 1000;

impl Jellyfin {
    fn user(&self) -> (&'static str, String) {
        ("userId", self.login.user_id.clone())
    }

    fn token(&self) -> &str {
        self.http.token().map_or("", Token::expose_secret)
    }

    fn items(&self, what: &str, query: &[(&str, String)]) -> Result<QueryResult, ProviderError> {
        let mut query = query.to_vec();
        query.push(self.user());
        self.http.get(what, "/Items", &query)
    }

    /// One page of the items of a kind (`Movie`, `Series`) in a library, in the order asked.
    fn page(
        &self,
        what: &str,
        kind: &str,
        library: &ItemId,
        query: &FilmQuery,
    ) -> Result<QueryResult, ProviderError> {
        let sort = match query.sort {
            FilmSort::Title => "SortName",
            FilmSort::DateAdded => "DateCreated,SortName",
            FilmSort::ReleaseYear => "ProductionYear,SortName",
            FilmSort::Rating => "CommunityRating,SortName",
        };
        let order = if query.descending {
            "Descending"
        } else {
            "Ascending"
        };
        let mut asked = vec![
            ("parentId", library.0.clone()),
            ("includeItemTypes", kind.into()),
            ("recursive", "true".into()),
            ("sortBy", sort.into()),
            ("sortOrder", order.into()),
            ("startIndex", query.start.to_string()),
            // Never 0: Jellyfin reads it as no limit at all.
            ("limit", query.limit.max(1).to_string()),
            ("imageTypeLimit", "1".into()),
            ("enableImageTypes", "Primary,Backdrop".into()),
            ("enableTotalRecordCount", "true".into()),
        ];
        let filters = &query.filters;
        if let Some(genre) = &filters.genre {
            asked.push(("genres", genre.clone()));
        }
        if let Some(decade) = filters.decade {
            let years: Vec<String> = (decade..decade + 10).map(|y| y.to_string()).collect();
            asked.push(("years", years.join(",")));
        }
        if filters.unwatched {
            asked.push(("isPlayed", "false".into()));
        }
        if let Some(letter) = query.before {
            // SortName is stored in lowercase and compared as it is: an uppercase letter would
            // sort before every title.
            asked.push(("nameLessThan", letter.to_lowercase().to_string()));
        }
        self.items(what, &asked)
    }

    fn report(
        &self,
        what: &str,
        path: &str,
        report: &PlaybackReport,
        playing: bool,
    ) -> Result<(), ProviderError> {
        let mut body = json!({
            "ItemId": report.film.0,
            "MediaSourceId": report.source.0,
            "PlaySessionId": report.session.0,
            "PositionTicks": dto::ticks(report.position),
        });
        if playing {
            body["IsPaused"] = json!(report.paused);
            body["CanSeek"] = json!(true);
            body["PlayMethod"] = json!("DirectPlay");
            body["AudioStreamIndex"] = json!(report.audio_track);
            body["SubtitleStreamIndex"] = json!(report.subtitle_track);
        }
        self.http.send(what, path, &[], &body)
    }
}

impl MediaServer for Jellyfin {
    fn server(&self) -> &ServerInfo {
        &self.server
    }

    fn user_name(&self) -> &str {
        &self.login.user_name
    }

    fn libraries(&self) -> Result<Vec<Library>, ProviderError> {
        let views: QueryResult = self.http.get("libraries", "/UserViews", &[self.user()])?;
        Ok(views.items.into_iter().map(Item::into_library).collect())
    }

    fn films(&self, library: &ItemId, query: &FilmQuery) -> Result<Page<Film>, ProviderError> {
        let found = self.page("films", "Movie", library, query)?;
        let films: Vec<Film> = found.items.iter().map(Item::film).collect();
        let total = found.total_record_count.unwrap_or(films.len() as u32);
        Ok(Page {
            items: films,
            total,
        })
    }

    fn filter_choices(&self, library: &ItemId) -> Result<FilterChoices, ProviderError> {
        let genres: QueryResult = self.http.get(
            "genres",
            "/Genres",
            &[self.user(), ("parentId", library.0.clone())],
        )?;
        let filters: QueryFilters = self.http.get(
            "years",
            "/Items/Filters",
            &[self.user(), ("parentId", library.0.clone())],
        )?;
        let mut genres: Vec<String> = genres.items.into_iter().map(|g| g.name).collect();
        genres.sort_by_key(|genre| genre.to_lowercase());
        let mut decades: Vec<i32> = filters.years.iter().map(|y| y - y.rem_euclid(10)).collect();
        decades.sort_unstable_by(|a, b| b.cmp(a));
        decades.dedup();
        Ok(FilterChoices { genres, decades })
    }

    fn continue_watching(&self) -> Result<Vec<Film>, ProviderError> {
        let found: QueryResult = self.http.get(
            "continue watching",
            "/UserItems/Resume",
            &[
                self.user(),
                ("includeItemTypes", "Movie".into()),
                ("mediaTypes", "Video".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary,Backdrop".into()),
            ],
        )?;
        Ok(found.items.iter().map(Item::film).collect())
    }

    fn film(&self, film: &ItemId) -> Result<FilmDetail, ProviderError> {
        let item: Item = self.http.get(
            "film",
            &format!("/Items/{}", encode(&film.0)),
            &[self.user()],
        )?;
        let similar: QueryResult = self.http.get(
            "similar films",
            &format!("/Items/{}/Similar", encode(&film.0)),
            &[self.user(), ("limit", SIMILAR.to_string())],
        )?;
        Ok(FilmDetail {
            film: item.film(),
            directors: item.directors(),
            cast: item.cast(),
            similar: similar.items.iter().map(Item::film).collect(),
            overview: item.overview,
            genres: item.genres,
            official_rating: item.official_rating,
            versions: item
                .media_sources
                .into_iter()
                .map(|s| s.into_source())
                .collect(),
        })
    }

    /// Jellyfin gives a film no link to its collections, so each collection of the user is looked
    /// through, for ids only, until one holds the film. Only that one is asked again, in full,
    /// for the posters of its other films.
    fn collection_of(&self, film: &ItemId) -> Result<Option<Collection>, ProviderError> {
        let ids_only = |parent: &str| {
            self.items(
                "collection",
                &[
                    ("parentId", parent.to_string()),
                    ("enableImages", "false".into()),
                    ("enableUserData", "false".into()),
                ],
            )
        };
        let boxsets = self.items(
            "collections",
            &[
                ("includeItemTypes", "BoxSet".into()),
                ("recursive", "true".into()),
                ("enableImages", "false".into()),
                ("enableUserData", "false".into()),
            ],
        )?;
        for boxset in boxsets.items {
            if !ids_only(&boxset.id)?.items.iter().any(|f| f.id == film.0) {
                continue;
            }
            let films = self.items(
                "collection",
                &[
                    ("parentId", boxset.id.clone()),
                    ("imageTypeLimit", "1".into()),
                    ("enableImageTypes", "Primary,Backdrop".into()),
                ],
            )?;
            return Ok(Some(Collection {
                id: ItemId(boxset.id),
                name: boxset.name,
                others: films
                    .items
                    .iter()
                    .filter(|f| f.id != film.0)
                    .map(Item::film)
                    .collect(),
            }));
        }
        Ok(None)
    }

    fn play(&self, film: &ItemId, source: &MediaSourceId) -> Result<Playback, ProviderError> {
        let body = json!({
            "UserId": self.login.user_id,
            "MediaSourceId": source.0,
            "EnableDirectPlay": true,
            "EnableDirectStream": false,
            "EnableTranscoding": false,
            "AutoOpenLiveStream": false,
        });
        let info: PlaybackInfoResponse = self.http.post(
            "playback",
            &format!("/Items/{}/PlaybackInfo", encode(&film.0)),
            &[self.user()],
            &body,
        )?;
        let session = info
            .play_session_id
            .ok_or_else(|| ProviderError::BadAnswer("playback: no play session".into()))?;
        let found = info
            .media_sources
            .into_iter()
            .find(|s| s.id == source.0)
            .ok_or_else(|| ProviderError::BadAnswer("playback: no such version".into()))?;

        let base = self.http.base();
        let (film_id, source_id) = (encode(&film.0), encode(&source.0));
        // `ApiKey`, not the older `api_key`: Jellyfin 10.11 keeps that one behind a legacy setting.
        let api_key = encode(self.token());
        let stream = StreamUrl::new(format!(
            "{base}/Videos/{film_id}/stream?static=true&mediaSourceId={source_id}\
             &playSessionId={}&deviceId={}&ApiKey={api_key}",
            encode(&session),
            encode(&self.http.device().id),
        ));
        let external_subtitles = found
            .media_streams
            .iter()
            .filter(|s| s.kind == "Subtitle" && s.is_external)
            .map(|s| ExternalSubtitle {
                index: s.index,
                url: StreamUrl::new(format!(
                    "{base}/Videos/{film_id}/{source_id}/Subtitles/{}/Stream.{}?ApiKey={api_key}",
                    s.index,
                    subtitle_format(s.codec.as_deref()),
                )),
            })
            .collect();
        Ok(Playback {
            session: PlaySessionId(session),
            film: film.clone(),
            source: source.clone(),
            stream,
            external_subtitles,
        })
    }

    fn report_started(&self, report: &PlaybackReport) -> Result<(), ProviderError> {
        self.report("playback started", "/Sessions/Playing", report, true)
    }

    fn report_progress(&self, report: &PlaybackReport) -> Result<(), ProviderError> {
        self.report(
            "playback progress",
            "/Sessions/Playing/Progress",
            report,
            true,
        )
    }

    fn report_stopped(&self, report: &PlaybackReport) -> Result<(), ProviderError> {
        self.report(
            "playback stopped",
            "/Sessions/Playing/Stopped",
            report,
            false,
        )
    }

    fn set_watched(&self, film: &ItemId, watched: bool) -> Result<(), ProviderError> {
        let path = format!("/UserPlayedItems/{}", encode(&film.0));
        if watched {
            self.http.post_empty("mark watched", &path, &[self.user()])
        } else {
            self.http.delete("mark unwatched", &path, &[self.user()])
        }
    }

    fn image(&self, image: &ImageRef, width: u32) -> Result<Vec<u8>, ProviderError> {
        let kind = match image.kind {
            ImageKind::Primary => "Primary",
            ImageKind::Backdrop => "Backdrop",
        };
        self.http.get_bytes(
            "image",
            &format!("/Items/{}/Images/{kind}", encode(&image.item.0)),
            &[
                ("tag", image.tag.clone()),
                ("maxWidth", width.to_string()),
                ("quality", "90".into()),
            ],
        )
    }

    /// One request per group, so that a group with many matches never crowds out the others
    /// (search hints share a single limit between the three). The three go out at the same
    /// time: a search costs one round trip, not three.
    fn search(&self, query: &str, per_group: u32) -> Result<SearchResults, ProviderError> {
        let titles = |kind: &str| {
            self.items(
                "search",
                &[
                    ("searchTerm", query.to_string()),
                    ("includeItemTypes", kind.into()),
                    ("recursive", "true".into()),
                    ("limit", per_group.to_string()),
                    ("imageTypeLimit", "1".into()),
                    ("enableImageTypes", "Primary,Backdrop".into()),
                ],
            )
        };
        let (films, series, people) = std::thread::scope(|scope| {
            let films = scope.spawn(|| titles("Movie"));
            let series = scope.spawn(|| titles("Series"));
            let people = self.http.get::<QueryResult>(
                "search people",
                "/Persons",
                &[
                    ("searchTerm", query.to_string()),
                    ("limit", per_group.to_string()),
                    ("imageTypeLimit", "1".into()),
                    ("enableImageTypes", "Primary".into()),
                    self.user(),
                ],
            );
            (
                films.join().expect("the films request panicked"),
                series.join().expect("the series request panicked"),
                people,
            )
        });
        let (films, series, people) = (films?, series?, people?);
        Ok(SearchResults {
            films: films.items.iter().map(Item::film).collect(),
            series: series.items.iter().map(Item::series_hit).collect(),
            people: people.items.iter().map(Item::person_hit).collect(),
        })
    }

    /// Asked page by page, in a stable order so that no title is skipped or read twice between
    /// two pages, up to `PERSON_TITLES_CAP` items: a person in thousands of episodes still gets
    /// a page, with their first thousand titles.
    fn person_titles(&self, person: &ItemId) -> Result<PersonTitles, ProviderError> {
        let mut found: Vec<Item> = Vec::new();
        while found.len() < PERSON_TITLES_CAP {
            let page = self.items(
                "person titles",
                &[
                    ("personIds", person.0.clone()),
                    ("includeItemTypes", "Movie,Series,Episode".into()),
                    ("recursive", "true".into()),
                    ("sortBy", "SortName".into()),
                    ("sortOrder", "Ascending".into()),
                    ("startIndex", found.len().to_string()),
                    ("limit", PERSON_TITLES_PAGE.to_string()),
                    ("imageTypeLimit", "1".into()),
                    ("enableImageTypes", "Primary,Backdrop".into()),
                ],
            )?;
            let asked = found.len();
            let total = page.total_record_count.map_or(usize::MAX, |t| t as usize);
            let short = page.items.len() < PERSON_TITLES_PAGE;
            found.extend(page.items);
            if short || found.len() >= total || found.len() == asked {
                break;
            }
        }
        Ok(dto::person_titles(&found))
    }

    fn logout(&self) -> Result<(), ProviderError> {
        // The server may be gone or have dropped the token already: the session ends here anyway.
        let _ = self.http.post_empty("logout", "/Sessions/Logout", &[]);
        self.store.delete(&self.login.keyring_account())?;
        let mut account = AccountFile::open(&self.path)?;
        account.login = None;
        account.save(&self.path)
    }

    fn series(&self, library: &ItemId, query: &FilmQuery) -> Result<Page<Series>, ProviderError> {
        let found = self.page("series", "Series", library, query)?;
        let series: Vec<Series> = found.items.iter().map(Item::series).collect();
        let total = found.total_record_count.unwrap_or(series.len() as u32);
        Ok(Page {
            items: series,
            total,
        })
    }

    fn series_detail(&self, series: &ItemId) -> Result<SeriesDetail, ProviderError> {
        let item: Item = self.http.get(
            "series",
            &format!("/Items/{}", encode(&series.0)),
            &[self.user()],
        )?;
        let similar: QueryResult = self.http.get(
            "similar series",
            &format!("/Items/{}/Similar", encode(&series.0)),
            &[self.user(), ("limit", SIMILAR.to_string())],
        )?;
        Ok(SeriesDetail {
            series: item.series(),
            cast: item.cast(),
            similar: similar.items.iter().map(Item::series).collect(),
            overview: item.overview,
            genres: item.genres,
            official_rating: item.official_rating,
        })
    }

    fn seasons(&self, series: &ItemId) -> Result<Vec<Season>, ProviderError> {
        let found: QueryResult = self.http.get(
            "seasons",
            &format!("/Shows/{}/Seasons", encode(&series.0)),
            &[self.user(), ("isMissing", "false".into())],
        )?;
        Ok(found.items.iter().map(Item::season).collect())
    }

    fn episodes(&self, series: &ItemId, season: &ItemId) -> Result<Vec<Episode>, ProviderError> {
        let found: QueryResult = self.http.get(
            "episodes",
            &format!("/Shows/{}/Episodes", encode(&series.0)),
            &[
                self.user(),
                ("seasonId", season.0.clone()),
                ("isMissing", "false".into()),
                ("fields", EPISODE_FIELDS.into()),
            ],
        )?;
        Ok(found.items.into_iter().map(Item::episode).collect())
    }

    fn next_up(&self, series: &ItemId) -> Result<Option<Episode>, ProviderError> {
        let found: QueryResult = self.http.get(
            "next up",
            "/Shows/NextUp",
            &[
                self.user(),
                ("seriesId", series.0.clone()),
                ("limit", "1".into()),
                ("enableResumable", "true".into()),
                ("fields", EPISODE_FIELDS.into()),
            ],
        )?;
        Ok(found.items.into_iter().next().map(Item::episode))
    }
}
