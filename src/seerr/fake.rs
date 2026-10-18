//! A fake Seerr server for the tests, on a local port: it answers the routes Bobine uses, with
//! invented titles, users and requests, written from Seerr's API description. It keeps every
//! request it gets, so that a test sees exactly what was sent, and serves TMDb's pictures on the
//! same port.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tiny_http::{Header, Response, Server};

/// The API key the fake server accepts.
pub const KEY: &str = "seerr-k3y-5f0c9a7d2e4b6180";
pub const VERSION: &str = "2.7.3";
/// The Jellyfin user id of the signed-in viewer, as Seerr keeps it for its user.
pub const VIEWER_JELLYFIN_ID: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";
/// The viewer's id on Seerr: far enough down the users to need a second page of them.
pub const VIEWER: u32 = 121;
pub const OWNER: u32 = 1;
pub const PICTURE: &[u8] = b"\xff\xd8\xff fake tmdb picture";

/// The collection of the tests: four films, oldest first.
pub const COLLECTION: u64 = 500;
pub const LIGHTHOUSE: u64 = 9001;
pub const HARBOUR_LIGHTS: u64 = 9002;
pub const HARBOUR_NIGHTS: u64 = 9003;
pub const HARBOUR_DAWN: u64 = 9004;
/// A film awaiting approval.
pub const PAPER_MOONS: u64 = 9005;
/// A series: specials, a first season available, a second one requested, a third one missing.
pub const NORTHERN_LINE: u64 = 7001;
pub const COPPER_COAST: u64 = 7002;

/// A request as the fake server got it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// The query as it came, still encoded.
    pub raw_query: String,
    pub query: Vec<(String, String)>,
    pub api_key: Option<String>,
    pub api_user: Option<String>,
    pub body: String,
}

