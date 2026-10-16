//! A fake Jellyfin server for the tests, on a local port: it answers the routes Bobine uses with
//! the fixtures beside this file (written from the API description, every name in them invented)
//! and keeps every request it gets.

use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use tiny_http::{Header, Response, Server};

pub const USER: &str = "ada";
pub const PASSWORD: &str = "correct horse battery staple";
pub const USER_ID: &str = "0f1e2d3c4b5a69788796a5b4c3d2e1f0";
pub const SERVER_ID: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90";
pub const TOKEN: &str = "fa4e70ce0b5e4f2c9d1a7b3e8c6d2f10";
pub const FILMS_LIBRARY: &str = "11111111111111111111111111111111";
pub const FILM: &str = "f0000000000000000000000000000001";
pub const SOURCE: &str = "5a000000000000000000000000000001";
pub const PLAY_SESSION: &str = "9c0ffee09c0ffee09c0ffee09c0ffee0";
pub const IMAGE: &[u8] = b"\x89PNG fake image";
pub const SERIES_LIBRARY: &str = "22222222222222222222222222222222";
pub const SERIES: &str = "50000000000000000000000000000001";
pub const SEASON: &str = "5e000000000000000000000000000002";
pub const EPISODE: &str = "e0000000000000000000000000000021";

fn fixture(name: &str) -> Value {
    let text = match name {
        "system_info_public" => include_str!("fixtures/system_info_public.json"),
        "authenticate_by_name" => include_str!("fixtures/authenticate_by_name.json"),
        "user_views" => include_str!("fixtures/user_views.json"),
        "films" => include_str!("fixtures/films.json"),
        "film_detail" => include_str!("fixtures/film_detail.json"),
        "boxsets" => include_str!("fixtures/boxsets.json"),
        "favourites" => include_str!("fixtures/favourites.json"),
        "boxset_sea_stories" => include_str!("fixtures/boxset_sea_stories.json"),
        "boxset_harbour_trilogy" => include_str!("fixtures/boxset_harbour_trilogy.json"),
        "similar" => include_str!("fixtures/similar.json"),
        "resume" => include_str!("fixtures/resume.json"),
        "search_series" => include_str!("fixtures/search_series.json"),
        "persons" => include_str!("fixtures/persons.json"),
        "person_titles" => include_str!("fixtures/person_titles.json"),
        "person_episodes" => include_str!("fixtures/person_episodes.json"),
        "playback_info" => include_str!("fixtures/playback_info.json"),
        "series" => include_str!("fixtures/series.json"),
        "series_detail" => include_str!("fixtures/series_detail.json"),
        "series_similar" => include_str!("fixtures/series_similar.json"),
        "seasons" => include_str!("fixtures/seasons.json"),
        "episodes" => include_str!("fixtures/episodes.json"),
        "next_up" => include_str!("fixtures/next_up.json"),
        _ => panic!("no fixture {name}"),
    };
    serde_json::from_str(text).unwrap()
}

/// A request as the fake server got it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub authorization: Option<String>,
    pub body: String,
}

impl Seen {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }
}

#[derive(Default)]
pub struct State {
    pub seen: Vec<Seen>,
    /// Answers every route like a web server that is not Jellyfin.
    pub not_jellyfin: bool,
    /// The token was revoked: every authenticated route answers 401.
    pub revoked: bool,
    /// The user may not see what is asked (parental control, access schedule): every
    /// authenticated route answers 403.
    pub forbidden: bool,
    /// The account is disabled: the login answers 403.
    pub account_disabled: bool,
    /// How long an image takes to be answered (a server converting a person's photo).
    pub image_delay: std::time::Duration,
    /// Every episode of every series is watched: Next Up has nothing.
    pub all_watched: bool,
    /// The films and series the user put in their list: the film and the series of the pages
    /// start out of it.
    pub favourites: Vec<String>,
    /// How many search requests are held until they have all arrived (0: none held), to tell
    /// requests sent at the same time from requests sent one after another.
    pub search_gate: usize,
    /// Search requests arrived at the gate so far.
    gate_arrivals: usize,
    /// For each held search request, whether all the others had arrived when it was let go.
    pub gate_met: Vec<bool>,
}

