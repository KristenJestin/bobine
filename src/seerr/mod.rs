//! The Seerr implementation of the request port (Jellyseerr and Overseerr merged into Seerr, same
//! `/api/v1`). The API key goes in the `X-Api-Key` header and acts as its owner; a request made
//! for the signed-in viewer also names their Seerr user in `X-Api-User`, found by their Jellyfin
//! id. TMDb's pictures are fetched from TMDb, without the key.

mod dto;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::json;
use ureq::http::Response;

use crate::jellyfin::http::{encode, normalize_address, transport};
use crate::provider::{ProviderError, Token};
use crate::requests::{
    ActingAs, Ask, CollectionParts, DiscoverRow, Discovered, MediaKind, MyRequest, RequestError,
    RequestServer, RequestStatus, ServerCheck, TitleDetail, TmdbId, TmdbImage,
};

const TIMEOUT: Duration = Duration::from_secs(15);
const IMAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// Where TMDb serves its pictures.
const TMDB_IMAGES: &str = "https://image.tmdb.org";
/// How many users are read at a time while looking for the viewer.
const USERS_PAGE: u32 = 100;
/// The most of the viewer's requests shown, each read with its title.
const MY_REQUESTS: u32 = 20;
/// How many of their titles are read at once.
const TITLE_READERS: usize = 4;

/// Whom the requests go as, once found.
#[derive(Clone)]
struct Acting {
    id: u32,
    name: String,
    viewer: bool,
}

/// A Seerr server, its key, and the Jellyfin user the requests are made for.
pub struct SeerrClient {
    agent: ureq::Agent,
    /// The address, `/api/v1` included.
    api: String,
    images: String,
    key: Token,
    viewer: String,
    /// Found at the first call that needs it, then kept.
    acting: Mutex<Option<Acting>>,
}

/// Neither the key nor the address: what the client is, not what it holds.
impl fmt::Debug for SeerrClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SeerrClient(<redacted>)")
    }
}

fn unreachable(what: &str, e: ureq::Error) -> RequestError {
    match transport(what, e) {
        ProviderError::Unreachable(why) => RequestError::Unreachable(why),
        other => RequestError::Unreachable(other.to_string()),
    }
}

/// An error answer. Seerr answers 403 to a key it does not know; to a request, a 403 is the
/// quota when its message says so ("Movie Quota exceeded."), else a right the viewer lacks.
fn refusal(code: u16, posted: bool, mut response: Response<ureq::Body>) -> RequestError {
    match code {
        401 => RequestError::KeyRefused,
        403 if !posted => RequestError::KeyRefused,
        403 => {
            let said: dto::Message = response.body_mut().read_json().unwrap_or_default();
            if said.message.to_lowercase().contains("quota") {
                RequestError::Quota
            } else {
                RequestError::Forbidden
            }
        }
        300..=399 => RequestError::Redirected,
        404 => RequestError::NotFound,
        409 => RequestError::AlreadyRequested,
        _ => RequestError::Server(code),
    }
}

/// The TMDb size for a picture drawn `width` pixels wide: the smallest at least as wide.
fn tmdb_size(width: u32) -> &'static str {
    match width {
        0..=342 => "w342",
        343..=500 => "w500",
        501..=780 => "w780",
        781..=1280 => "w1280",
        _ => "original",
    }
}

/// A TMDb path is a file name under the size: nothing that leads elsewhere.
fn tmdb_path(image: &TmdbImage) -> Option<&str> {
    let path = image.0.strip_prefix('/')?;
    let safe = |c: char| c.is_ascii_alphanumeric() || "._-".contains(c);
    (!path.is_empty() && path.chars().all(safe)).then_some(&image.0)
}

impl SeerrClient {
    /// A client of the Seerr server at `address`, for the Jellyfin user `viewer`.
    pub fn new(address: &str, key: Token, viewer: &str) -> Result<Self, RequestError> {
        Self::with_images(address, key, viewer, TMDB_IMAGES.to_string())
    }