impl Seen {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

pub struct State {
    pub seen: Vec<Seen>,
    /// Answers every route like a web server that is not Seerr.
    pub not_seerr: bool,
    /// Discover refuses the viewer, the key working elsewhere.
    pub discover_forbidden: bool,
    /// The titles of these TMDb ids cannot be read: their pages answer 500.
    pub failing_titles: Vec<u64>,
    /// Answers every route with a redirect there, as a proxy moved elsewhere would.
    pub redirect_to: Option<String>,
    /// No Seerr user has the viewer's Jellyfin id.
    pub no_viewer: bool,
    /// How many more films the viewer may request; `None`: no quota.
    pub movie_quota: Option<u32>,
    /// The films and series asked for so far, by TMDb id: pending from then on.
    pub requested: Vec<u64>,
    /// What Seerr's download tracker holds for a title, by TMDb id: `downloadStatus` and
    /// `downloadStatus4k` as they are in its media info once the title reached Radarr or Sonarr.
    pub queue: Vec<(u64, Value)>,
}

pub struct FakeSeerr {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FakeSeerr {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(&value[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

/// What Overseerr's validation refuses in a query value: a `+`, or a reserved character left
/// as it is.
fn strictly_encoded(raw: &str) -> bool {
    !raw.split('&')
        .filter_map(|pair| pair.split_once('=').map(|(_, v)| v))
        .any(|value| value.contains(['+', '!', '\'', '(', ')', '*', ' ']))
}

fn movie(id: u64, title: &str, date: &str, status: Option<u8>) -> Value {
    let mut movie = json!({
        "id": id,
        "mediaType": "movie",
        "title": title,
        "releaseDate": date,
        "overview": format!("An invented movie called {title}."),
        "voteAverage": 7.1,
        "posterPath": format!("/poster-{id}.jpg"),
        "backdropPath": format!("/backdrop-{id}.jpg"),
    });
    if let Some(status) = status {
        movie["mediaInfo"] = json!({ "tmdbId": id, "status": status });
    }
    movie
}

fn tv(id: u64, name: &str, date: &str, status: Option<u8>) -> Value {
    let mut tv = json!({
        "id": id,
        "mediaType": "tv",
        "name": name,
        "firstAirDate": date,
        "overview": format!("An invented series called {name}."),
        "voteAverage": 8.2,
        "posterPath": format!("/poster-{id}.jpg"),
        "backdropPath": format!("/backdrop-{id}.jpg"),
    });
    if let Some(status) = status {
        tv["mediaInfo"] = json!({ "tmdbId": id, "status": status });
    }
    tv
}

/// Every title the fake server knows, as a result list gives it.
fn catalogue(state: &State) -> Vec<Value> {
    let mut all = vec![
        movie(LIGHTHOUSE, "The Lighthouse Keeper", "2017-03-10", Some(5)),
        movie(HARBOUR_LIGHTS, "Harbour Lights", "2019-06-21", Some(5)),
        movie(HARBOUR_NIGHTS, "Harbour Nights", "2022-11-04", None),
        movie(HARBOUR_DAWN, "Harbour Dawn", "2024-09-13", Some(1)),
        movie(PAPER_MOONS, "Paper Moons", "2023-02-02", Some(2)),
        movie(9006, "Glass Rivers", "2025-12-19", Some(3)),
        tv(NORTHERN_LINE, "Northern Line", "2020-01-15", Some(4)),
        tv(COPPER_COAST, "Copper Coast", "2024-04-02", None),
    ];
    for title in &mut all {
        let id = title["id"].as_u64().unwrap();
        if state.requested.contains(&id) {
            title["mediaInfo"] = json!({ "tmdbId": id, "status": 2 });
        }
    }
    all
}

fn find(state: &State, id: u64, kind: &str) -> Option<Value> {
    catalogue(state)
        .into_iter()
        .find(|t| t["id"] == id && t["mediaType"] == kind)
}

fn page(results: Vec<Value>) -> Value {
    json!({
        "page": 1,
        "totalPages": 1,
        "totalResults": results.len(),
        "results": results,
    })
}

fn user(id: u32, jellyfin: &str, name: &str) -> Value {
    json!({
        "id": id,
        "displayName": name,
        "jellyfinUserId": jellyfin,
        "jellyfinUsername": name,
        "userType": 3,
    })
}

/// The users, one page of them: the owner first, then many, the viewer far down.
fn users(state: &State, seen: &Seen) -> Value {
    let all: Vec<Value> = (1..=150)
        .map(|id| match id {
            OWNER => user(id, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "Owner"),
            VIEWER if !state.no_viewer => user(id, VIEWER_JELLYFIN_ID, "camille"),
            _ => user(id, &format!("{id:032x}"), &format!("guest {id}")),
        })
        .collect();
    let take: usize = seen.param("take").map_or(10, |t| t.parse().unwrap());
    let skip: usize = seen.param("skip").map_or(0, |s| s.parse().unwrap());
    let results: Vec<Value> = all.iter().skip(skip).take(take).cloned().collect();
    json!({
        "pageInfo": {
            "pages": all.len().div_ceil(take),
            "pageSize": take,
            "results": all.len(),
            "page": skip / take + 1,
        },
        "results": results,
    })
}

fn credits() -> Value {
    json!({
        "cast": [
            { "id": 31, "name": "Ines Varga", "character": "The keeper", "profilePath": "/ines.jpg" },
            { "id": 32, "name": "Tomas Reed", "character": null, "profilePath": null },
        ],
        "crew": [],
    })
}

fn video(kind: &str, name: &str, site: &str, key: &str) -> Value {
    json!({ "type": kind, "name": name, "site": site, "key": key, "size": 1080,
        "url": format!("https://example.invalid/{site}/{key}") })
}

/// The videos Seerr lists for a title, as it maps TMDb's. Seerr gives neither the language, the
/// date nor whether a video is official: only its type, name, site and key.
fn related_videos(id: u64) -> Value {
    match id {
        LIGHTHOUSE => json!([
            video("Featurette", "Making of", "YouTube", "making01"),
            video("Teaser", "Teaser", "YouTube", "teaser01"),
            video("Trailer", "Trailer (fan cut)", "YouTube", "fanCut01"),
            video("Trailer", "Official Trailer", "YouTube", "offTrl01"),
            video("Clip", "Opening scene", "YouTube", "clip0001"),
        ]),
        HARBOUR_LIGHTS => json!([
            video("Teaser", "Teaser", "YouTube", "teaser02"),
            video("Featurette", "Behind the lens", "YouTube", "lens0001"),
            video("Behind the Scenes", "On set", "YouTube", "onset001"),
        ]),
        NORTHERN_LINE => json!([video("Trailer", "Season 3 Trailer", "YouTube", "lineTrl1")]),
        COPPER_COAST => json!([
            video("Trailer", "Trailer", "Dailymotion", "dm0001"),
            video("Trailer", "Trailer", "Vimeo", "12345678"),
        ]),
        _ => json!([]),
    }
}

/// The queue entries of a title go into its media info, as Seerr fills them when it loads it.
fn with_queue(state: &State, id: u64, details: &mut Value) {
    let Some((_, queue)) = state.queue.iter().find(|(queued, _)| *queued == id) else {
        return;
    };
    for (key, entries) in queue.as_object().unwrap() {
        details["mediaInfo"][key] = entries.clone();
    }
}

fn movie_details(state: &State, id: u64) -> Option<Value> {
    let mut details = find(state, id, "movie")?;
    with_queue(state, id, &mut details);
    details["relatedVideos"] = related_videos(id);
    details["runtime"] = json!(112);
    details["genres"] = json!([{ "id": 18, "name": "Drama" }]);
    details["credits"] = credits();
    if (LIGHTHOUSE..=HARBOUR_DAWN).contains(&id) {
        details["collection"] = json!({
            "id": COLLECTION,
            "name": "Harbour Saga",
            "posterPath": "/harbour-saga.jpg",
            "backdropPath": "/harbour-saga-backdrop.jpg",
        });
    }
    Some(details)
}

fn tv_details(state: &State, id: u64) -> Option<Value> {
    let mut details = find(state, id, "tv")?;
    details["relatedVideos"] = related_videos(id);
    details["genres"] = json!([{ "id": 9648, "name": "Mystery" }]);
    details["credits"] = credits();
    details["seasons"] = json!([
        { "id": 1, "seasonNumber": 0, "name": "Specials", "episodeCount": 2 },
        { "id": 2, "seasonNumber": 1, "name": "Season 1", "episodeCount": 8 },
        { "id": 3, "seasonNumber": 2, "name": "Season 2", "episodeCount": 8 },
        { "id": 4, "seasonNumber": 3, "name": "Season 3", "episodeCount": 10 },
    ]);
    if id == NORTHERN_LINE {
        details["mediaInfo"] = json!({
            "tmdbId": id,
            "status": 4,
            "seasons": [{ "seasonNumber": 1, "status": 5 }],
            "requests": [
                { "id": 40, "status": 1, "seasons": [{ "seasonNumber": 2, "status": 1 }] },
                { "id": 41, "status": 3, "seasons": [{ "seasonNumber": 3, "status": 3 }] },
            ],
        });
    }
    with_queue(state, id, &mut details);
    Some(details)
}

fn requests(seen: &Seen) -> Value {
    let by: u32 = seen.param("requestedBy").map_or(0, |b| b.parse().unwrap());
    let results = if by == VIEWER {
        json!([
            {
                "id": 40,
                "status": 1,
                "type": "tv",
                "media": { "tmdbId": NORTHERN_LINE, "mediaType": "tv", "status": 4 },
                "seasons": [{ "seasonNumber": 2, "status": 1 }],
            },
            {
                "id": 39,
                "status": 2,
                "type": "movie",
                "media": { "tmdbId": PAPER_MOONS, "mediaType": "movie", "status": 2 },
                "seasons": [],
            },
        ])
    } else {
        json!([])
    };
    let count = results.as_array().unwrap().len();
    json!({
        "pageInfo": { "pages": 1, "pageSize": 20, "results": count, "page": 1 },
        "results": results,
    })
}

enum Answer {
    Json(u16, Value),
    Bytes(Vec<u8>),
    Redirect(String),
}

fn refused() -> Answer {
    Answer::Json(
        403,
        json!({ "message": "You do not have permission to access this endpoint" }),
    )
}

fn answer(state: &mut State, seen: &Seen) -> Answer {
    if state.not_seerr {
        return Answer::Json(404, json!({ "error": "not here" }));
    }
    if let Some(elsewhere) = &state.redirect_to {
        return Answer::Redirect(format!("{elsewhere}{}", seen.path));
    }
    if let Some(path) = seen.path.strip_prefix("/t/p/") {
        // TMDb: no key, ever.
        return match path.split_once('/') {
            Some((_, picture)) if picture.ends_with(".jpg") => Answer::Bytes(PICTURE.to_vec()),
            _ => Answer::Json(404, json!({})),
        };
    }
    let Some(route) = seen.path.strip_prefix("/api/v1") else {
        return Answer::Json(404, json!({}));
    };
    if route == "/status" {
        return Answer::Json(200, json!({ "version": VERSION, "commitTag": "local" }));
    }
    if seen.api_key.as_deref() != Some(KEY) {
        return refused();
    }
    let segments: Vec<&str> = route.trim_start_matches('/').split('/').collect();
    let number = |at: usize| segments.get(at).and_then(|s| s.parse::<u64>().ok());
    let found = |value: Option<Value>| match value {
        Some(value) => Answer::Json(200, value),
        None => Answer::Json(404, json!({ "message": "Unable to retrieve" })),
    };
    match (seen.method.as_str(), segments.as_slice()) {
        ("GET", ["auth", "me"]) => Answer::Json(200, user(OWNER, "", "Owner")),
        ("GET", ["user"]) => Answer::Json(200, users(state, seen)),
        ("GET", ["user", _, "quota"]) => {
            let restricted = state.movie_quota == Some(0);
            Answer::Json(
                200,
                json!({
                    "movie": { "days": 7, "limit": 5, "used": 0, "remaining": state.movie_quota, "restricted": restricted },
                    "tv": { "days": 7, "limit": 0, "used": 0, "restricted": false },
                }),
            )
        }
        ("GET", ["search"]) => {
            if !strictly_encoded(&seen.raw_query) {
                return Answer::Json(400, json!({ "message": "query must be url encoded" }));
            }
            let query = seen.param("query").unwrap_or_default().to_lowercase();
            let mut hits: Vec<Value> = catalogue(state)
                .into_iter()
                .filter(|t| {
                    let name = t["title"].as_str().or(t["name"].as_str()).unwrap();
                    name.to_lowercase().contains(&query)
                })
                .collect();
            // A person matches too: Bobine leaves people out.
            hits.push(json!({ "id": 31, "mediaType": "person", "name": "Ines Varga" }));
            Answer::Json(200, page(hits))
        }
        ("GET", ["discover", ..]) if state.discover_forbidden => refused(),
        ("GET", ["discover", "trending"]) => Answer::Json(200, page(catalogue(state))),
        ("GET", ["discover", "movies"]) => {
            let movies = catalogue(state)
                .into_iter()
                .filter(|t| t["mediaType"] == "movie")
                .collect();
            Answer::Json(200, page(movies))
        }
        ("GET", ["discover", "movies", "upcoming"]) => {
            Answer::Json(200, page(vec![find(state, 9006, "movie").unwrap()]))
        }
        ("GET", ["discover", "tv"]) => {
            let series = catalogue(state)
                .into_iter()
                .filter(|t| t["mediaType"] == "tv")
                .collect();
            Answer::Json(200, page(series))
        }
        ("GET", ["movie" | "tv", _])
            if number(1).is_some_and(|id| state.failing_titles.contains(&id)) =>
        {
            Answer::Json(500, json!({ "message": "Something went wrong" }))
        }
        ("GET", ["movie", _]) => found(number(1).and_then(|id| movie_details(state, id))),
        ("GET", ["tv", _]) => found(number(1).and_then(|id| tv_details(state, id))),
        ("GET", ["movie", _, "recommendations"]) => Answer::Json(
            200,
            page(vec![
                find(state, HARBOUR_LIGHTS, "movie").unwrap(),
                find(state, PAPER_MOONS, "movie").unwrap(),
                find(state, 9006, "movie").unwrap(),
            ]),
        ),
        ("GET", ["tv", _, "recommendations"]) => {
            Answer::Json(200, page(vec![find(state, COPPER_COAST, "tv").unwrap()]))
        }
        ("GET", ["collection", _]) if number(1) == Some(COLLECTION) => {
            let parts: Vec<Value> = [LIGHTHOUSE, HARBOUR_LIGHTS, HARBOUR_NIGHTS, HARBOUR_DAWN]
                .iter()
                .map(|id| find(state, *id, "movie").unwrap())
                .collect();
            Answer::Json(
                200,
                json!({
                    "id": COLLECTION,
                    "name": "Harbour Saga",
                    "overview": "Four invented movies about one harbour.",
                    "posterPath": "/harbour-saga.jpg",
                    "backdropPath": "/harbour-saga-backdrop.jpg",
                    "parts": parts,
                }),
            )
        }
        ("GET", ["request"]) => Answer::Json(200, requests(seen)),
        ("POST", ["request"]) => {
            let body = seen.json();
            let id = body["mediaId"].as_u64().unwrap();
            if body["mediaType"] == "movie" {
                match &mut state.movie_quota {
                    Some(0) => {
                        return Answer::Json(403, json!({ "message": "Movie Quota exceeded." }));
                    }
                    Some(left) => *left -= 1,
                    None => {}
                }
            }
            if state.requested.contains(&id) {
                return Answer::Json(
                    409,
                    json!({ "message": "Request for this media already exists." }),
                );
            }
            state.requested.push(id);
            Answer::Json(
                201,
                json!({ "id": 100 + state.requested.len(), "status": 1 }),
            )
        }
        _ => Answer::Json(404, json!({ "message": "Not found" })),
    }
}

fn respond(request: tiny_http::Request, answer: Answer) {
    let _ = match answer {
        Answer::Json(code, value) => {
            let json = Header::from_bytes("Content-Type", "application/json").unwrap();
            request.respond(
                Response::from_string(value.to_string())
                    .with_status_code(code)
                    .with_header(json),
            )
        }
        Answer::Bytes(bytes) => request.respond(Response::from_data(bytes)),
        Answer::Redirect(to) => {
            let location = Header::from_bytes("Location", to).unwrap();
            request.respond(Response::empty(302).with_header(location))
        }
    };
}

fn header(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

impl FakeSeerr {
    pub fn start() -> Self {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let state = Arc::new(Mutex::new(State {
            seen: Vec::new(),
            not_seerr: false,
            redirect_to: None,
            failing_titles: Vec::new(),
            discover_forbidden: false,
            no_viewer: false,
            movie_quota: None,
            requested: Vec::new(),
            queue: Vec::new(),
        }));
        let thread = {
            let server = Arc::clone(&server);
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                for mut request in server.incoming_requests() {
                    let url = request.url().to_string();
                    let (path, raw_query) = url.split_once('?').unwrap_or((&url, ""));
                    let query = raw_query
                        .split('&')
                        .filter(|p| !p.is_empty())
                        .map(|p| {
                            let (k, v) = p.split_once('=').unwrap_or((p, ""));
                            (decode(k), decode(v))
                        })
                        .collect();
                    let mut body = String::new();
                    request.as_reader().read_to_string(&mut body).unwrap();
                    let seen = Seen {
                        method: request.method().as_str().to_string(),
                        path: path.to_string(),
                        raw_query: raw_query.to_string(),
                        query,
                        api_key: header(&request, "X-Api-Key"),
                        api_user: header(&request, "X-Api-User"),
                        body,
                    };
                    let answer = {
                        let mut state = state.lock().unwrap();
                        state.seen.push(seen.clone());
                        answer(&mut state, &seen)
                    };
                    respond(request, answer);
                }
            })
        };
        Self {
            url: format!("http://127.0.0.1:{port}"),
            state,
            server,
            thread: Some(thread),
        }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    /// The posts of requests, in the order they came.
    pub fn posts(&self) -> Vec<Value> {
        self.seen()
            .into_iter()
            .filter(|s| s.method == "POST" && s.path == "/api/v1/request")
            .map(|s| s.json())
            .collect()
    }
}
