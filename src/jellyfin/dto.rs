//! Jellyfin's JSON, as much of it as Bobine reads, and its translation into the port's types.

use crate::provider::{
    AudioTrack, CastMember, CollectionCard, Episode, Feature, FeatureKind, Film, ImageKind,
    ImageRef, ItemId, Library, LibraryKind, MediaSource, MediaSourceId, NewEpisode, PersonHit,
    PersonTitles, Season, Segment, SegmentKind, Series, SeriesHit, ServerInfo, SubtitleTrack,
    Title, Trailers, Trickplay, VideoRange, VideoTrack,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// Jellyfin counts time in ticks of 100 ns.
pub fn duration(ticks: i64) -> Duration {
    Duration::from_nanos(u64::try_from(ticks).unwrap_or(0).saturating_mul(100))
}

pub fn ticks(position: Duration) -> i64 {
    i64::try_from(position.as_nanos() / 100).unwrap_or(i64::MAX)
}

/// Seconds since 1970 of a date as Jellyfin writes it, in UTC: `2025-09-14T08:30:05.1234567Z`.
/// The fraction of a second is dropped.
pub fn seconds(date: &str) -> Option<i64> {
    let number = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = date.get(range)?;
        digits
            .bytes()
            .all(|b| b.is_ascii_digit())
            .then(|| digits.parse().ok())?
    };
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if date.get(4..5) != Some("-") || date.get(10..11) != Some("T") || !date.ends_with('Z') {
        return None;
    }
    // Days from the civil date (H. Hinnant's algorithm), the year starting in March.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicSystemInfo {
    pub id: String,
    pub server_name: String,
    pub version: String,
    pub product_name: Option<String>,
}

