//! The commands the desktop's launcher runs (its contract is `docs/launcher-format.md` in the
//! poste repository): `home` and `search` print launcher entries in JSON on stdout, and never open
//! a window; `open` and `play` go to the window (see `instance`).
//!
//! The exit code says how it went, beside a message entry the launcher can show: `OK`,
//! `NOT_SIGNED_IN`, `UNREACHABLE`, `FAILED` (the keyring, the account file, an answer the server
//! should not have given) and `USAGE`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use bobine::provider::{
    Connector, Episode, Film, ImageRef, ItemId, MediaServer, ProviderError, SeriesHit, Title,
};
use serde_json::{Value, json};

use crate::film_page::{self, From};
use crate::home_page;
use crate::images;
use crate::instance::{Kind, Request, valid_id};
use crate::mpris;
use crate::series_page;
use crate::start;

pub const OK: i32 = 0;
pub const NOT_SIGNED_IN: i32 = 1;
pub const UNREACHABLE: i32 = 2;
pub const FAILED: i32 = 3;
pub const USAGE: i32 = 4;

/// How many films, and how many series, a search lists.
const FOUND: u32 = 20;

/// What an entry shows for an icon when it has no poster: Bobine's own, from the icon theme.
const ICON: &str = "bobine";

/// How long the entries wait, in all, for the posters they do not have yet: a launcher asks at
/// every keystroke, and a list without some posters is better than a list that comes late.
const POSTER_BUDGET: Duration = Duration::from_secs(1);

/// How many posters are fetched at once.
const POSTER_FETCHES: usize = 8;

/// An entry, with the poster its icon is made of.
type Listed = (Option<ImageRef>, Value);

/// Where the posters are kept, and how long the entries wait for those not kept yet.
pub struct Posters {
    pub dir: PathBuf,
    pub budget: Duration,
}

impl Posters {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            budget: POSTER_BUDGET,
        }
    }
}

/// `$XDG_CACHE_HOME/bobine/posters`, else `~/.cache/bobine/posters`, beside the desktop's art.
pub fn poster_dir(
    xdg_cache_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    Some(crate::mpris::art_dir(xdg_cache_home, home)?.with_file_name("posters"))
}

const HOW: &str = "bobine home --json | search <text> --json | open movie|series|episode:<id> | \
                   play <id> [--from-start]";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Home,
    Search(String),
    /// `open` and `play`: for the window.
    Ask(Request),
}

/// The command `args` name (the program's name left out); `None` when they name none, and the
/// window opens. An error is a usage error, said in a line.
pub fn parse(args: &[String]) -> Option<Result<Command, String>> {
    let (first, rest) = args.split_first()?;
    // The flags of the headless harness, and any other, are the window's.
    if first.starts_with('-') {
        return None;
    }
    let words: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--json")
        .collect();
    Some(match (first.as_str(), words.as_slice()) {
        ("home", []) => Ok(Command::Home),
        ("home", _) => Err("home takes nothing but --json".into()),
        // The launcher hands the text typed as one argument, empty before anything is typed.
        ("search", text) => Ok(Command::Search(text.join(" "))),
        ("open", [address]) => address
            .split_once(':')
            .and_then(|(kind, id)| Some((Kind::from_word(kind)?, id)))
            .filter(|(_, id)| valid_id(id))
            .map(|(kind, id)| Command::Ask(Request::Open(kind, ItemId(id.into()))))
            .ok_or_else(|| {
                format!("open names movie:<id>, series:<id> or episode:<id>, not {address}")
            }),
        ("play", [id] | [id, "--from-start"] | ["--from-start", id]) if valid_id(id) => {
            let from = if words.contains(&"--from-start") {
                From::Beginning
            } else {
                From::Resume
            };
            Ok(Command::Ask(Request::Play(ItemId((*id).into()), from)))
        }
        ("open" | "play", _) => Err(format!("{first}: {HOW}")),
        (other, _) => Err(format!("no command {other}: {HOW}")),
    })
}

/// What a command prints, and how it exits.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    pub code: i32,
    pub entries: Vec<Value>,
}

impl Outcome {
    fn ok(entries: Vec<Value>) -> Self {
        Self { code: OK, entries }
    }

