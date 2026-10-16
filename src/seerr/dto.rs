//! Seerr's JSON, as much of it as Bobine reads, and its translation into the port's types.

use crate::requests::{
    Availability, CastPerson, CollectionParts, CollectionRef, Discovered, MediaKind, RequestStatus,
    SeasonOffer, TitleDetail, TmdbImage, year_of,
};
use serde::Deserialize;
use std::time::Duration;

#[derive(Deserialize)]
pub struct Status {
    pub version: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: u32,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub jellyfin_user_id: Option<String>,
    #[serde(default)]
    pub jellyfin_username: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

impl User {
    /// What Seerr shows of the user: the display name it works out, else what it is made of.
    pub fn name(self) -> String {
        [
            self.display_name,
            self.jellyfin_username,
            self.username,
            self.email,
        ]
        .into_iter()
        .flatten()
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| format!("user {}", self.id))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub results: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Users {
    pub page_info: PageInfo,
    pub results: Vec<User>,
}

#[derive(Deserialize)]
pub struct Results {
    pub results: Vec<MediaResult>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct SeasonStatus {
    pub season_number: u32,
    pub status: u8,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaRequestDto {
    pub status: u8,
    pub seasons: Vec<SeasonStatus>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaInfo {
    pub status: u8,
    pub seasons: Vec<SeasonStatus>,
    pub requests: Vec<MediaRequestDto>,
}

/// Seerr's media status: 1 unknown, 2 pending, 3 processing, 4 partially available, 5
/// available, 6 blocklisted, 7 deleted (as good as unknown: it may be asked for again).
fn availability(status: u8) -> Availability {
    match status {
        2 => Availability::Pending,
        3 => Availability::Processing,
        4 => Availability::PartiallyAvailable,
        5 => Availability::Available,
        6 => Availability::Blocked,
        _ => Availability::Missing,
    }
}

/// Seerr's request status: 1 pending, 2 approved, 3 declined, 4 failed, 5 completed.
pub fn request_status(status: u8) -> RequestStatus {
    match status {
        1 => RequestStatus::Pending,
        3 => RequestStatus::Declined,
        4 => RequestStatus::Failed,
        5 => RequestStatus::Completed,
        _ => RequestStatus::Approved,
    }
}

/// A film, a series or a person, as a list of results holds them.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaResult {
    pub id: u64,
    pub media_type: Option<String>,
    pub title: Option<String>,
    pub name: Option<String>,
    pub release_date: Option<String>,
    pub first_air_date: Option<String>,
    pub overview: Option<String>,
    pub vote_average: Option<f32>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub media_info: Option<MediaInfo>,
}

fn image(path: Option<String>) -> Option<TmdbImage> {
    path.filter(|path| !path.is_empty()).map(TmdbImage)
}

impl MediaResult {
    /// A film or a series; `None` for a person. `kind` is for a list that does not say (a
    /// collection holds films only).
    pub fn discovered(self, kind: Option<MediaKind>) -> Option<Discovered> {
        let kind = match self.media_type.as_deref() {
            Some("movie") => MediaKind::Movie,
            Some("tv") => MediaKind::Tv,
            Some(_) => return None,
            None => kind?,
        };
        let date = match kind {
            MediaKind::Movie => self.release_date,
            MediaKind::Tv => self.first_air_date,
        };
        Some(Discovered {
            kind,
            tmdb: self.id,
            title: self.title.or(self.name).unwrap_or_default(),
            year: date.as_deref().and_then(year_of),
            rating: self.vote_average.filter(|rating| *rating > 0.),
            overview: self.overview.filter(|text| !text.is_empty()),
            poster: image(self.poster_path),
            backdrop: image(self.backdrop_path),
            availability: availability(self.media_info.map_or(1, |info| info.status)),
        })
    }
}

#[derive(Deserialize)]
pub struct Genre {
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cast {
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Credits {
    pub cast: Vec<Cast>,
}

#[derive(Deserialize)]
pub struct CollectionLink {
    pub id: u64,
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TvSeason {
    pub season_number: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub episode_count: u32,
}

/// `/movie/{id}` and `/tv/{id}`: a result, with more.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    #[serde(flatten)]
    pub result: MediaResult,
    #[serde(default)]
    pub genres: Vec<Genre>,
    pub runtime: Option<u64>,
    #[serde(default)]
    pub credits: Credits,
    pub collection: Option<CollectionLink>,
    #[serde(default)]
    pub seasons: Vec<TvSeason>,
}

/// The most of the cast a page shows.
const CAST: usize = 20;

impl Details {
    /// None when the answer is not a film or a series.
    pub fn detail(mut self, kind: MediaKind) -> Option<TitleDetail> {
        let info = self.result.media_info.take().unwrap_or_default();
        // What is had or on its way, season by season: pending, processing, partly available
        // or available; asked for by a request still pending or approved. A request declined,
        // failed or done asks for nothing more (what a done one brought is the media's status).
        let taken = |number: u32| {
            info.seasons
                .iter()
                .any(|s| s.season_number == number && (2..=5).contains(&s.status))
                || info
                    .requests
                    .iter()
                    .filter(|r| matches!(r.status, 1 | 2))
                    .any(|r| r.seasons.iter().any(|s| s.season_number == number))
        };
        let seasons = self
            .seasons
            .into_iter()
            .filter(|season| season.season_number > 0)
            .map(|season| SeasonOffer {
                number: season.season_number,
                taken: taken(season.season_number),
                name: if season.name.is_empty() {
                    format!("Season {}", season.season_number)
                } else {
                    season.name
                },
                episodes: season.episode_count,
            })
            .collect();
        self.result.media_info = Some(MediaInfo {
            status: info.status,
            ..MediaInfo::default()
        });
        let title = self.result.discovered(Some(kind))?;
        Some(TitleDetail {
            title,
            genres: self.genres.into_iter().map(|genre| genre.name).collect(),
            runtime: self
                .runtime
                .filter(|minutes| *minutes > 0)
                .map(|minutes| Duration::from_secs(minutes * 60)),
            cast: self
                .credits
                .cast
                .into_iter()
                .take(CAST)
                .map(|cast| CastPerson {
                    name: cast.name,
                    character: cast.character.filter(|c| !c.is_empty()),
                    photo: image(cast.profile_path),
                })
                .collect(),
            collection: self.collection.map(|link| CollectionRef {
                id: link.id,
                name: link.name,
            }),
            seasons,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: u64,
    pub name: String,
    pub overview: Option<String>,
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub parts: Vec<MediaResult>,
}

impl Collection {
    pub fn parts(self) -> CollectionParts {
        let mut films: Vec<Discovered> = self
            .parts
            .into_iter()
            .filter_map(|part| part.discovered(Some(MediaKind::Movie)))
            .collect();
        // A film not out yet has no date: it goes last.
        films.sort_by_key(|film| (film.year.is_none(), film.year));
        CollectionParts {
            id: self.id,
            name: self.name,
            overview: self.overview.filter(|text| !text.is_empty()),
            backdrop: image(self.backdrop_path),
            films,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestedMedia {
    pub tmdb_id: u64,
    pub media_type: String,
}

#[derive(Deserialize)]
pub struct Request {
    pub status: u8,
    pub media: Option<RequestedMedia>,
}

#[derive(Deserialize)]
pub struct Requests {
    pub results: Vec<Request>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct QuotaPart {
    pub restricted: bool,
}

#[derive(Deserialize)]
pub struct Quota {
    #[serde(default)]
    pub movie: QuotaPart,
    #[serde(default)]
    pub tv: QuotaPart,
}

/// What an error answer says: the quota's is told apart by it.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Message {
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_media_status_code_has_its_availability() {
        let codes: Vec<Availability> = (1..=7).map(availability).collect();
        assert_eq!(
            codes,
            [
                Availability::Missing,
                Availability::Pending,
                Availability::Processing,
                Availability::PartiallyAvailable,
                Availability::Available,
                Availability::Blocked,
                // Deleted: it may be asked for again.
                Availability::Missing,
            ]
        );
    }

    #[test]
    fn each_request_status_code_has_its_status() {
        let codes: Vec<RequestStatus> = (1..=5).map(request_status).collect();
        assert_eq!(
            codes,
            [
                RequestStatus::Pending,
                RequestStatus::Approved,
                RequestStatus::Declined,
                RequestStatus::Failed,
                RequestStatus::Completed,
            ]
        );
    }

    /// Whether each of seven seasons is taken: seasons 1 to 6 by the media at the statuses 2 to
    /// 7, season 7 by a request at `request`.
    fn seasons_taken(request: u8) -> Vec<bool> {
        let details: Details = serde_json::from_value(serde_json::json!({
            "id": 1,
            "name": "Invented",
            "seasons": (1..=7).map(|n| serde_json::json!({ "seasonNumber": n })).collect::<Vec<_>>(),
            "mediaInfo": {
                "status": 4,
                "seasons": (1..=6).map(|n| serde_json::json!({ "seasonNumber": n, "status": n + 1 })).collect::<Vec<_>>(),
                "requests": [{ "status": request, "seasons": [{ "seasonNumber": 7 }] }],
            },
        }))
        .unwrap();
        let detail = details.detail(MediaKind::Tv).unwrap();
        detail.seasons.iter().map(|s| s.taken).collect()
    }

    #[test]
    fn a_season_is_taken_only_pending_processing_partly_available_or_available() {
        // Seasons 1 to 6 at the media statuses 2 to 7: blocklisted and deleted are not taken.
        assert_eq!(
            seasons_taken(1)[..6],
            [true, true, true, true, false, false]
        );
        // Season 7, by its request: pending and approved take it, the others do not.
        let by_request: Vec<bool> = (1..=5).map(|code| seasons_taken(code)[6]).collect();
        assert_eq!(by_request, [true, true, false, false, false]);
    }
}
