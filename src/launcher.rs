//! The commands the desktop's launcher runs (its contract is `docs/launcher-format.md` in the
//! poste repository): `home` and `search` print launcher entries in JSON on stdout, and never open
//! a window; `open` and `play` go to the window (see `instance`).
//!
//! The exit code says how it went, beside a message entry the launcher can show: `OK`,
//! `NOT_SIGNED_IN`, `UNREACHABLE`, `FAILED` (the keyring, the account file, an answer the server
//! should not have given) and `USAGE`.

use std::sync::Arc;

use bobine::provider::{
    Connector, Episode, Film, ItemId, MediaServer, ProviderError, SeriesHit, Title,
};
use serde_json::{Value, json};

use crate::film_page::{self, From};
use crate::home_page;
use crate::instance::{Kind, Request, valid_id};
use crate::series_page;
use crate::start;

pub const OK: i32 = 0;
pub const NOT_SIGNED_IN: i32 = 1;
pub const UNREACHABLE: i32 = 2;
pub const FAILED: i32 = 3;
pub const USAGE: i32 = 4;

/// How many films, and how many series, a search lists.
const FOUND: u32 = 20;

/// What the entries show for an icon: Bobine's own, from the icon theme.
const ICON: &str = "bobine";

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
/// the account file, and blocks.
pub fn run(
    command: &Command,
    open: impl FnOnce() -> Result<Arc<dyn Connector>, ProviderError>,
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
        Command::Home => home(&*server),
        Command::Search(text) => search(&*server, text),
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
fn home(server: &dyn MediaServer) -> Result<Outcome, ProviderError> {
    let started = server.continue_watching()?;
    let next = home_page::next_up_row(server.next_episodes(home_page::ROW)?, &started);
    let entries = started
        .iter()
        .map(title_entry)
        .chain(next.iter().map(episode_entry))
        .collect();
    Ok(Outcome::ok(entries))
}

/// The films, then the series, whose name holds `text`.
fn search(server: &dyn MediaServer, text: &str) -> Result<Outcome, ProviderError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Outcome::ok(Vec::new()));
    }
    let found = server.search(text, FOUND)?;
    let entries = found
        .films
        .iter()
        .map(film_entry)
        .chain(found.series.iter().map(series_entry))
        .collect();
    Ok(Outcome::ok(entries))
}

fn title_entry(title: &Title) -> Value {
    match title {
        Title::Film(film) => film_entry(film),
        Title::Episode(episode) => episode_entry(episode),
        Title::Series(series) => entry(
            Kind::Series,
            &series.id,
            &series.title,
            words(["Series".to_string()], series_page::years(series)),
            false,
        ),
    }
}

fn film_entry(film: &Film) -> Value {
    let year = film.year.map(|year| year.to_string());
    let left = film_page::time_left(film);
    entry(
        Kind::Movie,
        &film.id,
        &film.title,
        words(["Movie".to_string()], year.into_iter().chain(left)),
        film.resume.is_some(),
    )
}

fn episode_entry(episode: &Episode) -> Value {
    let (series, line) = home_page::card_lines(&Title::Episode(episode.clone()));
    entry(
        Kind::Episode,
        &episode.item.id,
        &series,
        words(line, film_page::time_left(&episode.item)),
        episode.item.resume.is_some(),
    )
}

fn series_entry(series: &SeriesHit) -> Value {
    let year = series.year.map(|year| year.to_string());
    entry(
        Kind::Series,
        &series.id,
        &series.title,
        words(["Series".to_string()], year),
        false,
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
/// and open the page. A series plays its next episode.
fn entry(kind: Kind, id: &ItemId, name: &str, subtitle: String, started: bool) -> Value {
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
    json!({
        "id": format!("{}:{}", kind.word(), id.0),
        "name": name,
        "subtitle": subtitle,
        "icon": ICON,
        "verb": if started { "Resume" } else { "Play" },
        "action": command(play),
        "actions": actions,
    })
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
        let home = run(&Command::Home, connector(&server));
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
        let home = run(&Command::Home, connector(&server));
        assert_eq!(home.code, OK);
        assert_eq!(home.json(), "[]");
    }

    #[test]
    fn search_json_lists_the_matching_films_and_series_with_their_actions() {
        let server = FakeServer::new(6, true);
        let found = run(&Command::Search("hiver".into()), connector(&server));
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
        let found = run(&Command::Search("  ".into()), connector(&server));
        assert_eq!((found.code, found.json()), (OK, "[]".to_string()));
        assert!(server.state.lock().unwrap().record.searches.is_empty());
    }

    #[test]
    fn not_signed_in_exits_with_its_own_code_and_a_json_message() {
        let server = FakeServer::new(6, false);
        for command in [Command::Home, Command::Search("hiver".into())] {
            let said = run(&command, connector(&server));
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
        let said = run(&Command::Home, move || {
            let connector: Arc<dyn Connector> = Arc::new(JellyfinConnector::open(path, keyring)?);
            Ok(connector)
        });
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
            let said = run(&command, connector(&server));
            assert_eq!(said.code, UNREACHABLE);
            assert_ne!(UNREACHABLE, NOT_SIGNED_IN);
            assert_eq!(said.entries.len(), 1);
            assert_eq!(said.entries[0]["name"], "Server unreachable");
            assert!(said.entries[0].get("action").is_none());
        }
    }

    #[test]
    fn a_keyring_out_of_reach_is_a_failure_of_its_own() {
        let said = run(&Command::Home, || {
            Err(ProviderError::Keyring(
                "org.freedesktop.secrets is not running".into(),
            ))
        });
        assert_eq!(said.code, FAILED);
        // The window's words, not the error's.
        assert_eq!(
            said.entries[0]["name"],
            "The system keyring cannot be reached."
        );
    }

    #[test]
    fn an_id_is_one_word_for_the_shell_whatever_it_holds() {
        assert_eq!(quoted("film-0"), "'film-0'");
        assert_eq!(quoted("a'b; rm -rf ~"), r"'a'\''b; rm -rf ~'");
    }
}