    /// One entry that says what went wrong.
    fn message(code: i32, name: &str, subtitle: Option<&str>, action: Option<Value>) -> Self {
        let mut entry = json!({ "id": "bobine-message", "name": name, "icon": ICON });
        if let Some(subtitle) = subtitle {
            entry["subtitle"] = subtitle.into();
        }
        if let Some(action) = action {
            entry["action"] = action;
        }
        Self {
            code,
            entries: vec![entry],
        }
    }

    pub fn usage(said: &str) -> Self {
        Self::message(USAGE, "Bobine does not know this command", Some(said), None)
    }

    /// The JSON array, on one line.
    pub fn json(&self) -> String {
        Value::Array(self.entries.clone()).to_string()
    }
}

/// `home` or `search`, on the session the connector keeps. `open` gives the connector: it reads
/// the account file, and blocks. The entries wear their poster when `posters` says where to keep
/// them.
pub fn run(
    command: &Command,
    open: impl FnOnce() -> Result<Arc<dyn Connector>, ProviderError>,
    posters: Option<&Posters>,
) -> Outcome {
    let connector = match open() {
        Ok(connector) => connector,
        Err(error) => return failure(&error, None),
    };
    let server = match connector.restore() {
        Ok(Some(server)) => server,
        Ok(None) => return not_signed_in(),
        Err(error) => return failure(&error, Some(&*connector)),
    };
    let listed = match command {
        Command::Home => home(&server, posters),
        Command::Search(text) => search(&server, text, posters),
        Command::Ask(_) => Ok(Outcome::ok(Vec::new())),
    };
    listed.unwrap_or_else(|error| failure(&error, Some(&*connector)))
}

fn not_signed_in() -> Outcome {
    // Bobine's window is where one signs in.
    Outcome::message(
        NOT_SIGNED_IN,
        "Sign in to Bobine",
        Some("Not signed in to a media server"),
        Some(command("bobine".into())),
    )
}

fn failure(error: &ProviderError, connector: Option<&dyn Connector>) -> Outcome {
    match error {
        // A locked keyring hides the token as well as no login does.
        ProviderError::SessionExpired
        | ProviderError::WrongCredentials
        | ProviderError::KeyringLocked => not_signed_in(),
        ProviderError::Unreachable(_) => {
            let address = connector.and_then(|connector| connector.last_address());
            Outcome::message(UNREACHABLE, "Server unreachable", address.as_deref(), None)
        }
        // The words chosen for the window: never the error's own, which may carry more.
        error => Outcome::message(FAILED, start::error_message(error), None, None),
    }
}

/// Continue watching, then Next up, as the Home lists them.
fn home(
    server: &Arc<dyn MediaServer>,
    posters: Option<&Posters>,
) -> Result<Outcome, ProviderError> {
    let started = server.continue_watching()?;
    let next = home_page::next_up_row(server.next_episodes(home_page::ROW)?, &started);
    let listed = started
        .iter()
        .map(title_entry)
        .chain(next.iter().map(episode_entry))
        .collect();
    Ok(Outcome::ok(with_icons(listed, server, posters)))
}

/// The films, then the series, whose name holds `text`.
fn search(
    server: &Arc<dyn MediaServer>,
    text: &str,
    posters: Option<&Posters>,
) -> Result<Outcome, ProviderError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Outcome::ok(Vec::new()));
    }
    let found = server.search(text, FOUND)?;
    let listed = found
        .films
        .iter()
        .map(film_entry)
        .chain(found.series.iter().map(series_entry))
        .collect();
    Ok(Outcome::ok(with_icons(listed, server, posters)))
}

fn title_entry(title: &Title) -> Listed {
    match title {
        Title::Film(film) => film_entry(film),
        Title::Episode(episode) => episode_entry(episode),
        Title::Series(series) => entry(
            Kind::Series,
            &series.id,
            &series.title,
            words(["Series".to_string()], series_page::years(series)),
            false,
            series.poster.clone(),
        ),
    }
}