impl PublicSystemInfo {
    pub fn into_info(self) -> ServerInfo {
        ServerInfo {
            id: self.id,
            name: self.server_name,
            version: self.version,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    pub user: UserDto,
    pub access_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub id: String,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult {
    pub items: Vec<Item>,
    #[serde(default)]
    pub total_record_count: Option<u32>,
}

/// What `POST /Playlists` answers.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistCreated {
    pub id: String,
}

/// What `/MediaSegments/{id}` lists.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SegmentsResult {
    pub items: Vec<SegmentDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SegmentDto {
    pub r#type: String,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

impl SegmentsResult {
    /// The segments Bobine skips, by start. Commercials and kinds it does not know are left out,
    /// and so is a segment that ends where it starts or before.
    pub fn segments(self) -> Vec<Segment> {
        let mut segments: Vec<Segment> = self
            .items
            .into_iter()
            .filter_map(|dto| {
                let kind = match dto.r#type.as_str() {
                    "Intro" => SegmentKind::Intro,
                    "Outro" => SegmentKind::Outro,
                    "Recap" => SegmentKind::Recap,
                    "Preview" => SegmentKind::Preview,
                    _ => return None,
                };
                let (start, end) = (duration(dto.start_ticks), duration(dto.end_ticks));
                (end > start).then_some(Segment { kind, start, end })
            })
            .collect();
        segments.sort_by_key(|segment| segment.start);
        segments
    }
}

/// What `/Items/Filters` lists of a library; only its years are read.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct QueryFilters {
    pub years: Vec<i32>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserData {
    pub played: bool,
    pub playback_position_ticks: i64,
    pub unplayed_item_count: Option<u32>,
    pub is_favorite: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub collection_type: Option<String>,
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub production_year: Option<i32>,
    pub community_rating: Option<f32>,
    pub run_time_ticks: Option<i64>,
    pub user_data: Option<UserData>,
    #[serde(default)]
    pub image_tags: HashMap<String, String>,
    #[serde(default)]
    pub backdrop_image_tags: Vec<String>,
    pub overview: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    pub official_rating: Option<String>,
    /// An episode's series.
    pub series_id: Option<String>,
    pub series_name: Option<String>,
    pub series_primary_image_tag: Option<String>,
    #[serde(default)]
    pub people: Vec<Person>,
    #[serde(default)]
    pub media_sources: Vec<MediaSourceInfo>,
    /// A series' end, `2024-05-12T00:00:00.0000000Z`.
    pub end_date: Option<String>,
    /// When the server added the item, `2025-09-14T08:30:05.1234567Z`, when asked for.
    pub date_created: Option<String>,
    /// A season's or an episode's number.
    pub index_number: Option<u32>,
    /// An episode's season number.
    pub parent_index_number: Option<u32>,
    pub season_id: Option<String>,
    /// A collection's films, when asked for.
    pub child_count: Option<u32>,
    /// A playlist's: `Video`, `Audio`.
    pub media_type: Option<String>,
    /// An item's place in the playlist it was listed from.
    pub playlist_item_id: Option<String>,
    /// The trickplay images, when asked for (`fields=Trickplay`): by version, then by width.
    #[serde(default)]
    pub trickplay: HashMap<String, HashMap<String, TrickplayInfo>>,
    /// Its ids elsewhere, `Tmdb`, `TmdbCollection`, `Imdb`…
    #[serde(default)]
    pub provider_ids: HashMap<String, String>,
    pub local_trailer_count: Option<u32>,
    #[serde(default)]
    pub remote_trailers: Vec<RemoteTrailer>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct RemoteTrailer {
    pub url: Option<String>,
}

/// How one version's trickplay images are cut at one width. The interval is in milliseconds.
#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct TrickplayInfo {
    pub width: u32,
    pub height: u32,
    pub tile_width: u32,
    pub tile_height: u32,
    pub thumbnail_count: u32,
    pub interval: u64,
}

impl Item {
    /// One of its ids elsewhere, as a number: `Tmdb`, `TmdbCollection`.
    pub fn provider_id(&self, provider: &str) -> Option<u64> {
        self.provider_ids
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(provider))
            .and_then(|(_, id)| id.trim().parse().ok())
    }

    /// The trickplay to show for one version, if the server made any for it.
    pub fn trickplay_of(&self, source: &str) -> Option<Trickplay> {
        let sizes = self.trickplay.get(source)?;
        Trickplay::choose(sizes.values().map(|info| Trickplay {
            width: info.width,
            height: info.height,
            columns: info.tile_width,
            rows: info.tile_height,
            count: info.thumbnail_count,
            interval: Duration::from_millis(info.interval),
        }))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Person {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    #[serde(rename = "Type")]
    pub kind: Option<String>,
    pub primary_image_tag: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceInfo {
    pub id: String,
    pub name: Option<String>,
    pub container: Option<String>,
    #[serde(default)]
    pub media_streams: Vec<MediaStream>,
    /// Relative to the server, with its own play session and token.
    pub transcoding_url: Option<String>,
    /// Chosen by the server from the settings of the user asked for; -1 for no subtitles.
    pub default_audio_stream_index: Option<i32>,
    pub default_subtitle_stream_index: Option<i32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    #[serde(rename = "Type")]
    pub kind: String,
    pub index: i32,
    pub codec: Option<String>,
    pub language: Option<String>,
    pub title: Option<String>,
    pub display_title: Option<String>,
    #[serde(default)]
    pub is_default: bool,
    #[serde(default)]
    pub is_forced: bool,
    #[serde(default)]
    pub is_external: bool,
    /// In a conversion, how a subtitle comes: `External` for a file at `delivery_url`, relative
    /// to the server and holding the token.
    pub delivery_method: Option<String>,
    pub delivery_url: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub channels: Option<u32>,
    pub video_range_type: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    #[serde(default)]
    pub media_sources: Vec<MediaSourceInfo>,
    pub play_session_id: Option<String>,
}

impl Item {
    pub fn into_library(self) -> Library {
        let kind = match self.collection_type.as_deref() {
            Some("movies") => LibraryKind::Films,
            Some("tvshows") => LibraryKind::Series,
            _ => LibraryKind::Other,
        };
        Library {
            id: ItemId(self.id),
            name: self.name,
            kind,
        }
    }

    pub fn film(&self) -> Film {
        let image = |kind, tag: &String| ImageRef {
            item: ItemId(self.id.clone()),
            kind,
            tag: tag.clone(),
        };
        let user_data = self.user_data.as_ref();
        Film {
            id: ItemId(self.id.clone()),
            title: self.name.clone(),
            year: self.production_year,
            community_rating: self.community_rating,
            runtime: self.run_time_ticks.map(duration),
            watched: user_data.is_some_and(|u| u.played),
            resume: user_data
                .map(|u| u.playback_position_ticks)
                .filter(|&t| t > 0)
                .map(duration),
            favourite: user_data.is_some_and(|u| u.is_favorite),
            poster: self
                .image_tags
                .get("Primary")
                .map(|tag| image(ImageKind::Primary, tag)),
            backdrop: self
                .backdrop_image_tags
                .first()
                .map(|tag| image(ImageKind::Backdrop, tag)),
        }
    }

    pub fn series(&self) -> Series {
        if let Some(series) = self.season_as_series() {
            return series;
        }
        let film = self.film();
        Series {
            id: film.id,
            title: film.title,
            year: film.year,
            end_year: self
                .end_date
                .as_deref()
                .and_then(|date| date.get(..4)?.parse().ok()),
            community_rating: film.community_rating,
            unwatched: self
                .user_data
                .as_ref()
                .and_then(|u| u.unplayed_item_count)
                .unwrap_or(0),
            watched: Some(film.watched),
            favourite: Some(film.favourite),
            poster: film.poster,
            backdrop: film.backdrop,
            season: None,
        }
    }

    /// A season the server hands where a series is expected (a series that gained a season, in
    /// `/Items/Latest`): its series, with the season to open it on. A season has no page of its
    /// own, and no backdrop worth borrowing.
    fn season_as_series(&self) -> Option<Series> {
        if self.kind.as_deref() != Some("Season") {
            return None;
        }
        let id = ItemId(self.series_id.clone()?);
        // The series' poster when the server gave it, else the season's own.
        let poster = match &self.series_primary_image_tag {
            Some(tag) => Some(ImageRef {
                item: id.clone(),
                kind: ImageKind::Primary,
                tag: tag.clone(),
            }),
            None => self.primary(),
        };
        let season = self.season();
        Some(Series {
            id,
            title: self.series_name.clone().unwrap_or_default(),
            year: None,
            end_year: None,
            community_rating: None,
            // What is left to watch in the new season, which is what the card is about.
            unwatched: season.unwatched,
            // The season's own state is not its series': the card offers no button for them.
            watched: None,
            favourite: None,
            poster,
            backdrop: None,
            season: Some(season),
        })
    }

    pub fn collection_card(&self) -> CollectionCard {
        CollectionCard {
            id: ItemId(self.id.clone()),
            name: self.name.clone(),
            films: self.child_count,
            poster: self.primary(),
        }
    }

    pub fn season(&self) -> Season {
        Season {
            id: ItemId(self.id.clone()),
            name: self.name.clone(),
            number: self.index_number,
            unwatched: self
                .user_data
                .as_ref()
                .and_then(|u| u.unplayed_item_count)
                .unwrap_or(0),
        }
    }

    pub fn episode(self) -> Episode {
        Episode {
            item: self.film(),
            series: ItemId(self.series_id.unwrap_or_default()),
            series_title: self.series_name.unwrap_or_default(),
            season: ItemId(self.season_id.unwrap_or_default()),
            season_number: self.parent_index_number,
            number: self.index_number,
            overview: self.overview,
            versions: self
                .media_sources
                .into_iter()
                .map(MediaSourceInfo::into_source)
                .collect(),
        }
    }

    /// An episode as the latest added list holds it, with its series' poster. None without
    /// the date it was added, which nothing can be told from.
    pub fn new_episode(self) -> Option<NewEpisode> {
        let added = seconds(self.date_created.as_deref()?)?;
        let poster = self.list_poster();
        Some(NewEpisode {
            episode: self.episode(),
            added,
            poster,
        })
    }

    /// What a row of the Home lists: an episode, a series (or the season of one), else a film.
    /// A season that names no series has no page to open: it is not listed.
    pub fn title(self) -> Option<Title> {
        Some(match self.kind.as_deref() {
            Some("Episode") => Title::Episode(self.episode()),
            Some("Season") if self.series_id.is_none() => return None,
            Some("Series" | "Season") => Title::Series(self.series()),
            _ => Title::Film(self.film()),
        })
    }

    /// What a library's latest list holds: `/Items/Latest` groups the episodes of a series only
    /// when several came in, so a lone one comes as the episode, and is listed as its series.
    pub fn latest(self) -> Option<Title> {
        if self.kind.as_deref() != Some("Episode") {
            return self.title();
        }
        let Some(id) = self.series_id.clone() else {
            return self.title();
        };
        let poster = self.series_primary_image_tag.as_ref().map(|tag| ImageRef {
            item: ItemId(id.clone()),
            kind: ImageKind::Primary,
            tag: tag.clone(),
        });
        Some(Title::Series(Series {
            id: ItemId(id),
            title: self.series_name.unwrap_or_default(),
            year: None,
            end_year: None,
            community_rating: None,
            unwatched: 0,
            // The episode says nothing of its series' state: the series page reads it.
            watched: None,
            favourite: None,
            poster,
            backdrop: None,
            season: None,
        }))
    }

    /// What the top of the Home says of a film or a series.
    pub fn feature(&self) -> Feature {
        let film = self.film();
        let series = self.kind.as_deref() == Some("Series");
        Feature {
            kind: if series {
                FeatureKind::Series
            } else {
                FeatureKind::Film
            },
            id: film.id,
            title: film.title,
            year: film.year,
            community_rating: film.community_rating,
            official_rating: self.official_rating.clone(),
            runtime: film.runtime.filter(|_| !series),
            overview: self.overview.clone().filter(|text| !text.is_empty()),
            backdrop: film.backdrop,
            logo: self.logo(),
            resume: film.resume.filter(|_| !series),
        }
    }

    /// The poster a list shows for the item: an episode's series' poster, not its wide still.
    pub fn list_poster(&self) -> Option<ImageRef> {
        if self.kind.as_deref() != Some("Episode") {
            return self.primary();
        }
        Some(ImageRef {
            item: ItemId(self.series_id.clone()?),
            kind: ImageKind::Primary,
            tag: self.series_primary_image_tag.clone()?,
        })
    }

    /// The title drawn as an image, when the server has one.
    pub fn logo(&self) -> Option<ImageRef> {
        self.image_tags.get("Logo").map(|tag| ImageRef {
            item: ItemId(self.id.clone()),
            kind: ImageKind::Logo,
            tag: tag.clone(),
        })
    }

    fn primary(&self) -> Option<ImageRef> {
        self.image_tags.get("Primary").map(|tag| ImageRef {
            item: ItemId(self.id.clone()),
            kind: ImageKind::Primary,
            tag: tag.clone(),
        })
    }

    pub fn series_hit(&self) -> SeriesHit {
        SeriesHit {
            id: ItemId(self.id.clone()),
            title: self.name.clone(),
            year: self.production_year,
            poster: self.primary(),
            favourite: self.user_data.as_ref().map(|u| u.is_favorite),
            watched: self.user_data.as_ref().map(|u| u.played),
        }
    }

    /// The series an episode belongs to, with the series' poster.
    fn series_of_episode(&self) -> Option<SeriesHit> {
        let id = self.series_id.as_ref()?;
        Some(SeriesHit {
            id: ItemId(id.clone()),
            title: self.series_name.clone()?,
            year: None,
            poster: self.series_primary_image_tag.as_ref().map(|tag| ImageRef {
                item: ItemId(id.clone()),
                kind: ImageKind::Primary,
                tag: tag.clone(),
            }),
            favourite: None,
            watched: None,
        })
    }

    pub fn person_hit(&self) -> PersonHit {
        PersonHit {
            id: ItemId(self.id.clone()),
            name: self.name.clone(),
            photo: self.primary(),
        }
    }

    /// Its trailers. A remote one is kept only when it is a web address: the player must never
    /// be sent to a file of this machine by a server.
    pub fn trailers(&self) -> Trailers {
        Trailers {
            local: self.local_trailer_count.is_some_and(|count| count > 0),
            remote: self
                .remote_trailers
                .iter()
                .filter_map(|trailer| trailer.url.as_deref())
                .find(|url| super::is_absolute(url))
                .map(str::to_string),
        }
    }

    pub fn directors(&self) -> Vec<String> {
        self.people
            .iter()
            .filter(|p| p.kind.as_deref() == Some("Director"))
            .map(|p| p.name.clone())
            .collect()
    }

    pub fn cast(&self) -> Vec<CastMember> {
        self.people
            .iter()
            .filter(|p| p.kind.as_deref() == Some("Actor"))
            .map(|p| CastMember {
                id: ItemId(p.id.clone()),
                name: p.name.clone(),
                role: p.role.clone().filter(|r| !r.is_empty()),
                photo: p.primary_image_tag.as_ref().map(|tag| ImageRef {
                    item: ItemId(p.id.clone()),
                    kind: ImageKind::Primary,
                    tag: tag.clone(),
                }),
            })
            .collect()
    }
}

/// What a person took part in, from the items they are credited on: films, series, and
/// episodes counted as their series (once each; the series' own entry wins, it knows its year).
/// Newest year first, then A to Z whatever the case, titles with no year last.
pub fn person_titles(items: &[Item]) -> PersonTitles {
    let mut films: Vec<Film> = Vec::new();
    let mut series: Vec<SeriesHit> = Vec::new();
    for item in items {
        match item.kind.as_deref() {
            Some("Series") => {
                match series.iter_mut().find(|s| s.id.0 == item.id) {
                    Some(seen) => *seen = item.series_hit(),
                    None => series.push(item.series_hit()),
                };
            }
            Some("Episode") => {
                if let Some(hit) = item.series_of_episode()
                    && !series.iter().any(|s| s.id == hit.id)
                {
                    series.push(hit);
                }
            }
            _ => films.push(item.film()),
        }
    }
    films.sort_by_cached_key(|f| title_order(f.year, &f.title));
    series.sort_by_cached_key(|s| title_order(s.year, &s.title));
    PersonTitles { films, series }
}

fn title_order(year: Option<i32>, title: &str) -> (bool, std::cmp::Reverse<i32>, String) {
    let undated = year.is_none();
    (
        undated,
        std::cmp::Reverse(year.unwrap_or_default()),
        title.to_lowercase(),
    )
}

fn range(kind: Option<&str>) -> VideoRange {
    match kind {
        Some("SDR") => VideoRange::Sdr,
        Some("HDR10") => VideoRange::Hdr10,
        Some("HDR10Plus") => VideoRange::Hdr10Plus,
        Some("HLG") => VideoRange::Hlg,
        Some(dovi) if dovi.starts_with("DOVI") => VideoRange::DolbyVision,
        _ => VideoRange::Unknown,
    }
}

impl MediaSourceInfo {
    pub fn into_source(self) -> MediaSource {
        let streams =
            |kind: &'static str| self.media_streams.iter().filter(move |s| s.kind == kind);
        let video = streams("Video").next().map(|s| VideoTrack {
            codec: s.codec.clone(),
            width: s.width,
            height: s.height,
            range: range(s.video_range_type.as_deref()),
        });
        let audio = streams("Audio")
            .map(|s| AudioTrack {
                index: s.index,
                language: s.language.clone(),
                codec: s.codec.clone(),
                channels: s.channels,
                title: s.title.clone().or_else(|| s.display_title.clone()),
                default: s.is_default,
            })
            .collect();
        let subtitles = streams("Subtitle")
            .map(|s| SubtitleTrack {
                index: s.index,
                language: s.language.clone(),
                codec: s.codec.clone(),
                title: s.title.clone().or_else(|| s.display_title.clone()),
                forced: s.is_forced,
                default: s.is_default,
                external: s.is_external,
            })
            .collect();
        MediaSource {
            id: MediaSourceId(self.id),
            name: self.name.unwrap_or_default(),
            container: self.container,
            video,
            audio,
            subtitles,
            default_audio: self.default_audio_stream_index,
            default_subtitle: self
                .default_subtitle_stream_index
                .map(|index| (index >= 0).then_some(index)),
        }
    }
}
