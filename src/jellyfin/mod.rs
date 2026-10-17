//! The Jellyfin implementation of the provider port, for Jellyfin 10.10 and later.

mod account;
mod dto;
pub(crate) mod http;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use crate::provider::{
    Collection, CollectionCard, CollectionDetail, Connector, EntryId, Episode, ExternalSubtitle,
    Favourite, Feature, Film, FilmDetail, FilmQuery, FilmSort, FilterChoices, HistorySource,
    ImageKind, ImageRef, ItemId, Library, LocalTime, MediaServer, MediaSourceId, Page,
    PersonTitles, Play, PlayMethod, PlaySessionId, Playback, PlaybackReport, Playlist,
    PlaylistEntry, ProviderError, SearchResults, Season, Segment, Series, SeriesDetail, ServerInfo,
    StreamUrl, Title, TmdbIndex, Token, TokenStore, TranscodeRequest, Trickplay, WatchHistory,
    Watched, WatchedKind,
};
use account::{AccountFile, Login};
use dto::{
    AuthenticationResult, Item, MediaSourceInfo, PlaybackInfoResponse, PublicSystemInfo,
    QueryFilters, QueryResult,
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
/// Whether `path` is a whole address (a remote subtitle's) and not a path of the server.
fn is_absolute(path: &str) -> bool {
    let starts_with = |scheme: &str| {
        path.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    };
    starts_with("http://") || starts_with("https://")
}

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

    fn last_address(&self) -> Option<String> {
        let account = AccountFile::open(&self.path).ok()?;
        account
            .login
            .map(|login| login.address)
            .or(account.last_address)
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

/// High enough that the server never converts a file for its bitrate alone: the user's own
/// limit on the server still applies.
const MAX_STREAMING_BITRATE: u64 = 1_000_000_000;

/// What the player plays, sent only when it asks for a conversion. mpv plays nearly every
/// container and codec: a direct play profile that names none of them covers them all. The
/// conversion is the most common one: H.264 and AAC in MPEG-TS segments, over HLS, under
/// `max_bitrate` bits per second.
fn device_profile(max_bitrate: u64) -> serde_json::Value {
    json!({
        "Name": "Bobine",
        "MaxStreamingBitrate": max_bitrate,
        "DirectPlayProfiles": [{ "Type": "Video" }, { "Type": "Audio" }],
        "TranscodingProfiles": [{
            "Type": "Video",
            "Container": "ts",
            "Protocol": "hls",
            "VideoCodec": "h264",
            "AudioCodec": "aac",
            "Context": "Streaming",
            "MaxAudioChannels": "6",
        }],
        "SubtitleProfiles": subtitle_profiles(),
    })
}

/// A text subtitle comes as a file beside the converted picture when the server can extract it;
/// the server draws into the picture only what the HLS stream cannot carry.
fn subtitle_profiles() -> Vec<serde_json::Value> {
    let external = ["srt", "ass", "ssa", "vtt"]
        .map(|format| json!({ "Format": format, "Method": "External" }));
    let embedded = [
        "subrip", "srt", "ass", "ssa", "webvtt", "vtt", "mov_text", "pgssub", "dvdsub", "dvbsub",
    ]
    .map(|format| json!({ "Format": format, "Method": "Embed" }));
    external.into_iter().chain(embedded).collect()
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

    /// What a playlist holds, in its order, its first `limit` entries only if given, and how
    /// many it holds; only what plays here: films and episodes (a series put in a playlist is
    /// put in as its episodes).
    fn entries(
        &self,
        playlist: &ItemId,
        fields: &str,
        limit: Option<usize>,
    ) -> Result<(Vec<PlaylistEntry>, usize), ProviderError> {
        let mut query = vec![
            self.user(),
            ("fields", fields.into()),
            ("imageTypeLimit", "1".into()),
            ("enableImageTypes", "Primary,Backdrop".into()),
        ];
        query.extend(limit.map(|limit| ("limit", limit.to_string())));
        let found: QueryResult = self.http.get(
            "playlist",
            &format!("/Playlists/{}/Items", encode(&playlist.0)),
            &query,
        )?;
        let count = found.total_record_count.unwrap_or(found.items.len() as u32) as usize;
        let entries = found
            .items
            .into_iter()
            .filter(|item| matches!(item.kind.as_deref(), Some("Movie" | "Episode")))
            .filter_map(|item| {
                let id = EntryId(item.playlist_item_id.clone()?);
                let poster = item.list_poster();
                Some(PlaylistEntry {
                    id,
                    title: item.title()?,
                    poster,
                })
            })
            .collect();
        Ok((entries, count))
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
            FilmSort::LastAdded => "DateLastContentAdded,SortName",
            FilmSort::ReleaseYear => "ProductionYear,SortName",
            FilmSort::Rating => "CommunityRating,SortName",
        };
        // One order per sorted field: the first goes the way asked, the title that breaks its
        // ties always reads A to Z.
        let order = sort
            .split(',')
            .enumerate()
            .map(|(field, _)| {
                if field == 0 && query.descending {
                    "Descending"
                } else {
                    "Ascending"
                }
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut asked = vec![
            ("parentId", library.0.clone()),
            ("includeItemTypes", kind.into()),
            ("recursive", "true".into()),
            ("sortBy", sort.into()),
            ("sortOrder", order),
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

    /// Asks the server how a version plays, `body` saying how it may: its play session and
    /// what it says of the version.
    fn playback_info(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
        body: &serde_json::Value,
    ) -> Result<(String, MediaSourceInfo), ProviderError> {
        let info: PlaybackInfoResponse = self.http.post(
            "playback",
            &format!("/Items/{}/PlaybackInfo", encode(&film.0)),
            &[self.user()],
            body,
        )?;
        let session = info
            .play_session_id
            .ok_or_else(|| ProviderError::BadAnswer("playback: no play session".into()))?;
        let found = info
            .media_sources
            .into_iter()
            .find(|s| s.id == source.0)
            .ok_or_else(|| ProviderError::BadAnswer("playback: no such version".into()))?;
        Ok((session, found))
    }

    /// The address of `path` on the server, with the token: the server puts it in the paths it
    /// gives; an older one may not. An absolute address is another host's (a remote subtitle):
    /// as it is, never with our token.
    fn served(&self, path: &str) -> String {
        if is_absolute(path) {
            return path.to_string();
        }
        let mut url = format!("{}{path}", self.http.base());
        if !path.contains("ApiKey=") && !path.contains("api_key=") {
            let separator = if path.contains('?') { '&' } else { '?' };
            url += &format!("{separator}ApiKey={}", encode(self.token()));
        }
        url
    }

    /// The playback of the version `found`, streamed from `stream`, with its subtitle files.
    fn opened(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
        session: String,
        found: &MediaSourceInfo,
        stream: String,
    ) -> Playback {
        let base = self.http.base();
        let (film_id, source_id) = (encode(&film.0), encode(&source.0));
        let api_key = encode(self.token());
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
        Playback {
            session: PlaySessionId(session),
            film: film.clone(),
            source: source.clone(),
            stream: StreamUrl::new(stream),
            external_subtitles,
        }
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
            body["PlayMethod"] = json!(match report.method {
                PlayMethod::DirectPlay => "DirectPlay",
                PlayMethod::Transcode => "Transcode",
            });
            body["IsPaused"] = json!(report.paused);
            body["CanSeek"] = json!(true);
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

    fn user_id(&self) -> &str {
        &self.login.user_id
    }

    fn libraries(&self) -> Result<Vec<Library>, ProviderError> {
        let views: QueryResult = self.http.get("libraries", "/UserViews", &[self.user()])?;
        Ok(views.items.into_iter().map(Item::into_library).collect())
    }

    fn films(&self, library: &ItemId, query: &FilmQuery) -> Result<Page<Film>, ProviderError> {
        let found = self.page("movies", "Movie", library, query)?;
        let films: Vec<Film> = found.items.iter().map(Item::film).collect();
        let total = found.total_record_count.unwrap_or(films.len() as u32);
        Ok(Page {
            items: films,
            total,
        })
    }

    fn filter_choices(&self, library: &ItemId) -> Result<FilterChoices, ProviderError> {
        let genres: Result<QueryResult, _> = self.http.get(
            "genres",
            "/Genres",
            &[self.user(), ("parentId", library.0.clone())],
        );
        let filters: Result<QueryFilters, _> = self.http.get(
            "years",
            "/Items/Filters",
            &[self.user(), ("parentId", library.0.clone())],
        );
        // One lookup missing leaves its chip with nothing to offer, not the other one: only an
        // expired session, which no lookup can do without, or both missing, is a failure.
        let (genres, filters) = match (genres, filters) {
            (Err(error @ ProviderError::SessionExpired), _)
            | (_, Err(error @ ProviderError::SessionExpired))
            | (Err(error), Err(_)) => return Err(error),
            (genres, filters) => (genres.ok(), filters.ok()),
        };
        let mut genres: Vec<String> = genres
            .map(|genres| genres.items.into_iter().map(|g| g.name).collect())
            .unwrap_or_default();
        genres.sort_by_key(|genre| genre.to_lowercase());
        let mut decades: Vec<i32> = filters
            .map(|filters| filters.years.iter().map(|y| y - y.rem_euclid(10)).collect())
            .unwrap_or_default();
        decades.sort_unstable_by(|a, b| b.cmp(a));
        decades.dedup();
        Ok(FilterChoices { genres, decades })
    }

    fn continue_watching(&self) -> Result<Vec<Title>, ProviderError> {
        let found: QueryResult = self.http.get(
            "continue watching",
            "/UserItems/Resume",
            &[
                self.user(),
                ("includeItemTypes", "Movie,Episode".into()),
                ("mediaTypes", "Video".into()),
                // An episode started from the Home plays without its page: it needs its
                // versions now.
                ("fields", "MediaSources".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary,Backdrop".into()),
            ],
        )?;
        Ok(found.items.into_iter().filter_map(Item::title).collect())
    }

    fn film(&self, film: &ItemId) -> Result<FilmDetail, ProviderError> {
        let item: Item = self.http.get(
            "movie",
            &format!("/Items/{}", encode(&film.0)),
            &[self.user()],
        )?;
        let similar: QueryResult = self.http.get(
            "similar movies",
            &format!("/Items/{}/Similar", encode(&film.0)),
            &[self.user(), ("limit", SIMILAR.to_string())],
        )?;
        Ok(FilmDetail {
            tmdb: item.provider_id("Tmdb"),
            tmdb_collection: item.provider_id("TmdbCollection"),
            trailers: item.trailers(),
            film: item.film(),
            logo: item.logo(),
            directors: item.directors(),
            cast: item.cast(),
            similar: similar.items.iter().map(Item::film).collect(),
            overview: item.overview,
            genres: item.genres,
            official_rating: item.official_rating,
            favourite: item.user_data.as_ref().is_some_and(|u| u.is_favorite),
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
                    ("includeItemTypes", "Movie".into()),
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
        // No device profile: the file is played as it is whatever the server thinks of it; a
        // conversion is asked only once the player could not play it.
        let body = json!({
            "UserId": self.login.user_id,
            "MediaSourceId": source.0,
            "EnableDirectPlay": true,
            "EnableDirectStream": false,
            "EnableTranscoding": false,
            "AutoOpenLiveStream": false,
        });
        let (session, found) = self.playback_info(film, source, &body)?;
        // `ApiKey`, not the older `api_key`: Jellyfin 10.11 keeps that one behind a legacy setting.
        let stream = format!(
            "{}/Videos/{}/stream?static=true&mediaSourceId={}\
             &playSessionId={}&deviceId={}&ApiKey={}",
            self.http.base(),
            encode(&film.0),
            encode(&source.0),
            encode(&session),
            encode(&self.http.device().id),
            encode(self.token()),
        );
        Ok(self.opened(film, source, session, &found, stream))
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
        )?;
        if report.method != PlayMethod::Transcode {
            return Ok(());
        }
        // Nothing reads the conversion any more: the server's encoder stops now, not when it
        // notices.
        self.http.delete(
            "stop the conversion",
            "/Videos/ActiveEncodings",
            &[
                ("deviceId", self.http.device().id.clone()),
                ("playSessionId", report.session.0.clone()),
            ],
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
            ImageKind::Logo => "Logo",
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
        let mut account = AccountFile::open(&self.path)?;
        account.login = None;
        // The server stays: the login that follows starts from its address.
        account.last_address = Some(self.login.address.clone());
        // The file first: when the keyring cannot be reached the token stays in it, but nothing
        // points at it any more, so the next start never signs back in silently.
        account.save(&self.path)?;
        self.store.delete(&self.login.keyring_account())
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
            tmdb: item.provider_id("Tmdb"),
            trailers: item.trailers(),
            series: item.series(),
            favourite: item.user_data.as_ref().is_some_and(|u| u.is_favorite),
            cast: item.cast(),
            similar: similar.items.iter().map(Item::series).collect(),
            logo: item.logo(),
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

    fn next_episodes(&self, limit: u32) -> Result<Vec<Episode>, ProviderError> {
        let found: QueryResult = self.http.get(
            "next up",
            "/Shows/NextUp",
            &[
                self.user(),
                ("limit", limit.to_string()),
                ("enableResumable", "true".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary,Backdrop".into()),
                ("fields", EPISODE_FIELDS.into()),
            ],
        )?;
        Ok(found.items.into_iter().map(Item::episode).collect())
    }

    /// Not an item list: Jellyfin answers this one with a bare array. The episodes of a series
    /// library come grouped under their series.
    fn latest(&self, library: &ItemId, limit: u32) -> Result<Vec<Title>, ProviderError> {
        let found: Vec<Item> = self.http.get(
            "latest",
            "/Items/Latest",
            &[
                self.user(),
                ("parentId", library.0.clone()),
                ("limit", limit.to_string()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary,Backdrop".into()),
                // An episode or a season that comes alone is listed as its series, with the series
                // poster.
                ("fields", "SeriesPrimaryImage".into()),
            ],
        )?;
        // Two episodes of one series, or one episode beside its series, are one entry.
        let mut titles: Vec<Title> = Vec::new();
        for title in found.into_iter().filter_map(Item::latest) {
            if !titles.iter().any(|seen| seen.id() == title.id()) {
                titles.push(title);
            }
        }
        Ok(titles)
    }

    /// The simplest honest choice: the newest titles, films and series, that the user has not
    /// watched, and that have a backdrop to fill the header.
    fn featured(&self, limit: u32) -> Result<Vec<Feature>, ProviderError> {
        let found = self.items(
            "featured",
            &[
                ("includeItemTypes", "Movie,Series".into()),
                ("recursive", "true".into()),
                ("sortBy", "DateCreated".into()),
                ("sortOrder", "Descending".into()),
                ("isPlayed", "false".into()),
                ("imageTypes", "Backdrop".into()),
                ("limit", limit.to_string()),
                ("fields", "Overview,OfficialRating".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Backdrop,Logo".into()),
            ],
        )?;
        Ok(found.items.iter().map(Item::feature).collect())
    }

    fn collections(&self) -> Result<Vec<CollectionCard>, ProviderError> {
        let found = self.items(
            "collections",
            &[
                ("includeItemTypes", "BoxSet".into()),
                ("recursive", "true".into()),
                ("sortBy", "SortName".into()),
                ("fields", "ChildCount".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary".into()),
            ],
        )?;
        // A card with no picture is bare here: the grid must not wait for one request per
        // collection, so its poster comes by `collection_poster`, after the grid shows.
        Ok(found.items.iter().map(Item::collection_card).collect())
    }

    fn collection_poster(&self, collection: &ItemId) -> Result<Option<ImageRef>, ProviderError> {
        // `imageTypes` keeps the titles with a poster: the first one listed may have none while
        // the next has one. A collection of series holds no film at all.
        let found = self.items(
            "collection poster",
            &[
                ("parentId", collection.0.clone()),
                ("includeItemTypes", "Movie,Series".into()),
                ("sortBy", "ProductionYear,PremiereDate,SortName".into()),
                ("imageTypes", "Primary".into()),
                ("limit", "1".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary".into()),
            ],
        )?;
        Ok(found.items.first().and_then(|item| item.film().poster))
    }

    fn collection(&self, collection: &ItemId) -> Result<CollectionDetail, ProviderError> {
        let item: Item = self.http.get(
            "collection",
            &format!("/Items/{}", encode(&collection.0)),
            &[self.user()],
        )?;
        let films = self.items(
            "collection movies",
            &[
                ("parentId", collection.0.clone()),
                ("includeItemTypes", "Movie".into()),
                ("sortBy", "ProductionYear,PremiereDate,SortName".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary,Backdrop".into()),
            ],
        )?;
        Ok(CollectionDetail {
            id: ItemId(item.id.clone()),
            name: item.name.clone(),
            backdrop: item.film().backdrop,
            overview: item.overview,
            films: films.items.iter().map(Item::film).collect(),
        })
    }

    fn favourites(&self) -> Result<Vec<Favourite>, ProviderError> {
        let found = self.items(
            "favorites",
            &[
                ("filters", "IsFavorite".into()),
                ("includeItemTypes", "Movie,Series".into()),
                ("recursive", "true".into()),
                ("sortBy", "SortName".into()),
                ("imageTypeLimit", "1".into()),
                ("enableImageTypes", "Primary,Backdrop".into()),
            ],
        )?;
        Ok(found
            .items
            .iter()
            .map(|item| match item.kind.as_deref() {
                Some("Series") => Favourite::Series(item.series()),
                _ => Favourite::Film(item.film()),
            })
            .collect())
    }

    fn set_favourite(&self, item: &ItemId, favourite: bool) -> Result<(), ProviderError> {
        let path = format!("/UserFavoriteItems/{}", encode(&item.0));
        if favourite {
            self.http
                .post_empty("add to the list", &path, &[self.user()])
        } else {
            self.http
                .delete("take out of the list", &path, &[self.user()])
        }
    }

    /// One request for the playlists, then one per playlist for what it holds, one after
    /// another: a user has a few. One the server will not read (shared, then taken away) is
    /// left out rather than failing the others.
    fn playlists(&self, first: Option<usize>) -> Result<Vec<Playlist>, ProviderError> {
        let found = self.items(
            "playlists",
            &[
                ("includeItemTypes", "Playlist".into()),
                ("recursive", "true".into()),
                ("sortBy", "SortName".into()),
            ],
        )?;
        let mut playlists = Vec::new();
        for item in found.items {
            if item.media_type.as_deref() == Some("Audio") {
                continue;
            }
            let id = ItemId(item.id);
            let (entries, count) = match self.entries(&id, "", first) {
                Ok(read) => read,
                Err(ProviderError::Forbidden | ProviderError::NotFound) => continue,
                Err(error) => return Err(error),
            };
            playlists.push(Playlist {
                id,
                name: item.name,
                entries,
                count,
            });
        }
        Ok(playlists)
    }

    /// An episode played from the playlist plays without its page: it needs its versions now.
    fn playlist(&self, playlist: &ItemId) -> Result<Vec<PlaylistEntry>, ProviderError> {
        Ok(self.entries(playlist, EPISODE_FIELDS, None)?.0)
    }

    fn create_playlist(&self, name: &str, item: &ItemId) -> Result<(ItemId, usize), ProviderError> {
        let made: dto::PlaylistCreated = self.http.post(
            "new playlist",
            "/Playlists",
            &[],
            &json!({
                "Name": name,
                "Ids": [item.0],
                "UserId": self.login.user_id,
                "MediaType": "Video",
            }),
        )?;
        let id = ItemId(made.id);
        // As for an add: made, but not counted, it holds the one title.
        let count = self.entries(&id, "", Some(0)).map_or(1, |(_, count)| count);
        Ok((id, count))
    }

    /// The server answers nothing of what it put in: its count, before and after, says it.
    fn add_to_playlist(&self, playlist: &ItemId, item: &ItemId) -> Result<usize, ProviderError> {
        let (_, before) = self.entries(playlist, "", Some(0))?;
        self.http.post_empty(
            "add to the playlist",
            &format!("/Playlists/{}/Items", encode(&playlist.0)),
            &[("ids", item.0.clone()), self.user()],
        )?;
        // Put in, but not counted again: one entry, as for a film or an episode.
        let after = self.entries(playlist, "", Some(0)).map(|(_, count)| count);
        Ok(after.map_or(1, |after| after.saturating_sub(before)))
    }

    fn remove_from_playlist(
        &self,
        playlist: &ItemId,
        entry: &EntryId,
    ) -> Result<(), ProviderError> {
        self.http.delete(
            "take out of the playlist",
            &format!("/Playlists/{}/Items", encode(&playlist.0)),
            &[("entryIds", entry.0.clone())],
        )
    }

    fn trickplay(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
    ) -> Result<Option<Trickplay>, ProviderError> {
        let item: Item = self.http.get(
            "trickplay",
            &format!("/Items/{}", encode(&film.0)),
            &[self.user(), ("fields", "Trickplay".into())],
        )?;
        Ok(item.trickplay_of(&source.0))
    }

    fn trickplay_tile(
        &self,
        film: &ItemId,
        source: &MediaSourceId,
        trickplay: &Trickplay,
        tile: u32,
    ) -> Result<Vec<u8>, ProviderError> {
        // The token goes in the header, as for every image: the address carries none.
        self.http.get_bytes(
            "trickplay tile",
            &format!(
                "/Videos/{}/Trickplay/{}/{tile}.jpg",
                encode(&film.0),
                trickplay.width
            ),
            &[("mediaSourceId", source.0.clone())],
        )
    }

    fn transcode(&self, request: &TranscodeRequest) -> Result<Playback, ProviderError> {
        let (film, source) = (&request.film, &request.source);
        let max_bitrate = request.max_bitrate.unwrap_or(MAX_STREAMING_BITRATE);
        let body = json!({
            "UserId": self.login.user_id,
            "MediaSourceId": source.0,
            "EnableDirectPlay": false,
            "EnableDirectStream": false,
            "EnableTranscoding": true,
            "AutoOpenLiveStream": false,
            "MaxStreamingBitrate": max_bitrate,
            "DeviceProfile": device_profile(max_bitrate),
            "StartTimeTicks": dto::ticks(request.start),
            "AudioStreamIndex": request.audio_track,
            "SubtitleStreamIndex": request.subtitle_track,
        });
        let (session, found) = self.playback_info(film, source, &body)?;
        let path = found.transcoding_url.as_deref().ok_or_else(|| {
            ProviderError::BadAnswer("playback: the server converts nothing for it".into())
        })?;
        let stream = self.served(path);
        let mut playback = self.opened(film, source, session, &found, stream);
        // The embedded text subtitle chosen comes as a file the server extracts, beside the
        // converted picture.
        let delivered = found.media_streams.iter().find(|s| {
            s.kind == "Subtitle"
                && !s.is_external
                && Some(s.index) == request.subtitle_track
                && s.delivery_method.as_deref() == Some("External")
        });
        if let Some(subtitle) = delivered
            && let Some(path) = subtitle.delivery_url.as_deref()
        {
            playback.external_subtitles.push(ExternalSubtitle {
                index: subtitle.index,
                url: StreamUrl::new(self.served(path)),
            });
        }
        Ok(playback)
    }

    /// Servers before 10.10 have no such route: they answer 404, which is no segments.
    fn media_segments(&self, item: &ItemId) -> Result<Vec<Segment>, ProviderError> {
        let kinds = ["Intro", "Outro", "Recap", "Preview"];
        let query: Vec<(&str, String)> = kinds
            .into_iter()
            .map(|kind| ("includeSegmentTypes", kind.to_string()))
            .collect();
        let found: Result<dto::SegmentsResult, ProviderError> = self.http.get(
            "media segments",
            &format!("/MediaSegments/{}", encode(&item.0)),
            &query,
        );
        match found {
            Ok(found) => Ok(found.segments()),
            Err(ProviderError::NotFound) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    fn tmdb_index(&self) -> Result<TmdbIndex, ProviderError> {
        let found: QueryResult = self.http.get(
            "the library's TMDb ids",
            "/Items",
            &[
                self.user(),
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Movie,Series".into()),
                ("HasTmdbId", "true".into()),
                ("Fields", "ProviderIds".into()),
                ("EnableImages", "false".into()),
                ("EnableUserData", "false".into()),
                ("EnableTotalRecordCount", "false".into()),
            ],
        )?;
        let mut index = TmdbIndex::default();
        for item in &found.items {
            let Some(tmdb) = item.provider_id("Tmdb") else {
                continue;
            };
            let id = ItemId(item.id.clone());
            match item.kind.as_deref() {
                Some("Movie") => index.films.insert(tmdb, id),
                Some("Series") => index.series.insert(tmdb, id),
                _ => None,
            };
        }
        Ok(index)
    }

    fn local_trailer(&self, item: &ItemId) -> Result<StreamUrl, ProviderError> {
        let found: Vec<Item> = self.http.get(
            "trailer",
            &format!("/Items/{}/LocalTrailers", encode(&item.0)),
            &[self.user()],
        )?;
        let trailer = found.first().ok_or(ProviderError::NotFound)?;
        // A file may have a version id of its own, apart from the item's.
        let source = trailer
            .media_sources
            .first()
            .map_or(trailer.id.as_str(), |source| source.id.as_str());
        // Direct play, with no play session: nothing here is a playback the server follows.
        Ok(StreamUrl::new(format!(
            "{}/Videos/{}/stream?static=true&mediaSourceId={}&deviceId={}&ApiKey={}",
            self.http.base(),
            encode(&trailer.id),
            encode(source),
            encode(&self.http.device().id),
            encode(self.token()),
        )))
    }

    /// The plugin's plays first; its routes answer 404 when it is not installed, and 403 to a
    /// user who is not an administrator: the titles marked played are read then.
    fn watch_history(&self) -> Result<WatchHistory, ProviderError> {
        let rows = self.reporting_rows()?;
        if !rows.is_empty() {
            let titles = self.watched_titles()?;
            let plays = rows.into_iter().map(|row| row.play(&titles)).collect();
            return Ok(WatchHistory {
                source: HistorySource::PlaybackReporting,
                plays,
            });
        }
        let played = self.items(
            "played items",
            &[
                ("IsPlayed", "true".into()),
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Movie,Episode".into()),
                ("Fields", "UserData,Genres,RunTimeTicks".into()),
            ],
        )?;
        let titles = if played.items.is_empty() {
            Titles::default()
        } else {
            self.watched_titles()?
        };
        Ok(WatchHistory {
            source: HistorySource::PlayedItems,
            plays: played
                .items
                .iter()
                .flat_map(|item| played_item(item, &titles))
                .collect(),
        })
    }
}

impl Jellyfin {
    /// The signed-in user's plays the Playback Reporting plugin recorded; none when it is not
    /// there or not open to them. Its report routes add up every user's plays: only its query
    /// route gives one user's, each with its time and title.
    fn reporting_rows(&self) -> Result<Vec<ReportingRow>, ProviderError> {
        let (Some(query), Some(user)) = (
            reporting_query(&self.login.user_id),
            plugin_user_id(&self.login.user_id),
        ) else {
            return Ok(Vec::new());
        };
        let answer: Result<dto::CustomQueryAnswer, ProviderError> = self.http.post(
            "watch history",
            "/user_usage_stats/submit_custom_query",
            &[],
            &json!({ "CustomQueryString": query, "ReplaceUserId": false }),
        );
        let answer = match answer {
            Ok(answer) => answer,
            Err(ProviderError::NotFound | ProviderError::Forbidden) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let Some(at) = REPORTING_COLUMNS
            .iter()
            .map(|name| answer.colums.iter().position(|column| column == name))
            .collect::<Option<Vec<usize>>>()
        else {
            return Ok(Vec::new());
        };
        let cell = |row: &[serde_json::Value], column: usize| match row.get(at[column]) {
            Some(serde_json::Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        Ok(answer
            .results
            .iter()
            // The query asked only the user's rows: what else comes back is not counted.
            .filter(|row| plugin_user_id(&cell(row, 1)).as_deref() == Some(user.as_str()))
            .map(|row| ReportingRow {
                at: LocalTime::parse(&cell(row, 0)),
                item: cell(row, 2),
                episode: cell(row, 3) == "Episode",
                name: cell(row, 4),
                seconds: cell(row, 5).parse().unwrap_or(0),
            })
            .collect())
    }

    /// The library's movies and series, with their genres and posters, to name what was played.
    fn watched_titles(&self) -> Result<Titles, ProviderError> {
        let found = self.items(
            "the library's titles",
            &[
                ("Recursive", "true".into()),
                ("IncludeItemTypes", "Movie,Series".into()),
                ("Fields", "Genres".into()),
                ("EnableUserData", "false".into()),
                ("EnableTotalRecordCount", "false".into()),
                ("ImageTypeLimit", "1".into()),
                ("EnableImageTypes", "Primary".into()),
            ],
        )?;
        let mut titles = Titles::default();
        for item in &found.items {
            let kind = match item.kind.as_deref() {
                Some("Movie") => WatchedKind::Movie,
                Some("Series") => WatchedKind::Series,
                _ => continue,
            };
            let watched = Watched {
                id: Some(ItemId(item.id.clone())),
                name: item.name.clone(),
                kind,
                poster: item.list_poster(),
                genres: item.genres.clone(),
            };
            if kind == WatchedKind::Series {
                titles
                    .series_by_name
                    .insert(item.name.clone(), watched.clone());
            }
            titles.by_id.insert(item.id.clone(), watched);
        }
        Ok(titles)
    }
}

/// The library's titles by id, and its series by name too: the plugin names an episode's series
/// only in the episode's name.
#[derive(Default)]
struct Titles {
    by_id: std::collections::HashMap<String, Watched>,
    series_by_name: std::collections::HashMap<String, Watched>,
}

/// The columns read from the plugin's table, in this order.
const REPORTING_COLUMNS: [&str; 6] = [
    "DateCreated",
    "UserId",
    "ItemId",
    "ItemType",
    "ItemName",
    "PlayDuration",
];

/// A play the plugin recorded. Its time is the server's clock, taken as the local one.
struct ReportingRow {
    at: Option<LocalTime>,
    item: String,
    episode: bool,
    name: String,
    seconds: u64,
}

impl ReportingRow {
    fn play(self, titles: &Titles) -> Play {
        let title = if self.episode {
            let series = series_of_episode(&self.name);
            titles
                .series_by_name
                .get(series)
                .cloned()
                .unwrap_or_else(|| gone(series, WatchedKind::Series))
        } else {
            titles
                .by_id
                .get(&self.item)
                .cloned()
                .unwrap_or_else(|| gone(&self.name, WatchedKind::Movie))
        };
        Play {
            title,
            at: self.at,
            seconds: self.seconds,
            count: 1,
        }
    }
}

/// A title the library no longer holds, known by its name.
fn gone(name: &str, kind: WatchedKind) -> Watched {
    Watched {
        id: None,
        name: name.to_string(),
        kind,
        poster: None,
        genres: Vec::new(),
    }
}

/// The series of an episode the plugin names `Series - s01e02 - Episode`.
fn series_of_episode(name: &str) -> &str {
    let mut end = name.len();
    while let Some(at) = name[..end].rfind(" - s") {
        let rest = &name[at + 4..];
        let season = rest.bytes().take_while(u8::is_ascii_digit).count();
        let episode = rest[season..]
            .strip_prefix('e')
            .map(|after| (after, after.bytes().take_while(u8::is_ascii_digit).count()));
        if let Some((after, digits)) = episode
            && season > 0
            && digits > 0
            && after[digits..].starts_with(" - ")
        {
            return &name[..at];
        }
        end = at;
    }
    name
}

/// The plays of a title marked played: its last play on its date, the ones before at no known
/// time, each as long as the title. A title marked played by hand was watched once.
fn played_item(item: &Item, titles: &Titles) -> Vec<Play> {
    let title = match item.kind.as_deref() {
        Some("Episode") => {
            let series = item.series_id.clone().unwrap_or_default();
            titles
                .by_id
                .get(&series)
                .cloned()
                .unwrap_or_else(|| Watched {
                    id: item.series_id.clone().map(ItemId),
                    name: item.series_name.clone().unwrap_or_default(),
                    kind: WatchedKind::Series,
                    poster: item.list_poster(),
                    genres: Vec::new(),
                })
        }
        _ => titles
            .by_id
            .get(&item.id)
            .cloned()
            .unwrap_or_else(|| Watched {
                id: Some(ItemId(item.id.clone())),
                name: item.name.clone(),
                kind: WatchedKind::Movie,
                poster: item.list_poster(),
                genres: item.genres.clone(),
            }),
    };
    let length = item.run_time_ticks.unwrap_or(0).max(0) as u64 / 10_000_000;
    let user_data = item.user_data.as_ref();
    let count = user_data.map_or(0, |data| data.play_count).max(1);
    let last = user_data
        .and_then(|data| data.last_played_date.as_deref())
        .and_then(LocalTime::from_utc);
    let (dated, before) = if last.is_some() {
        (1, count - 1)
    } else {
        (0, count)
    };
    let mut plays = Vec::new();
    if dated == 1 {
        plays.push(Play {
            title: title.clone(),
            at: last,
            seconds: length,
            count: 1,
        });
    }
    if before > 0 {
        plays.push(Play {
            title,
            at: None,
            seconds: length * u64::from(before),
            count: before,
        });
    }
    plays
}

/// The plugin's user ids are Jellyfin's without dashes, in lower case. None for anything that is
/// not hex: it would not be put in a query.
fn plugin_user_id(user_id: &str) -> Option<String> {
    let id = user_id.replace('-', "").to_ascii_lowercase();
    (!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(id)
}

/// The query of the signed-in user's plays of movies and episodes. The plugin runs whatever it
/// is sent, writes included: only this SELECT is ever sent, constants and the user's id, which
/// is hex or nothing is asked.
fn reporting_query(user_id: &str) -> Option<String> {
    let id = plugin_user_id(user_id)?;
    Some(format!(
        "SELECT {} FROM PlaybackActivity WHERE UserId = '{id}' \
         AND ItemType IN ('Movie', 'Episode') ORDER BY DateCreated",
        REPORTING_COLUMNS.join(", ")
    ))
}