    /// The same, with TMDb's pictures from `images` (a test's local server).
    pub fn with_images(
        address: &str,
        key: Token,
        viewer: &str,
        images: String,
    ) -> Result<Self, RequestError> {
        let address = normalize_address(address).map_err(|_| RequestError::InvalidAddress)?;
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            // ureq would send the key on to wherever a redirect points.
            .max_redirects(0)
            .user_agent(format!("Bobine/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Ok(Self {
            agent,
            api: format!("{address}/api/v1"),
            images,
            key,
            viewer: viewer.to_string(),
            acting: Mutex::new(None),
        })
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("{}{path}", self.api);
        for (i, (key, value)) in query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(key);
            url.push('=');
            url.push_str(&encode(value));
        }
        url
    }

    /// `posted`: the call asks for something, rather than reads.
    fn checked(
        what: &str,
        posted: bool,
        sent: Result<Response<ureq::Body>, ureq::Error>,
    ) -> Result<Response<ureq::Body>, RequestError> {
        let response = sent.map_err(|e| unreachable(what, e))?;
        let code = response.status().as_u16();
        if (200..300).contains(&code) {
            Ok(response)
        } else {
            Err(refusal(code, posted, response))
        }
    }

    fn json<T: DeserializeOwned>(
        what: &str,
        mut response: Response<ureq::Body>,
    ) -> Result<T, RequestError> {
        response
            .body_mut()
            .read_json()
            .map_err(|_| RequestError::BadAnswer(format!("{what}: not the expected JSON")))
    }

    /// A call with the key; `as_user` names the Seerr user it is made for.
    fn get<T: DeserializeOwned>(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
        as_user: Option<u32>,
    ) -> Result<T, RequestError> {
        let mut request = self
            .agent
            .get(&self.url(path, query))
            .header("X-Api-Key", self.key.expose_secret());
        if let Some(user) = as_user {
            request = request.header("X-Api-User", &user.to_string());
        }
        Self::json(what, Self::checked(what, false, request.call())?)
    }

    /// Who the requests go as: the Seerr user with the viewer's Jellyfin id, else the key's owner.
    /// Users are listed with the key alone, page after page.
    fn acting(&self) -> Result<Acting, RequestError> {
        if let Some(acting) = self.acting.lock().unwrap().clone() {
            return Ok(acting);
        }
        let wanted = self.viewer.replace('-', "").to_lowercase();
        let mut skip = 0;
        let found = loop {
            let users: dto::Users = self.get(
                "users",
                "/user",
                &[("take", USERS_PAGE.to_string()), ("skip", skip.to_string())],
                None,
            )?;
            let count = users.results.len() as u32;
            let viewer = users.results.into_iter().find(|user| {
                user.jellyfin_user_id
                    .as_deref()
                    .is_some_and(|id| id.replace('-', "").to_lowercase() == wanted)
            });
            if let Some(user) = viewer {
                break Some(user);
            }
            skip += count;
            if count == 0 || skip >= users.page_info.results {
                break None;
            }
        };
        let acting = match found {
            Some(user) => Acting {
                id: user.id,
                name: user.name(),
                viewer: true,
            },
            None => {
                let owner: dto::User = self.get("the key's owner", "/auth/me", &[], None)?;
                Acting {
                    id: owner.id,
                    name: owner.name(),
                    viewer: false,
                }
            }
        };
        *self.acting.lock().unwrap() = Some(acting.clone());
        Ok(acting)
    }

    /// The header naming the viewer, when the requests go as them.
    fn as_viewer(&self) -> Result<Option<u32>, RequestError> {
        let acting = self.acting()?;
        Ok(acting.viewer.then_some(acting.id))
    }

    fn results(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<Discovered>, RequestError> {
        let page: dto::Results = self.get(what, path, query, None)?;
        Ok(page
            .results
            .into_iter()
            .filter_map(|result| result.discovered(None))
            .collect())
    }
}

fn kind_path(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Movie => "movie",
        MediaKind::Tv => "tv",
    }
}

impl RequestServer for SeerrClient {
    fn check(&self) -> Result<ServerCheck, RequestError> {
        // Without the key: only a server that answers as Seerr goes further.
        let sent = self.agent.get(&self.url("/status", &[])).call();
        let status: dto::Status = Self::checked("status", false, sent)
            .and_then(|response| Self::json("status", response))
            .map_err(|e| match e {
                RequestError::Unreachable(_) | RequestError::Redirected => e,
                _ => RequestError::NotARequestServer,
            })?;
        let _: dto::User = self.get("the key's owner", "/auth/me", &[], None)?;
        let acting = self.acting()?;
        Ok(ServerCheck {
            version: status.version,
            acting_as: if acting.viewer {
                ActingAs::Viewer(acting.name)
            } else {
                ActingAs::Owner(acting.name)
            },
        })
    }

    fn discover(&self, row: DiscoverRow) -> Result<Vec<Discovered>, RequestError> {
        let (path, kind) = match row {
            DiscoverRow::Trending => ("/discover/trending", None),
            DiscoverRow::PopularMovies => ("/discover/movies", Some(MediaKind::Movie)),
            DiscoverRow::PopularSeries => ("/discover/tv", Some(MediaKind::Tv)),
            DiscoverRow::Upcoming => ("/discover/movies/upcoming", Some(MediaKind::Movie)),
        };
        let page: dto::Results = self.get("discover", path, &[("page", "1".into())], None)?;
        Ok(page
            .results
            .into_iter()
            .filter_map(|result| result.discovered(kind))
            .collect())
    }

