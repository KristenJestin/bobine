//! Jellyfin's JSON, as much of it as Bobine reads, and its translation into the port's types.

use crate::provider::{
    AudioTrack, CastMember, CollectionCard, Episode, Feature, FeatureKind, Film, ImageKind,
    ImageRef, ItemId, Library, LibraryKind, MediaSource, MediaSourceId, PersonHit, PersonTitles,
    Season, Series, SeriesHit, ServerInfo, SubtitleTrack, Title, VideoRange, VideoTrack,
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
    /// A season's or an episode's number.
    pub index_number: Option<u32>,
    /// An episode's season number.
    pub parent_index_number: Option<u32>,
    pub season_id: Option<String>,
    /// A collection's films, when asked for.
    pub child_count: Option<u32>,
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
            poster: film.poster,
            backdrop: film.backdrop,
        }
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

    /// What a row of the Home lists: an episode, a series, else a film.
    pub fn title(self) -> Title {
        match self.kind.as_deref() {
            Some("Episode") => Title::Episode(self.episode()),
            Some("Series") => Title::Series(self.series()),
            _ => Title::Film(self.film()),
        }
    }

    /// What a library's latest list holds: `/Items/Latest` groups the episodes of a series only
    /// when several came in, so a lone one comes as the episode, and is listed as its series.
    pub fn latest(self) -> Title {
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
        Title::Series(Series {
            id: ItemId(id),
            title: self.series_name.unwrap_or_default(),
            year: None,
            end_year: None,
            community_rating: None,
            unwatched: 0,
            poster,
            backdrop: None,
        })
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
            logo: self.image_tags.get("Logo").map(|tag| ImageRef {
                item: ItemId(self.id.clone()),
                kind: ImageKind::Logo,
                tag: tag.clone(),
            }),
            resume: film.resume.filter(|_| !series),
        }
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
        })
    }

    pub fn person_hit(&self) -> PersonHit {
        PersonHit {
            id: ItemId(self.id.clone()),
            name: self.name.clone(),
            photo: self.primary(),
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
        }
    }
}
