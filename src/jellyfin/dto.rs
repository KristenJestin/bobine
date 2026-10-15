//! Jellyfin's JSON, as much of it as Bobine reads, and its translation into the port's types.

use crate::provider::{
    AudioTrack, CastMember, Film, ImageKind, ImageRef, ItemId, Library, LibraryKind, MediaSource,
    MediaSourceId, ServerInfo, SubtitleTrack, VideoRange, VideoTrack,
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

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
pub struct UserData {
    pub played: bool,
    pub playback_position_ticks: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub collection_type: Option<String>,
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
    #[serde(default)]
    pub people: Vec<Person>,
    #[serde(default)]
    pub media_sources: Vec<MediaSourceInfo>,
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