/// How long a held search request waits for the others before it is let go anyway.
const GATE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// A person with more titles than one page holds.
pub const PERSON_WITH_MANY_TITLES: &str = "9e000000000000000000000000000004";
/// A person with more titles than the program ever asks for.
pub const PERSON_WITH_ENDLESS_TITLES: &str = "9e000000000000000000000000000005";
/// A person seen only in episodes.
pub const PERSON_IN_EPISODES: &str = "9e000000000000000000000000000002";

pub struct FakeServer {
    pub url: String,
    pub state: Arc<Mutex<State>>,
    server: Arc<Server>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for FakeServer {
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
            b'+' => out.push(b' '),
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

fn split(url: &str) -> (String, Vec<(String, String)>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let query = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect();
    (path.to_string(), query)
}

enum Answer {
    Json(u16, Value),
    Bytes(Vec<u8>),
    Empty(u16),
}

/// The films (or series) of the library in the order asked, one page of them.
fn films(seen: &Seen, fixture_name: &str) -> Value {
    let mut films = fixture(fixture_name).as_array().unwrap().clone();
    let by = seen.param("SortBy").unwrap_or("SortName");
    let first = by.split(',').next().unwrap();
    films.sort_by_key(|f| f[first].to_string());
    if seen.param("SortOrder") == Some("Descending") {
        films.reverse();
    }
    let total = films.len();
    let start: usize = seen.param("StartIndex").map_or(0, |s| s.parse().unwrap());
    let limit: usize = seen.param("Limit").map_or(total, |s| s.parse().unwrap());
    let page: Vec<Value> = films.into_iter().skip(start).take(limit).collect();
    json!({ "Items": page, "TotalRecordCount": total, "StartIndex": start })
}

/// The items of a fixture whose name holds the search term, at most `Limit` of them.
fn matching(fixture_name: &str, seen: &Seen) -> Value {
    let term = seen.param("SearchTerm").unwrap_or_default().to_lowercase();
    let limit: usize = seen
        .param("Limit")
        .map_or(usize::MAX, |s| s.parse().unwrap());
    let found: Vec<Value> = fixture(fixture_name)
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| {
            item["Name"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains(&term)
        })
        .take(limit)
        .cloned()
        .collect();
    json!({ "Items": found, "TotalRecordCount": found.len() })
}

/// The items of `list` of the types asked, as the server keeps only those; all when none asked.
fn of_types(mut list: Value, seen: &Seen) -> Value {
    if let Some(types) = seen.param("IncludeItemTypes") {
        let items = list["Items"].as_array_mut().unwrap();
        items.retain(|item| types.split(',').any(|t| item["Type"] == t));
        list["TotalRecordCount"] = json!(items.len());
    }
    list
}

/// What a person took part in, as the server pages it: only the types asked, `Limit` of them
/// from `StartIndex`, and the total of them all.
fn person_titles(seen: &Seen) -> Value {
    let person = seen.param("PersonIds").unwrap();
    let mut items: Vec<Value> = match person {
        PERSON_WITH_MANY_TITLES => generated_films(250),
        PERSON_WITH_ENDLESS_TITLES => generated_films(2500),
        PERSON_IN_EPISODES => fixture("person_episodes").as_array().unwrap().clone(),
        _ => fixture("person_titles").as_array().unwrap().clone(),
    };
    let types = seen.param("IncludeItemTypes").unwrap_or_default();
    items.retain(|item| types.split(',').any(|t| item["Type"] == t));
    let total = items.len();
    let start: usize = seen.param("StartIndex").map_or(0, |s| s.parse().unwrap());
    let limit: usize = seen.param("Limit").map_or(total, |s| s.parse().unwrap());
    let page: Vec<Value> = items.into_iter().skip(start).take(limit).collect();
    json!({ "Items": page, "TotalRecordCount": total, "StartIndex": start })
}

fn generated_films(count: usize) -> Vec<Value> {
    (0..count)
        .map(|i| {
            json!({
                "Name": format!("Reel {i:04}"),
                "Id": format!("f1{i:030}"),
                "Type": "Movie",
                "ProductionYear": 1950 + i % 70,
            })
        })
        .collect()
}

/// An item as the server shows it to a user whose list holds `state.favourites`.
fn favourite_of(state: &State, id: &str, mut item: Value) -> Value {
    item["UserData"]["IsFavorite"] = json!(state.favourites.iter().any(|f| f == id));
    item
}

fn answer(state: &State, seen: &Seen) -> Answer {
    if state.not_jellyfin {
        return Answer::Json(404, json!({ "error": "not here" }));
    }
    let route = (seen.method.as_str(), seen.path.as_str());
    match route {
        ("GET", "/System/Info/Public") => return Answer::Json(200, fixture("system_info_public")),
        ("POST", "/Users/AuthenticateByName") => {
            if state.account_disabled {
                return Answer::Json(403, json!("User is disabled."));
            }
            let body = seen.json();
            return if body["Username"] == USER && body["Pw"] == PASSWORD {
                Answer::Json(200, fixture("authenticate_by_name"))
            } else {
                Answer::Json(401, json!("Error processing request."))
            };
        }
        _ => {}
    }
    let token = format!("Token=\"{TOKEN}\"");
    let signed_in = seen
        .authorization
        .as_deref()
        .is_some_and(|a| a.starts_with("MediaBrowser ") && a.contains(&token));
    if !signed_in || state.revoked {
        return Answer::Empty(401);
    }
    if state.forbidden {
        return Answer::Empty(403);
    }
    let images = format!("/Items/{FILM}/Images/");
    match route {
        ("POST", "/Sessions/Logout") => Answer::Empty(204),
        ("GET", "/UserViews") => Answer::Json(200, fixture("user_views")),
        ("GET", "/UserItems/Resume") => Answer::Json(200, fixture("resume")),
        ("GET", "/Persons") => Answer::Json(200, matching("persons", seen)),
        ("GET", "/Items") if seen.param("Filters") == Some("IsFavorite") => {
            Answer::Json(200, fixture("favourites"))
        }
        ("GET", "/Items") => match (seen.param("ParentId"), seen.param("IncludeItemTypes")) {
            (_, Some("Movie")) if seen.param("SearchTerm").is_some() => {
                Answer::Json(200, matching("films", seen))
            }
            (_, Some("Series")) if seen.param("SearchTerm").is_some() => {
                Answer::Json(200, matching("search_series", seen))
            }
            _ if seen.param("PersonIds").is_some() => Answer::Json(200, person_titles(seen)),
            (Some(FILMS_LIBRARY), _) => Answer::Json(200, films(seen, "films")),
            (Some(SERIES_LIBRARY), Some("Series")) => Answer::Json(200, films(seen, "series")),
            (None, Some("BoxSet")) => Answer::Json(200, fixture("boxsets")),
            (Some("b0000000000000000000000000000001"), _) => {
                Answer::Json(200, of_types(fixture("boxset_sea_stories"), seen))
            }
            (Some("b0000000000000000000000000000002"), _) => {
                Answer::Json(200, of_types(fixture("boxset_harbour_trilogy"), seen))
            }
            _ => Answer::Json(200, json!({ "Items": [], "TotalRecordCount": 0 })),
        },
        ("GET", path) if path == format!("/Items/{FILM}") => {
            Answer::Json(200, favourite_of(state, FILM, fixture("film_detail")))
        }
        ("GET", path) if path.starts_with("/Items/b000") => fixture("boxsets")["Items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|boxset| path == format!("/Items/{}", boxset["Id"].as_str().unwrap()))
            .map_or(Answer::Empty(404), |boxset| {
                Answer::Json(200, boxset.clone())
            }),
        ("POST" | "DELETE", path) if path.starts_with("/UserFavoriteItems/") => Answer::Json(
            200,
            json!({ "IsFavorite": seen.method == "POST", "PlaybackPositionTicks": 0 }),
        ),
        ("GET", path) if path == format!("/Items/{FILM}/Similar") => {
            Answer::Json(200, fixture("similar"))
        }
        ("POST", path) if path == format!("/Items/{FILM}/PlaybackInfo") => {
            Answer::Json(200, fixture("playback_info"))
        }
        ("POST", path) if path == format!("/Items/{EPISODE}/PlaybackInfo") => {
            let episode = &fixture("episodes")["Items"][0];
            Answer::Json(
                200,
                json!({ "MediaSources": episode["MediaSources"], "PlaySessionId": PLAY_SESSION }),
            )
        }
        (
            "POST",
            "/Sessions/Playing" | "/Sessions/Playing/Progress" | "/Sessions/Playing/Stopped",
        ) => Answer::Empty(204),
        ("GET", path) if path == format!("/Items/{SERIES}") => {
            Answer::Json(200, favourite_of(state, SERIES, fixture("series_detail")))
        }
        ("GET", path) if path == format!("/Items/{SERIES}/Similar") => {
            Answer::Json(200, fixture("series_similar"))
        }
        ("GET", path) if path == format!("/Shows/{SERIES}/Seasons") => {
            Answer::Json(200, fixture("seasons"))
        }
        ("GET", path) if path == format!("/Shows/{SERIES}/Episodes") => {
            match seen.param("SeasonId") {
                Some(SEASON) => Answer::Json(200, fixture("episodes")),
                _ => Answer::Json(200, json!({ "Items": [], "TotalRecordCount": 0 })),
            }
        }
        ("GET", "/Shows/NextUp") => match seen.param("SeriesId") {
            Some(SERIES) if !state.all_watched => Answer::Json(200, fixture("next_up")),
            _ => Answer::Json(200, json!({ "Items": [], "TotalRecordCount": 0 })),
        },
        ("POST" | "DELETE", path)
            if [FILM, SEASON, EPISODE]
                .iter()
                .any(|id| path == format!("/UserPlayedItems/{id}")) =>
        {
            Answer::Json(
                200,
                json!({ "Played": seen.method == "POST", "PlaybackPositionTicks": 0 }),
            )
        }
        ("GET", path) if path.starts_with(&images) => Answer::Bytes(IMAGE.to_vec()),
        _ => Answer::Empty(404),
    }
}

/// Waits until every held search request has arrived; false if they did not all come in time.
fn wait_at_gate(state: &Mutex<State>) -> bool {
    let started = std::time::Instant::now();
    loop {
        {
            let state = state.lock().unwrap();
            if state.gate_arrivals >= state.search_gate {
                return true;
            }
        }
        if started.elapsed() > GATE_TIMEOUT {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn respond(request: tiny_http::Request, answer: Answer, delay: std::time::Duration) {
    let json = Header::from_bytes("Content-Type", "application/json").unwrap();
    let _ = match answer {
        Answer::Json(code, value) => request.respond(
            Response::from_string(value.to_string())
                .with_status_code(code)
                .with_header(json),
        ),
        Answer::Bytes(bytes) => {
            std::thread::sleep(delay);
            request.respond(Response::from_data(bytes))
        }
        Answer::Empty(code) => request.respond(Response::empty(code)),
    };
}

impl FakeServer {
    pub fn start() -> Self {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let port = server.server_addr().to_ip().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let thread = {
            let server = Arc::clone(&server);
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                for mut request in server.incoming_requests() {
                    let (path, query) = split(request.url());
                    let mut body = String::new();
                    request.as_reader().read_to_string(&mut body).unwrap();
                    let authorization = request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Authorization"))
                        .map(|h| h.value.as_str().to_string());
                    let seen = Seen {
                        method: request.method().as_str().to_string(),
                        path,
                        query,
                        authorization,
                        body,
                    };
                    let (answer, delay, gated) = {
                        let mut state = state.lock().unwrap();
                        state.seen.push(seen.clone());
                        if let Some(id) = seen.path.strip_prefix("/UserFavoriteItems/") {
                            state.favourites.retain(|f| f != id);
                            if seen.method == "POST" {
                                state.favourites.push(id.to_string());
                            }
                        }
                        let gated = state.search_gate > 0
                            && (seen.path == "/Persons" || seen.param("SearchTerm").is_some());
                        if gated {
                            state.gate_arrivals += 1;
                        }
                        (answer(&state, &seen), state.image_delay, gated)
                    };
                    if gated {
                        let state = Arc::clone(&state);
                        std::thread::spawn(move || {
                            let met = wait_at_gate(&state);
                            state.lock().unwrap().gate_met.push(met);
                            respond(request, answer, delay);
                        });
                    } else {
                        respond(request, answer, delay);
                    }
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

    /// The requests so far, oldest first.
    pub fn seen(&self) -> Vec<Seen> {
        self.state.lock().unwrap().seen.clone()
    }

    /// The last request to this path.
    pub fn last(&self, method: &str, path: &str) -> Seen {
        self.seen()
            .into_iter()
            .rev()
            .find(|s| s.method == method && s.path == path)
            .unwrap_or_else(|| panic!("no {method} {path}"))
    }
}
