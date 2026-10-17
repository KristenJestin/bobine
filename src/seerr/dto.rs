//! Seerr's JSON, as much of it as Bobine reads, and its translation into the port's types.

use crate::requests::{
    Availability, CastPerson, CollectionParts, CollectionRef, Discovered, Download, MediaKind,
    RequestStatus, SeasonOffer, TitleDetail, TmdbImage, year_of,
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

/// An entry of `downloadStatus`: Seerr's `DownloadingItem`, from Radarr's or Sonarr's queue.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DownloadDto {
    pub download_id: Option<String>,
    pub size: Option<f64>,
    pub size_left: Option<f64>,
    /// A .NET time span: `00:12:34`, `1.02:03:04`.
    pub time_left: Option<String>,
    pub episode: Option<EpisodeDto>,
}

/// The episode of a series' queue entry.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct EpisodeDto {
    pub season_number: Option<u32>,
    pub episode_number: Option<u32>,
}

/// The time a queue says is left, `hh:mm:ss` with days before a dot (`d.hh:mm:ss`) and
/// fractions of a second after the seconds; none for what is not a time span.
fn time_left(text: &str) -> Option<Duration> {
    let (days, clock) = match text.split_once('.') {
        Some((days, rest)) if !days.contains(':') => (days.parse::<u64>().ok()?, rest),
        _ => (0, text),
    };
    let mut parts = clock.split(':');
    let hours: u64 = parts.next()?.parse().ok()?;
    let minutes: u64 = parts.next()?.parse().ok()?;
    let seconds: u64 = parts.next()?.split('.').next()?.parse().ok()?;
    if parts.next().is_some() || minutes > 59 || seconds > 59 {
        return None;
    }
    // Digits go through `u64`: a text past its range is no time, not a panic.
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(hours.checked_mul(3600)?)?
        .checked_add(minutes * 60 + seconds)?;
    Some(Duration::from_secs(seconds))
}

impl DownloadDto {
    fn download(self, ultra_hd: bool) -> Download {
        let episode = self
            .episode
            .and_then(|episode| Some((episode.season_number?, episode.episode_number?)));
        Download {
            id: self.download_id.filter(|id| !id.is_empty()),
            size: bytes(self.size),
            size_left: bytes(self.size_left),
            time_left: self.time_left.as_deref().and_then(time_left),
            episode,
            ultra_hd,
        }
    }
}