fn film_entry(film: &Film) -> Listed {
    let year = film.year.map(|year| year.to_string());
    let left = film_page::time_left(film);
    entry(
        Kind::Movie,
        &film.id,
        &film.title,
        words(["Movie".to_string()], year.into_iter().chain(left)),
        film.resume.is_some(),
        film.poster.clone(),
    )
}

fn episode_entry(episode: &Episode) -> Listed {
    let (series, line) = home_page::card_lines(&Title::Episode(episode.clone()));
    entry(
        Kind::Episode,
        &episode.item.id,
        &series,
        words(line, film_page::time_left(&episode.item)),
        episode.item.resume.is_some(),
        episode.series_poster.clone(),
    )
}

fn series_entry(series: &SeriesHit) -> Listed {
    let year = series.year.map(|year| year.to_string());
    entry(
        Kind::Series,
        &series.id,
        &series.title,
        words(["Series".to_string()], year),
        false,
        series.poster.clone(),
    )
}

/// The words of a subtitle, apart by a dot.
fn words(
    first: impl IntoIterator<Item = String>,
    more: impl IntoIterator<Item = String>,
) -> String {
    first
        .into_iter()
        .chain(more)
        .collect::<Vec<_>>()
        .join(" · ")
}

/// An entry: Enter plays (or resumes, when `started`), and the other actions play from the start
/// and open the page. A series plays its next episode. `poster` is the picture of its icon, which
/// stays Bobine's own until `with_icons` has one.
fn entry(
    kind: Kind,
    id: &ItemId,
    name: &str,
    subtitle: String,
    started: bool,
    poster: Option<ImageRef>,
) -> Listed {
    let id_word = quoted(&id.0);
    let play = format!("bobine play {id_word}");
    let mut actions = Vec::new();
    if started {
        actions.push(json!({
            "label": "Play from the start",
            "action": command(format!("{play} --from-start")),
        }));
    }
    actions.push(json!({
        "label": "Open the page",
        "action": command(format!("bobine open {}", quoted(&format!("{}:{}", kind.word(), id.0)))),
    }));
    let entry = json!({
        "id": format!("{}:{}", kind.word(), id.0),
        "name": name,
        "subtitle": subtitle,
        "icon": ICON,
        "verb": if started { "Resume" } else { "Play" },
        "action": command(play),
        "actions": actions,
    });
    (poster, entry)
}

/// The entries, each with the path of its poster for an icon when it has one in the end.
fn with_icons(
    listed: Vec<Listed>,
    server: &Arc<dyn MediaServer>,
    posters: Option<&Posters>,
) -> Vec<Value> {
    let kept = posters.map_or_else(HashMap::new, |posters| {
        keep_posters(
            server,
            posters,
            listed.iter().filter_map(|(poster, _)| poster.as_ref()),
        )
    });
    listed
        .into_iter()
        .map(|(poster, mut entry)| {
            let path = poster.and_then(|poster| kept.get(&(poster.item.0, poster.tag)).cloned());
            if let Some(path) = path.as_deref().and_then(Path::to_str) {
                entry["icon"] = path.into();
            }
            entry
        })
        .collect()
}

/// The path of each poster, by item and tag: those already kept, then those fetched in time.
/// Whatever is late is left out, and its fetch ends with the process.
fn keep_posters<'a>(
    server: &Arc<dyn MediaServer>,
    posters: &Posters,
    wanted: impl Iterator<Item = &'a ImageRef>,
) -> HashMap<(String, String), PathBuf> {
    let mut kept = HashMap::new();
    let mut missing = VecDeque::new();
    let mut seen = HashSet::new();
    for image in wanted {
        let key = (image.item.0.clone(), image.tag.clone());
        if !seen.insert(key.clone()) {
            continue;
        }
        match mpris::cached_art(&posters.dir, &key.0, &key.1) {
            Some(path) => {
                kept.insert(key, path);
            }
            None => missing.push_back(image.clone()),
        }
    }
    if missing.is_empty() {
        return kept;
    }
    let workers = missing.len().min(POSTER_FETCHES);
    let queue = Arc::new(Mutex::new(missing));
    let (sender, receiver) = mpsc::channel();
    for _ in 0..workers {
        let (server, queue, sender, dir) = (
            server.clone(),
            queue.clone(),
            sender.clone(),
            posters.dir.clone(),
        );
        std::thread::spawn(move || {
            while let Some(image) = queue.lock().ok().and_then(|mut queue| queue.pop_front()) {
                if let Some(path) = fetch_poster(&*server, &dir, &image) {
                    let _ = sender.send(((image.item.0, image.tag), path));
                }
            }
        });
    }
    drop(sender);
    let deadline = Instant::now() + posters.budget;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(left) {
            Ok((key, path)) => {
                kept.insert(key, path);
            }
            Err(_) => break,
        }
    }
    kept
}