    fn search(&self, query: &str) -> Result<Vec<Discovered>, RequestError> {
        self.results(
            "search",
            "/search",
            &[("query", query.to_string()), ("page", "1".into())],
        )
    }

    fn title(&self, kind: MediaKind, id: TmdbId) -> Result<TitleDetail, RequestError> {
        let details: dto::Details =
            self.get("title", &format!("/{}/{id}", kind_path(kind)), &[], None)?;
        details
            .detail(kind)
            .ok_or_else(|| RequestError::BadAnswer("title: not a movie or a series".into()))
    }

    fn recommendations(
        &self,
        kind: MediaKind,
        id: TmdbId,
    ) -> Result<Vec<Discovered>, RequestError> {
        let page: dto::Results = self.get(
            "recommendations",
            &format!("/{}/{id}/recommendations", kind_path(kind)),
            &[("page", "1".into())],
            None,
        )?;
        Ok(page
            .results
            .into_iter()
            .filter_map(|result| result.discovered(Some(kind)))
            .collect())
    }

    fn collection(&self, id: TmdbId) -> Result<CollectionParts, RequestError> {
        let collection: dto::Collection =
            self.get("collection", &format!("/collection/{id}"), &[], None)?;
        Ok(collection.parts())
    }

    fn my_requests(&self) -> Result<Vec<MyRequest>, RequestError> {
        let acting = self.acting()?;
        let requests: dto::Requests = self.get(
            "requests",
            "/request",
            &[
                ("take", MY_REQUESTS.to_string()),
                ("skip", "0".into()),
                ("sort", "added".into()),
                ("requestedBy", acting.id.to_string()),
            ],
            acting.viewer.then_some(acting.id),
        )?;
        // A request names its title by id only: each is read, as Seerr's own page does, a few
        // at a time. One that cannot be read is left out, not the whole list.
        let wanted: Vec<(MediaKind, TmdbId, RequestStatus)> = requests
            .results
            .into_iter()
            .filter_map(|request| {
                let media = request.media?;
                let kind = match media.media_type.as_str() {
                    "tv" => MediaKind::Tv,
                    _ => MediaKind::Movie,
                };
                Some((kind, media.tmdb_id, dto::request_status(request.status)))
            })
            .collect();
        let read: Vec<Mutex<Option<MyRequest>>> = wanted.iter().map(|_| Mutex::new(None)).collect();
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..TITLE_READERS.min(wanted.len()) {
                scope.spawn(|| {
                    loop {
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&(kind, id, status)) = wanted.get(at) else {
                            break;
                        };
                        if let Ok(detail) = self.title(kind, id) {
                            *read[at].lock().unwrap() = Some(MyRequest {
                                title: detail.title,
                                status,
                            });
                        }
                    }
                });
            }
        });
        Ok(read
            .into_iter()
            .filter_map(|slot| slot.into_inner().unwrap())
            .collect())
    }

    fn quota_reached(&self, kind: MediaKind) -> Result<bool, RequestError> {
        let acting = self.acting()?;
        let quota: dto::Quota =
            self.get("quota", &format!("/user/{}/quota", acting.id), &[], None)?;
        Ok(match kind {
            MediaKind::Movie => quota.movie.restricted,
            MediaKind::Tv => quota.tv.restricted,
        })
    }

    fn request(&self, ask: &Ask) -> Result<(), RequestError> {
        let body = match ask {
            Ask::Movie(id) => json!({ "mediaType": "movie", "mediaId": id, "is4k": false }),
            Ask::Tv(id, seasons) => {
                json!({ "mediaType": "tv", "mediaId": id, "seasons": seasons, "is4k": false })
            }
        };
        let mut request = self
            .agent
            .post(&self.url("/request", &[]))
            .header("X-Api-Key", self.key.expose_secret());
        if let Some(user) = self.as_viewer()? {
            request = request.header("X-Api-User", &user.to_string());
        }
        Self::checked("request", true, request.send_json(&body)).map(drop)
    }

    fn image(&self, image: &TmdbImage, width: u32) -> Result<Vec<u8>, RequestError> {
        let path = tmdb_path(image).ok_or(RequestError::NotFound)?;
        let url = format!("{}/t/p/{}{path}", self.images, tmdb_size(width));
        let sent = self
            .agent
            .get(&url)
            .config()
            .timeout_global(Some(IMAGE_TIMEOUT))
            // No key goes to TMDb: its redirects may be followed.
            .max_redirects(10)
            .build()
            .call();
        // TMDb, not Seerr: its 403 is no key refused.
        Self::checked("picture", true, sent)?
            .body_mut()
            .with_config()
            .limit(32 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| unreachable("picture", e))
    }
}