/// A size in bytes as JSON numbers come, whole or not; nothing for a missing or negative one.
fn bytes(size: Option<f64>) -> u64 {
    size.filter(|size| *size > 0.).map_or(0, |size| size as u64)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MediaInfo {
    pub status: u8,
    pub seasons: Vec<SeasonStatus>,
    pub requests: Vec<MediaRequestDto>,
    pub download_status: Vec<DownloadDto>,
    pub download_status4k: Vec<DownloadDto>,
}

impl MediaInfo {
    /// What is downloading for the title, in HD and in 4K.
    pub fn downloads(self) -> Vec<Download> {
        let hd = self.download_status.into_iter().map(|d| d.download(false));
        let ultra = self.download_status4k.into_iter().map(|d| d.download(true));
        hd.chain(ultra).collect()
    }
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

/// A video Seerr lists for a title (`relatedVideos`). Seerr keeps of TMDb's only these: not the
/// language, the date nor whether it is official.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RelatedVideo {
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub site: String,
    pub key: String,
}

impl RelatedVideo {
    /// The page the player is given, built from the site and the key: the address Seerr sends
    /// along is never used, so that only these two hosts can ever reach `yt-dlp`.
    fn address(&self) -> Option<String> {
        let host = match self.site.as_str() {
            "YouTube" => "https://www.youtube.com/watch?v=",
            "Vimeo" => "https://vimeo.com/",
            _ => return None,
        };
        let plain = !self.key.is_empty()
            && (self.key.chars()).all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        plain.then(|| format!("{host}{}", self.key))
    }

    /// "Official Trailer", but not "Unofficial Trailer".
    fn official(&self) -> bool {
        (self.name.split(|c: char| !c.is_alphanumeric()))
            .any(|w| w.eq_ignore_ascii_case("official"))
    }
}

/// The trailer a request page plays: among the videos of type Trailer on a site `yt-dlp` plays,
/// an official one, else the first one listed. The user's language is not preferred: Seerr drops
/// each video's `iso_639_1`, so the order kept is TMDb's, as Seerr returns it.
fn trailer(videos: Vec<RelatedVideo>) -> Option<String> {
    let trailers: Vec<(bool, String)> = videos
        .iter()
        .filter(|video| video.kind == "Trailer")
        .filter_map(|video| Some((video.official(), video.address()?)))
        .collect();
    let official = trailers.iter().find(|(official, _)| *official);
    official
        .or(trailers.first())
        .map(|(_, address)| address.clone())
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
    #[serde(default)]
    pub related_videos: Vec<RelatedVideo>,
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
        let downloads = info.downloads();
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
            trailer: trailer(self.related_videos),
            downloads,
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

    fn video(kind: &str, name: &str, site: &str, key: &str) -> RelatedVideo {
        RelatedVideo {
            kind: kind.into(),
            name: name.into(),
            site: site.into(),
            key: key.into(),
        }
    }

    #[test]
    fn the_official_trailer_is_chosen_over_an_unofficial_one() {
        let videos = vec![
            video("Trailer", "Trailer", "YouTube", "plain001"),
            video("Trailer", "Unofficial Trailer", "YouTube", "unoff001"),
            video("Trailer", "Official Trailer #2", "YouTube", "offic002"),
            video("Trailer", "Official Trailer", "YouTube", "offic001"),
        ];
        assert_eq!(
            trailer(videos).as_deref(),
            Some("https://www.youtube.com/watch?v=offic002"),
            "the first official one, as Seerr lists them; 'unofficial' is no 'official'"
        );
    }

    #[test]
    fn without_an_official_trailer_the_first_one_listed_is_chosen() {
        let videos = vec![
            video("Trailer", "Trailer", "YouTube", "first001"),
            video("Trailer", "Trailer", "YouTube", "second01"),
        ];
        assert_eq!(
            trailer(videos).as_deref(),
            Some("https://www.youtube.com/watch?v=first001")
        );
    }

    #[test]
    fn a_key_that_is_not_a_plain_id_is_left_out_so_that_no_other_address_reaches_the_player() {
        let videos = vec![
            video("Trailer", "Official Trailer", "YouTube", "x&ytdl=--exec"),
            video("Trailer", "Official Trailer", "YouTube", ""),
            video("Trailer", "Trailer", "Vimeo", "12/../34"),
            video("Trailer", "Trailer", "YouTube", "good_id-1"),
        ];
        assert_eq!(
            trailer(videos).as_deref(),
            Some("https://www.youtube.com/watch?v=good_id-1")
        );
    }

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

    #[test]
    fn a_queue_s_time_left_is_read_as_a_time_span() {
        let secs = |text| time_left(text).map(|time| time.as_secs());
        assert_eq!(secs("00:02:00"), Some(120));
        assert_eq!(secs("01:20:05"), Some(4805));
        assert_eq!(secs("1.02:03:04"), Some(86_400 + 2 * 3600 + 3 * 60 + 4));
        assert_eq!(secs("00:00:12.3456789"), Some(12));
        // Past what a `u64` of seconds holds: no time.
        for too_long in ["999999999999999999.00:00:00", "18446744073709551615:00:00"] {
            assert_eq!(secs(too_long), None, "{too_long:?}");
        }
        for not_a_time in ["", "soon", "12:30", "aa:bb:cc", "00:61:00", "-00:01:00"] {
            assert_eq!(secs(not_a_time), None, "{not_a_time:?}");
        }
    }

    /// A series' page whose media info is `info`.
    fn downloads_of(info: serde_json::Value) -> Vec<Download> {
        let details: Details = serde_json::from_value(serde_json::json!({
            "id": 1,
            "name": "Invented",
            "mediaInfo": info,
        }))
        .unwrap();
        details.detail(MediaKind::Tv).unwrap().downloads
    }

    #[test]
    fn the_download_status_of_a_media_info_is_read_with_the_4k_one_after_it() {
        let downloads = downloads_of(serde_json::json!({
            "status": 3,
            "downloadStatus": [
                { "mediaType": "tv", "externalId": 7, "size": 5000, "sizeLeft": 2000,
                  "status": "downloading", "timeLeft": "00:12:34", "title": "Invented.S01",
                  "downloadId": "ABC123", "estimatedCompletionTime": "2026-10-17T20:00:00Z",
                  "episode": { "seasonNumber": 1, "episodeNumber": 2, "absoluteEpisodeNumber": 2, "id": 9 } },
            ],
            "downloadStatus4k": [
                { "mediaType": "tv", "externalId": 8, "size": 9000.0, "sizeLeft": 9000.0,
                  "status": "queued", "title": "Invented.4K" },
            ],
        }));
        assert_eq!(
            downloads,
            [
                Download {
                    id: Some("ABC123".into()),
                    size: 5000,
                    size_left: 2000,
                    time_left: Some(Duration::from_secs(12 * 60 + 34)),
                    episode: Some((1, 2)),
                    ultra_hd: false,
                },
                Download {
                    id: None,
                    size: 9000,
                    size_left: 9000,
                    time_left: None,
                    episode: None,
                    ultra_hd: true,
                },
            ]
        );
    }

    #[test]
    fn a_media_info_without_a_download_status_downloads_nothing() {
        assert!(downloads_of(serde_json::json!({ "status": 3 })).is_empty());
        assert!(
            downloads_of(serde_json::json!({
                "status": 3, "downloadStatus": [], "downloadStatus4k": []
            }))
            .is_empty()
        );
    }

    #[test]
    fn an_entry_with_null_fields_is_read_not_an_error() {
        let downloads = downloads_of(serde_json::json!({
            "status": 3,
            "downloadStatus": [{ "size": null, "sizeLeft": null, "timeLeft": null, "downloadId": null }],
        }));
        assert_eq!(
            downloads,
            [Download {
                id: None,
                size: 0,
                size_left: 0,
                time_left: None,
                episode: None,
                ultra_hd: false,
            }]
        );
    }
}