/// The poster, as small as the window asks it, kept as the server sent it. An answer that is not
/// an image is not kept: it would be served as the icon for good.
fn fetch_poster(server: &dyn MediaServer, dir: &Path, image: &ImageRef) -> Option<PathBuf> {
    let bytes = server.image(image, images::POSTER).ok()?;
    images::format(&bytes)?;
    mpris::store_art(dir, &image.item.0, &image.tag, &bytes).ok()
}

fn command(line: String) -> Value {
    json!({ "type": "command", "command": line })
}

/// `word` as one word for `bash -c`, whatever the server put in an id.
fn quoted(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use bobine::jellyfin::JellyfinConnector;

    use super::*;
    use crate::fake::{FakeServer, MemoryKeyring};

    /// An account file that kept a login: its token is in the keyring.
    const KEPT_LOGIN: &str = r#"device_id = "0123456789abcdef0123456789abcdef"

[login]
address = "https://media.example.org"
server_id = "server-1"
server_name = "Salon"
server_version = "10.11.0"
user_id = "user-1"
user_name = "camille"
"#;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    fn connector(
        server: &FakeServer,
    ) -> impl FnOnce() -> Result<Arc<dyn Connector>, ProviderError> {
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        move || Ok(connector)
    }

    fn ids(outcome: &Outcome) -> Vec<&str> {
        outcome
            .entries
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect()
    }

    fn entry_of<'a>(outcome: &'a Outcome, id: &str) -> &'a Value {
        outcome
            .entries
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap_or_else(|| panic!("no {id} in {}", outcome.json()))
    }

    fn commands(entry: &Value) -> Vec<(&str, &str)> {
        entry["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|action| {
                assert_eq!(action["action"]["type"], "command");
                (
                    action["label"].as_str().unwrap(),
                    action["action"]["command"].as_str().unwrap(),
                )
            })
            .collect()
    }

    #[test]
    fn the_commands_are_read_from_the_arguments() {
        assert_eq!(parse(&args("home --json")), Some(Ok(Command::Home)));
        assert_eq!(
            parse(&args("search --json les marées")),
            Some(Ok(Command::Search("les marées".into())))
        );
        assert_eq!(
            parse(&args("search --json")),
            Some(Ok(Command::Search(String::new())))
        );
        assert_eq!(
            parse(&args("open movie:film-7")),
            Some(Ok(Command::Ask(Request::Open(
                Kind::Movie,
                ItemId("film-7".into())
            ))))
        );
        assert_eq!(
            parse(&args("open episode:series-0-s2-e4")),
            Some(Ok(Command::Ask(Request::Open(
                Kind::Episode,
                ItemId("series-0-s2-e4".into())
            ))))
        );
        assert_eq!(
            parse(&args("play film-0")),
            Some(Ok(Command::Ask(Request::Play(
                ItemId("film-0".into()),
                From::Resume
            ))))
        );
        assert_eq!(
            parse(&args("play film-0 --from-start")),
            Some(Ok(Command::Ask(Request::Play(
                ItemId("film-0".into()),
                From::Beginning
            ))))
        );
    }

    #[test]
    fn no_command_opens_the_window() {
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&args("--screen home")), None);
    }

    #[test]
    fn a_wrong_command_is_a_usage_error() {
        for line in [
            "frobnicate",
            "home now",
            "open",
            "open film-7",
            "open album:a1",
            "open movie:",
            "play",
            "play a b",
            "play movie:0/1",
        ] {
            assert!(matches!(parse(&args(line)), Some(Err(_))), "{line}");
        }
        let usage = Outcome::usage("open: …");
        assert_eq!(usage.code, USAGE);
        assert_eq!(usage.entries.len(), 1);
        assert!(usage.entries[0].get("action").is_none());
    }

    #[test]
    fn home_json_lists_the_titles_to_resume_then_the_next_episodes_with_their_actions() {
        let server = FakeServer::new(6, true);
        let home = run(&Command::Home, connector(&server), None);
        assert_eq!(home.code, OK);
        assert_eq!(
            ids(&home),
            [
                "movie:film-0",
                "movie:film-2",
                "movie:film-5",
                "episode:series-0-s2-e4",
                // Next up: the episode started is already listed above.
                "episode:series-2-s1-e3",
            ]
        );
        let film = entry_of(&home, "movie:film-0");
        assert_eq!(film["name"], "Le Dernier Hiver");
        assert_eq!(film["subtitle"], "Movie · 2025 · 59m left");
        assert_eq!(film["icon"], "bobine");
        assert_eq!(film["verb"], "Resume");
        assert_eq!(film["action"]["type"], "command");
        assert_eq!(film["action"]["command"], "bobine play 'film-0'");
        assert_eq!(
            commands(film),
            [
                ("Play from the start", "bobine play 'film-0' --from-start"),
                ("Open the page", "bobine open 'movie:film-0'"),
            ]
        );
        let next = entry_of(&home, "episode:series-2-s1-e3");
        assert_eq!(next["name"], "Hivernage");
        assert_eq!(next["verb"], "Play");
        assert_eq!(next["action"]["command"], "bobine play 'series-2-s1-e3'");
        // Not started: nothing to start again.
        assert_eq!(
            commands(next),
            [("Open the page", "bobine open 'episode:series-2-s1-e3'")]
        );
    }

    #[test]
    fn home_json_when_nothing_is_in_progress_prints_an_empty_array() {
        let server = FakeServer::new(6, true);
        {
            let mut state = server.state.lock().unwrap();
            for film in &mut state.films {
                film.resume = None;
            }
            for episode in &mut state.episodes {
                episode.item.resume = None;
                episode.item.watched = false;
            }
        }
        let home = run(&Command::Home, connector(&server), None);
        assert_eq!(home.code, OK);
        assert_eq!(home.json(), "[]");
    }

    #[test]
    fn search_json_lists_the_matching_films_and_series_with_their_actions() {
        let server = FakeServer::new(6, true);
        let found = run(&Command::Search("hiver".into()), connector(&server), None);
        assert_eq!(found.code, OK);
        assert_eq!(
            ids(&found),
            ["movie:film-0", "series:series-2", "series:series-3"]
        );
        assert_eq!(server.state.lock().unwrap().record.searches, ["hiver"]);
        let series = entry_of(&found, "series:series-3");
        assert_eq!(series["name"], "Hiver blanc");
        assert_eq!(series["verb"], "Play");
        assert_eq!(series["action"]["command"], "bobine play 'series-3'");
        assert_eq!(
            commands(series),
            [("Open the page", "bobine open 'series:series-3'")]
        );
        let film = entry_of(&found, "movie:film-0");
        assert_eq!(film["verb"], "Resume");
        assert_eq!(commands(film).len(), 2);
    }

    #[test]
    fn search_json_with_nothing_typed_prints_an_empty_array_and_asks_nothing() {
        let server = FakeServer::new(6, true);
        let found = run(&Command::Search("  ".into()), connector(&server), None);
        assert_eq!((found.code, found.json()), (OK, "[]".to_string()));
        assert!(server.state.lock().unwrap().record.searches.is_empty());
    }

    #[test]
    fn not_signed_in_exits_with_its_own_code_and_a_json_message() {
        let server = FakeServer::new(6, false);
        for command in [Command::Home, Command::Search("hiver".into())] {
            let said = run(&command, connector(&server), None);
            assert_eq!(said.code, NOT_SIGNED_IN);
            assert_eq!(said.entries.len(), 1);
            assert_eq!(said.entries[0]["name"], "Sign in to Bobine");
            // Enter opens the window, where one signs in.
            assert_eq!(said.entries[0]["action"]["command"], "bobine");
        }
    }

    #[test]
    fn a_locked_keyring_is_not_signed_in() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("account.toml");
        std::fs::write(&path, KEPT_LOGIN).unwrap();
        let keyring = Arc::new(MemoryKeyring::default());
        keyring.locked.store(true, Ordering::SeqCst);
        let said = run(
            &Command::Home,
            move || {
                let connector: Arc<dyn Connector> =
                    Arc::new(JellyfinConnector::open(path, keyring)?);
                Ok(connector)
            },
            None,
        );
        assert_eq!(said.code, NOT_SIGNED_IN);
        assert_eq!(said.entries[0]["name"], "Sign in to Bobine");
    }

    #[test]
    fn a_session_the_server_ended_is_not_signed_in_either() {
        let said = failure(&ProviderError::SessionExpired, None);
        assert_eq!(said.code, NOT_SIGNED_IN);
    }

    #[test]
    fn the_server_unreachable_exits_with_its_own_code_and_a_json_message() {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().unreachable = true;
        for command in [Command::Home, Command::Search("hiver".into())] {
            let said = run(&command, connector(&server), None);
            assert_eq!(said.code, UNREACHABLE);
            assert_ne!(UNREACHABLE, NOT_SIGNED_IN);
            assert_eq!(said.entries.len(), 1);
            assert_eq!(said.entries[0]["name"], "Server unreachable");
            assert!(said.entries[0].get("action").is_none());
        }
    }

    #[test]
    fn a_keyring_out_of_reach_is_a_failure_of_its_own() {
        let said = run(
            &Command::Home,
            || {
                Err(ProviderError::Keyring(
                    "org.freedesktop.secrets is not running".into(),
                ))
            },
            None,
        );
        assert_eq!(said.code, FAILED);
        // The window's words, not the error's.
        assert_eq!(
            said.entries[0]["name"],
            "The system keyring cannot be reached."
        );
    }

    /// A cache folder of the tests, and the posters kept in it: no budget runs out.
    fn posters_in(dir: &tempfile::TempDir) -> Posters {
        Posters {
            dir: dir.path().join("bobine").join("posters"),
            budget: Duration::from_secs(30),
        }
    }

    fn icon_of<'a>(outcome: &'a Outcome, id: &str) -> &'a str {
        entry_of(outcome, id)["icon"].as_str().unwrap()
    }

    #[test]
    fn home_json_gives_each_entry_its_poster_from_the_cache() {
        let server = FakeServer::new(6, true);
        let cache = tempfile::tempdir().unwrap();
        let posters = posters_in(&cache);
        let home = run(&Command::Home, connector(&server), Some(&posters));
        assert_eq!(home.code, OK);
        for (id, item) in [
            ("movie:film-0", "film-0"),
            ("movie:film-2", "film-2"),
            // An episode wears its series' poster, not its own still.
            ("episode:series-0-s2-e4", "series-0"),
            ("episode:series-2-s1-e3", "series-2"),
        ] {
            let icon = std::path::Path::new(icon_of(&home, id));
            assert!(icon.is_absolute(), "{id}: {icon:?}");
            assert!(icon.starts_with(&posters.dir), "{id}: {icon:?}");
            assert!(
                icon.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(&format!("{item}.")),
                "{id}: {icon:?}"
            );
            assert!(std::fs::metadata(icon).unwrap().len() > 0, "{id}");
        }
        // The window's width for a poster, so a poster is as small as the window's.
        let state = server.state.lock().unwrap();
        assert!(!state.record.images.is_empty());
        assert!(
            state
                .record
                .images
                .iter()
                .all(|(_, width)| *width == crate::images::POSTER)
        );
        // Nothing else in the folder than finished posters.
        for file in std::fs::read_dir(&posters.dir).unwrap().flatten() {
            let name = file.file_name().into_string().unwrap();
            assert!(!name.ends_with(".part"), "{name}");
        }
    }

    #[test]
    fn search_json_gives_each_entry_its_poster_too() {
        let server = FakeServer::new(6, true);
        let cache = tempfile::tempdir().unwrap();
        let posters = posters_in(&cache);
        let found = run(
            &Command::Search("hiver".into()),
            connector(&server),
            Some(&posters),
        );
        for id in ["movie:film-0", "series:series-2", "series:series-3"] {
            let icon = std::path::Path::new(icon_of(&found, id));
            assert!(icon.starts_with(&posters.dir), "{id}: {icon:?}");
            assert!(icon.is_file(), "{id}: {icon:?}");
        }
    }

    #[test]
    fn a_title_without_a_poster_keeps_the_bobine_icon() {
        let server = FakeServer::new(6, true);
        {
            let mut state = server.state.lock().unwrap();
            state.films[0].poster = None;
            for episode in &mut state.episodes {
                episode.series_poster = None;
            }
        }
        let cache = tempfile::tempdir().unwrap();
        let posters = posters_in(&cache);
        let home = run(&Command::Home, connector(&server), Some(&posters));
        assert_eq!(home.code, OK);
        assert_eq!(icon_of(&home, "movie:film-0"), "bobine");
        assert_eq!(icon_of(&home, "episode:series-0-s2-e4"), "bobine");
        // The others still have theirs.
        assert!(icon_of(&home, "movie:film-2").starts_with('/'));
    }

    #[test]
    fn a_poster_that_fails_leaves_the_list_printed_with_the_bobine_icon() {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().images_fail = true;
        let cache = tempfile::tempdir().unwrap();
        let posters = posters_in(&cache);
        let home = run(&Command::Home, connector(&server), Some(&posters));
        assert_eq!(home.code, OK);
        assert_eq!(home.entries.len(), 5);
        for entry in &home.entries {
            assert_eq!(entry["icon"], "bobine");
        }
        // Nothing half written is left to be taken for a poster.
        assert!(!posters.dir.exists() || std::fs::read_dir(&posters.dir).unwrap().next().is_none());
    }

    #[test]
    fn a_poster_that_is_late_does_not_hold_the_list_back() {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().image_delay = Duration::from_secs(5);
        let cache = tempfile::tempdir().unwrap();
        let posters = Posters {
            budget: Duration::from_millis(200),
            ..posters_in(&cache)
        };
        let started = std::time::Instant::now();
        let home = run(&Command::Home, connector(&server), Some(&posters));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(home.entries.len(), 5);
        assert_eq!(icon_of(&home, "movie:film-0"), "bobine");
    }

    #[test]
    fn a_poster_already_cached_is_not_fetched_again() {
        let server = FakeServer::new(6, true);
        let cache = tempfile::tempdir().unwrap();
        let posters = posters_in(&cache);
        let first = run(&Command::Home, connector(&server), Some(&posters));
        let asked = server.state.lock().unwrap().record.images.len();
        // Four posters for five entries: the series of the two episodes differ, the films too.
        assert!(asked >= 4, "{asked}");
        let second = run(&Command::Home, connector(&server), Some(&posters));
        assert_eq!(server.state.lock().unwrap().record.images.len(), asked);
        assert_eq!(first.entries, second.entries);
    }

    #[test]
    fn the_name_of_a_poster_is_made_of_its_item_and_tag_only() {
        let server = FakeServer::new(6, true);
        // A tag is the server's to write: it must not leave the folder nor carry a query.
        server.state.lock().unwrap().films[0].poster = Some(ImageRef {
            item: ItemId("film-0".into()),
            kind: bobine::provider::ImageKind::Primary,
            tag: "../t?api_key=SECRET".into(),
        });
        let cache = tempfile::tempdir().unwrap();
        let posters = posters_in(&cache);
        let home = run(&Command::Home, connector(&server), Some(&posters));
        let icon = std::path::Path::new(icon_of(&home, "movie:film-0"));
        assert_eq!(icon.parent(), Some(posters.dir.as_path()));
        let name = icon.file_name().unwrap().to_str().unwrap();
        assert!(
            !name.contains('?') && !name.contains('=') && !name.contains('/'),
            "{name}"
        );
    }

    #[test]
    fn an_id_is_one_word_for_the_shell_whatever_it_holds() {
        assert_eq!(quoted("film-0"), "'film-0'");
        assert_eq!(quoted("a'b; rm -rf ~"), r"'a'\''b; rm -rf ~'");
    }
}
