//! Each scenario of the provider port against the fake Jellyfin server and an in-memory keyring.

use super::JellyfinConnector;
use super::fake::*;
use crate::provider::*;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

struct Setup {
    fake: FakeServer,
    dir: TempDir,
    store: Arc<MemoryStore>,
    connector: JellyfinConnector,
}

impl Setup {
    fn new() -> Self {
        let fake = FakeServer::start();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        let connector =
            JellyfinConnector::open(dir.path().join("jellyfin.toml"), store.clone()).unwrap();
        Self {
            fake,
            dir,
            store,
            connector,
        }
    }

    fn signed_in(&self) -> Arc<dyn MediaServer> {
        self.connector
            .login(&self.fake.url, USER, PASSWORD)
            .unwrap()
    }

    /// A second start of the program, on the same account file and keyring.
    fn restart(&self) -> JellyfinConnector {
        JellyfinConnector::open(self.account_path(), self.store.clone()).unwrap()
    }

    fn account_path(&self) -> std::path::PathBuf {
        self.dir.path().join("jellyfin.toml")
    }

    fn account_text(&self) -> String {
        std::fs::read_to_string(self.account_path()).unwrap()
    }

    fn tokens(&self) -> Vec<String> {
        let tokens = self.store.tokens.lock().unwrap();
        tokens
            .values()
            .map(|t| t.expose_secret().to_string())
            .collect()
    }
}

fn film() -> ItemId {
    ItemId(FILM.into())
}

fn query(sort: FilmSort, descending: bool, start: u32, limit: u32) -> FilmQuery {
    FilmQuery {
        sort,
        descending,
        filters: Filters::default(),
        before: None,
        start,
        limit,
    }
}

fn titles(page: &Page<Film>) -> Vec<&str> {
    page.items.iter().map(|f| f.title.as_str()).collect()
}

#[test]
fn probe_shows_the_server_name_and_version() {
    let s = Setup::new();
    let info = s.connector.probe(&format!("  {}/ ", s.fake.url)).unwrap();
    assert_eq!(info.name, "Lighthouse");
    assert_eq!(info.version, "10.10.7");
    assert_eq!(info.id, SERVER_ID);
}

#[test]
fn not_a_jellyfin_server_is_refused() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().not_jellyfin = true;
    assert_eq!(
        s.connector.probe(&s.fake.url),
        Err(ProviderError::NotAMediaServer)
    );
    assert_eq!(
        s.connector.login(&s.fake.url, USER, PASSWORD).err(),
        Some(ProviderError::NotAMediaServer)
    );
    assert!(s.tokens().is_empty());
}

#[test]
fn an_address_without_a_scheme_is_refused() {
    let s = Setup::new();
    let bare = s.fake.url.trim_start_matches("http://").to_string();
    assert_eq!(s.connector.probe(&bare), Err(ProviderError::InvalidAddress));
}

#[test]
fn login_success_keeps_the_token_in_the_keyring() {
    let s = Setup::new();
    let server = s.signed_in();
    assert_eq!(server.server().name, "Lighthouse");
    assert_eq!(server.user_name(), USER);
    assert_eq!(server.user_id(), USER_ID);
    assert_eq!(s.tokens(), vec![TOKEN.to_string()]);

    let auth = s.fake.last("POST", "/Users/AuthenticateByName");
    assert_eq!(auth.json()["Username"], USER);
    let header = auth.authorization.unwrap();
    assert!(
        header.starts_with("MediaBrowser Client=\"Bobine\""),
        "{header}"
    );
    assert!(header.contains("DeviceId=\""), "{header}");
    assert!(header.contains("Version=\""), "{header}");
    assert!(!header.contains("Token="), "{header}");
}

#[test]
fn login_with_a_wrong_password_is_refused() {
    let s = Setup::new();
    let refused = s.connector.login(&s.fake.url, USER, "not the password");
    assert_eq!(refused.err(), Some(ProviderError::WrongCredentials));
    assert!(s.tokens().is_empty());
    assert!(s.restart().restore().unwrap().is_none());
}

#[test]
fn the_password_is_kept_nowhere() {
    let s = Setup::new();
    let _server = s.signed_in();
    assert!(!s.account_text().contains(PASSWORD));
    assert!(!s.tokens().iter().any(|t| t.contains(PASSWORD)));
    assert!(
        !s.account_text().contains(TOKEN),
        "the token is only in the keyring"
    );
}

#[test]
fn token_restored_at_start() {
    let s = Setup::new();
    let _ = s.signed_in();
    let server = s.restart().restore().unwrap().expect("a session");
    assert_eq!(server.user_name(), USER);
    assert_eq!(server.user_id(), USER_ID);
    assert_eq!(server.server().name, "Lighthouse");
    server.libraries().unwrap();
    let header = s.fake.last("GET", "/UserViews").authorization.unwrap();
    assert!(header.contains(&format!("Token=\"{TOKEN}\"")), "{header}");
}

#[test]
fn nothing_to_restore_without_a_login() {
    let s = Setup::new();
    assert!(s.connector.restore().unwrap().is_none());
}

#[test]
fn nothing_to_restore_when_the_keyring_lost_the_token() {
    let s = Setup::new();
    let _ = s.signed_in();
    s.store.tokens.lock().unwrap().clear();
    assert!(s.restart().restore().unwrap().is_none());
}

#[test]
fn logout() {
    let s = Setup::new();
    let server = s.signed_in();
    let device = s.account_text();
    server.logout().unwrap();
    let header = s
        .fake
        .last("POST", "/Sessions/Logout")
        .authorization
        .unwrap();
    assert!(header.contains(&format!("Token=\"{TOKEN}\"")));
    assert!(s.tokens().is_empty());
    assert!(s.restart().restore().unwrap().is_none());
    let device_id = |text: &str| {
        text.lines()
            .find(|l| l.starts_with("device_id"))
            .map(String::from)
    };
    assert_eq!(device_id(&s.account_text()), device_id(&device));
}

#[test]
fn logout_without_the_server_still_forgets_the_token() {
    let s = Setup::new();
    let server = s.signed_in();
    drop(s.fake);
    server.logout().unwrap();
    assert!(s.store.tokens.lock().unwrap().is_empty());
}

#[test]
fn signing_out_keeps_the_server_address_for_the_next_login() {
    let s = Setup::new();
    assert_eq!(s.connector.last_address(), None);
    let server = s.signed_in();
    server.logout().unwrap();
    assert_eq!(s.connector.last_address(), Some(s.fake.url.clone()));
    assert_eq!(s.restart().last_address(), Some(s.fake.url.clone()));
    // The address is all the account file keeps of the login.
    let text = s.account_text();
    assert!(!text.contains(USER) && !text.contains(TOKEN), "{text}");
}

#[test]
fn signing_out_while_the_server_is_unreachable_still_signs_out_locally() {
    let s = Setup::new();
    let server = s.signed_in();
    let address = s.fake.url.clone();
    let (path, store) = (s.account_path(), s.store.clone());
    drop(s.fake);
    server.logout().unwrap();
    assert!(store.tokens.lock().unwrap().is_empty());
    let next = JellyfinConnector::open(path, store).unwrap();
    assert!(next.restore().unwrap().is_none());
    assert_eq!(next.last_address(), Some(address));
}

#[test]
fn the_address_of_the_session_signed_in_is_known_before_it_signs_out() {
    let s = Setup::new();
    s.signed_in();
    assert_eq!(s.connector.last_address(), Some(s.fake.url.clone()));
    assert_eq!(s.restart().last_address(), Some(s.fake.url.clone()));
}

#[test]
fn a_keyring_that_cannot_delete_the_token_still_leaves_no_session_to_restore() {
    let s = Setup::new();
    let server = s.signed_in();
    s.store
        .delete_fails
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(server.logout(), Err(ProviderError::Keyring(_))));
    // The token is still in the keyring, but the account file no longer points at it: the next
    // start shows the login instead of signing back in silently.
    let next = s.restart();
    assert!(next.restore().unwrap().is_none());
    assert_eq!(next.last_address(), Some(s.fake.url.clone()));
}

#[test]
fn the_device_id_is_stable_across_starts() {
    let s = Setup::new();
    let _ = s.signed_in();
    let _ = s.restart().restore().unwrap().unwrap().libraries();
    let ids: Vec<String> = s
        .fake
        .seen()
        .iter()
        .filter_map(|r| r.authorization.clone())
        .map(|h| {
            h.split("DeviceId=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert!(ids.len() >= 2);
    assert!(
        ids.iter().all(|id| id == &ids[0] && id.len() == 32),
        "{ids:?}"
    );
}

#[test]
fn a_revoked_token_asks_to_sign_in_again() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().revoked = true;
    assert_eq!(server.libraries(), Err(ProviderError::SessionExpired));
}

#[test]
fn a_refused_item_is_not_an_expired_session() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().forbidden = true;
    let refused = server.film(&film()).unwrap_err();
    assert_eq!(refused, ProviderError::Forbidden);
    assert_ne!(refused, ProviderError::SessionExpired);
    let said = format!("{refused} {refused:?}");
    assert!(!said.contains(&s.fake.url), "{said}");
    assert!(!said.contains(TOKEN), "{said}");
}

#[test]
fn a_disabled_account_is_not_a_wrong_password() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().account_disabled = true;
    let refused = s.connector.login(&s.fake.url, USER, PASSWORD).err();
    assert_eq!(refused, Some(ProviderError::AccountDisabled));
    assert_ne!(
        ProviderError::AccountDisabled.to_string(),
        ProviderError::WrongCredentials.to_string()
    );
    assert!(s.tokens().is_empty());
    assert!(s.restart().restore().unwrap().is_none());
}

#[test]
fn libraries_of_the_user() {
    let s = Setup::new();
    let libraries = s.signed_in().libraries().unwrap();
    let names: Vec<(&str, LibraryKind)> = libraries
        .iter()
        .map(|l| (l.name.as_str(), l.kind))
        .collect();
    assert_eq!(
        names,
        [
            ("Films", LibraryKind::Films),
            ("Series", LibraryKind::Series),
            ("Collections", LibraryKind::Other)
        ]
    );
    assert_eq!(libraries[0].id, ItemId(FILMS_LIBRARY.into()));
    assert_eq!(
        s.fake.last("GET", "/UserViews").param("userId"),
        Some(USER_ID)
    );
}

#[test]
fn films_are_paged() {
    let s = Setup::new();
    let server = s.signed_in();
    let library = ItemId(FILMS_LIBRARY.into());
    let first = server
        .films(&library, &query(FilmSort::Title, false, 0, 2))
        .unwrap();
    let second = server
        .films(&library, &query(FilmSort::Title, false, 2, 2))
        .unwrap();
    let last = server
        .films(&library, &query(FilmSort::Title, false, 4, 2))
        .unwrap();
    assert_eq!(titles(&first), ["Amber Fields", "Copper Sky"]);
    assert_eq!(titles(&second), ["Lanterns", "Northbound"]);
    assert_eq!(titles(&last), ["The Quiet Harbour"]);
    assert_eq!((first.total, last.total), (5, 5));

    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("ParentId"), Some(FILMS_LIBRARY));
    assert_eq!(asked.param("IncludeItemTypes"), Some("Movie"));
    assert_eq!(asked.param("Recursive"), Some("true"));
    assert_eq!(asked.param("StartIndex"), Some("4"));
    assert_eq!(asked.param("Limit"), Some("2"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
}

#[test]
fn films_are_sorted_by_title() {
    let s = Setup::new();
    let server = s.signed_in();
    let library = ItemId(FILMS_LIBRARY.into());
    let page = server
        .films(&library, &query(FilmSort::Title, true, 0, 10))
        .unwrap();
    assert_eq!(
        titles(&page),
        [
            "The Quiet Harbour",
            "Northbound",
            "Lanterns",
            "Copper Sky",
            "Amber Fields"
        ]
    );
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("SortBy"), Some("SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Descending"));
}

#[test]
fn films_are_sorted_by_date_added() {
    let s = Setup::new();
    let server = s.signed_in();
    let library = ItemId(FILMS_LIBRARY.into());
    let page = server
        .films(&library, &query(FilmSort::DateAdded, true, 0, 3))
        .unwrap();
    assert_eq!(
        titles(&page),
        ["Lanterns", "Amber Fields", "The Quiet Harbour"]
    );
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("SortBy"), Some("DateCreated,SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Descending,Ascending"));
}

#[test]
fn films_are_sorted_by_release_year() {
    let s = Setup::new();
    let server = s.signed_in();
    let library = ItemId(FILMS_LIBRARY.into());
    let page = server
        .films(&library, &query(FilmSort::ReleaseYear, false, 0, 10))
        .unwrap();
    let years: Vec<Option<i32>> = page.items.iter().map(|f| f.year).collect();
    assert_eq!(
        years,
        [Some(1987), Some(1998), Some(2003), Some(2011), Some(2020)]
    );
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("SortBy"), Some("ProductionYear,SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Ascending,Ascending"));
}

fn filtered(sort: FilmSort, descending: bool, filters: Filters) -> FilmQuery {
    FilmQuery {
        filters,
        ..query(sort, descending, 0, 10)
    }
}

fn genre(name: &str) -> Filters {
    Filters {
        genre: Some(name.into()),
        ..Filters::default()
    }
}

#[test]
fn films_are_filtered_by_genre() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &filtered(FilmSort::Title, false, genre("Mystery")),
        )
        .unwrap();
    assert_eq!(titles(&page), ["Copper Sky", "The Quiet Harbour"]);
    assert_eq!(page.total, 2);
    assert_eq!(
        s.fake.last("GET", "/Items").param("Genres"),
        Some("Mystery")
    );
}

#[test]
fn films_are_filtered_by_decade() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &filtered(
                FilmSort::Title,
                false,
                Filters {
                    decade: Some(2010),
                    ..Filters::default()
                },
            ),
        )
        .unwrap();
    assert_eq!(titles(&page), ["Amber Fields"]);
    assert_eq!(
        s.fake.last("GET", "/Items").param("Years"),
        Some("2010,2011,2012,2013,2014,2015,2016,2017,2018,2019")
    );
}

#[test]
fn only_the_films_not_watched_are_asked_for() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &filtered(
                FilmSort::Title,
                false,
                Filters {
                    unwatched: true,
                    ..Filters::default()
                },
            ),
        )
        .unwrap();
    assert_eq!(
        titles(&page),
        ["Copper Sky", "Lanterns", "Northbound", "The Quiet Harbour"]
    );
    assert_eq!(page.total, 4);
    assert_eq!(
        s.fake.last("GET", "/Items").param("IsPlayed"),
        Some("false")
    );
}

#[test]
fn no_filter_sends_no_filter_parameter() {
    let s = Setup::new();
    s.signed_in()
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &query(FilmSort::Title, false, 0, 10),
        )
        .unwrap();
    let asked = s.fake.last("GET", "/Items");
    for key in ["Genres", "Years", "IsPlayed", "NameLessThan"] {
        assert_eq!(asked.param(key), None, "{key}");
    }
}

#[test]
fn films_are_sorted_by_rating_best_first() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &filtered(FilmSort::Rating, true, Filters::default()),
        )
        .unwrap();
    // The film nobody rated comes last.
    assert_eq!(
        titles(&page),
        [
            "Copper Sky",
            "The Quiet Harbour",
            "Amber Fields",
            "Lanterns",
            "Northbound"
        ]
    );
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("SortBy"), Some("CommunityRating,SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Descending,Ascending"));
}

#[test]
fn titles_that_tie_in_a_descending_sort_still_read_a_to_z() {
    // "Gale" and "Éclipse" are both from 2016: the newest year first, and between them the
    // titles in their own order, as one order per sorted field asks.
    let s = Setup::new();
    let server = s.signed_in();
    let page = server
        .films(&letters(), &query(FilmSort::ReleaseYear, true, 0, 100))
        .unwrap();
    let years: Vec<Option<i32>> = page.items.iter().map(|f| f.year).collect();
    assert_eq!(years[3..6], [Some(2017), Some(2016), Some(2016)]);
    assert_eq!(titles(&page)[4..6], ["Gale", "Éclipse"]);
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("SortBy"), Some("ProductionYear,SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Descending,Ascending"));
}

#[test]
fn filters_and_sort_apply_together_and_the_total_counts_the_filtered_list() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &FilmQuery {
                limit: 1,
                ..filtered(
                    FilmSort::ReleaseYear,
                    true,
                    Filters {
                        genre: Some("Drama".into()),
                        decade: None,
                        unwatched: true,
                    },
                )
            },
        )
        .unwrap();
    // Drama not watched: Lanterns (2003) and The Quiet Harbour (1998).
    assert_eq!(titles(&page), ["Lanterns"]);
    assert_eq!(page.total, 2);
}

#[test]
fn series_are_filtered_by_genre_decade_and_unwatched() {
    let s = Setup::new();
    let server = s.signed_in();
    let names = |page: &Page<Series>| -> Vec<String> {
        page.items.iter().map(|s| s.title.clone()).collect()
    };
    let drama = server
        .series(
            &series_library(),
            &filtered(FilmSort::Title, false, genre("Drama")),
        )
        .unwrap();
    assert_eq!(names(&drama), ["Bramble Line", "Tidewater"]);
    assert_eq!(s.fake.last("GET", "/Items").param("Genres"), Some("Drama"));
    let twenties = server
        .series(
            &series_library(),
            &filtered(
                FilmSort::Title,
                false,
                Filters {
                    decade: Some(2020),
                    ..Filters::default()
                },
            ),
        )
        .unwrap();
    assert_eq!(names(&twenties), ["Glass Orchard", "Tidewater"]);
    let unwatched = server
        .series(
            &series_library(),
            &filtered(
                FilmSort::Rating,
                true,
                Filters {
                    unwatched: true,
                    ..Filters::default()
                },
            ),
        )
        .unwrap();
    assert_eq!(names(&unwatched), ["Tidewater", "Bramble Line"]);
    assert_eq!(
        s.fake.last("GET", "/Items").param("IsPlayed"),
        Some("false")
    );
}

#[test]
fn a_library_offers_its_genres_and_its_decades() {
    let s = Setup::new();
    let choices = s
        .signed_in()
        .filter_choices(&ItemId(FILMS_LIBRARY.into()))
        .unwrap();
    assert_eq!(choices.genres, ["Drama", "Mystery", "Thriller"]);
    // 1987, 1998, 2003, 2011 and 2020: the latest decade first.
    assert_eq!(choices.decades, [2020, 2010, 2000, 1990, 1980]);
    let genres = s.fake.last("GET", "/Genres");
    assert_eq!(genres.param("ParentId"), Some(FILMS_LIBRARY));
    assert_eq!(genres.param("userId"), Some(USER_ID));
    assert_eq!(
        s.fake.last("GET", "/Items/Filters").param("ParentId"),
        Some(FILMS_LIBRARY)
    );
}

/// The choices of the films library while `path` answers `status`.
fn choices_with(path: &str, status: u16) -> Result<FilterChoices, ProviderError> {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake
        .state
        .lock()
        .unwrap()
        .broken
        .push((path.into(), status));
    server.filter_choices(&ItemId(FILMS_LIBRARY.into()))
}

#[test]
fn the_decades_stay_when_the_genres_cannot_be_had() {
    let choices = choices_with("/Genres", 500).unwrap();
    assert_eq!(choices.genres, Vec::<String>::new());
    assert_eq!(choices.decades, [2020, 2010, 2000, 1990, 1980]);
}

#[test]
fn the_genres_stay_when_the_decades_cannot_be_had() {
    let choices = choices_with("/Items/Filters", 404).unwrap();
    assert_eq!(choices.genres, ["Drama", "Mystery", "Thriller"]);
    assert_eq!(choices.decades, Vec::<i32>::new());
}

#[test]
fn a_lookup_refused_to_the_user_leaves_the_other_one() {
    let choices = choices_with("/Genres", 403).unwrap();
    assert_eq!(choices.decades, [2020, 2010, 2000, 1990, 1980]);
}

#[test]
fn an_expired_session_on_one_lookup_is_the_usual_expired_session() {
    // Not swallowed as a missing lookup, not another error: the one that asks to sign in again.
    assert_eq!(
        choices_with("/Genres", 401),
        Err(ProviderError::SessionExpired)
    );
    assert_eq!(
        choices_with("/Items/Filters", 401),
        Err(ProviderError::SessionExpired)
    );
}

#[test]
fn both_lookups_failing_is_a_failure() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().forbidden = true;
    assert_eq!(
        server.filter_choices(&ItemId(FILMS_LIBRARY.into())),
        Err(ProviderError::Forbidden)
    );
}

fn letters() -> ItemId {
    ItemId(LETTERS_LIBRARY.into())
}

/// The index of the first title of `letter`, as the server counts it.
fn first_of(server: &dyn MediaServer, letter: char, filters: Filters) -> u32 {
    server
        .films(
            &letters(),
            &FilmQuery {
                filters,
                before: Some(letter),
                ..query(FilmSort::Title, false, 0, 1)
            },
        )
        .unwrap()
        .total
}

#[test]
fn the_index_of_a_letter_is_the_count_of_the_titles_before_it() {
    let s = Setup::new();
    let server = s.signed_in();
    // "(untitled)" and "1984 remix" come first, then "amber road", "brume"…
    assert_eq!(first_of(&*server, 'A', Filters::default()), 2);
    assert_eq!(first_of(&*server, 'B', Filters::default()), 3);
    assert_eq!(first_of(&*server, 'G', Filters::default()), 6);
    assert_eq!(first_of(&*server, 'Z', Filters::default()), 7);
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("ParentId"), Some(LETTERS_LIBRARY));
    assert_eq!(asked.param("SortBy"), Some("SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Ascending"));
}

#[test]
fn the_letter_is_sent_in_lowercase_as_sort_names_are_stored() {
    // Jellyfin keeps SortName in lowercase and compares the bytes: "Z" would sort before
    // every one of them.
    let s = Setup::new();
    first_of(&*s.signed_in(), 'Z', Filters::default());
    assert_eq!(
        s.fake.last("GET", "/Items").param("NameLessThan"),
        Some("z")
    );
}

#[test]
fn the_index_of_a_letter_asks_for_one_title_never_for_none() {
    // Jellyfin reads a limit of 0 as no limit: the whole library would come back.
    let s = Setup::new();
    first_of(&*s.signed_in(), 'C', Filters::default());
    assert_eq!(s.fake.last("GET", "/Items").param("Limit"), Some("1"));
}

#[test]
fn the_jump_to_a_letter_matches_the_order_the_server_lists_in() {
    // Whatever the server does with accents, the count and the list are its own: the titles
    // before the index are the ones that sort before the letter, the others are not.
    let s = Setup::new();
    let server = s.signed_in();
    let listed = server
        .films(&letters(), &query(FilmSort::Title, false, 0, 100))
        .unwrap()
        .items;
    for letter in 'A'..='Z' {
        let index = first_of(&*server, letter, Filters::default()) as usize;
        let before = |title: &str| title.to_lowercase() < letter.to_lowercase().to_string();
        assert!(listed[..index].iter().all(|f| before(&f.title)), "{letter}");
        assert!(
            listed[index..].iter().all(|f| !before(&f.title)),
            "{letter}"
        );
    }
}

#[test]
fn an_accented_title_is_wherever_the_server_puts_it() {
    // The fake server keeps accents and compares bytes, as Jellyfin is believed to: "Éclipse"
    // and "Écume" come after "Zèbre", so the jump to E lands on "Eden", not on them. What a
    // real server does is not verified.
    let s = Setup::new();
    let server = s.signed_in();
    let listed = server
        .films(&letters(), &query(FilmSort::Title, false, 0, 100))
        .unwrap()
        .items;
    let titles: Vec<&str> = listed.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(titles[7..], ["Zèbre", "Éclipse", "Écume"]);
    assert_eq!(
        titles[first_of(&*server, 'E', Filters::default()) as usize],
        "Eden"
    );
}

#[test]
fn the_index_of_a_letter_counts_within_the_filtered_list() {
    let s = Setup::new();
    let server = s.signed_in();
    // Drama only: "1984 Remix", "Amber Road", "Gale", "Éclipse", "Écume": two before F.
    assert_eq!(first_of(&*server, 'F', genre("Drama")), 2);
    assert_eq!(s.fake.last("GET", "/Items").param("Genres"), Some("Drama"));
}

#[test]
fn a_film_in_a_list_has_poster_rating_runtime_watched_and_resume() {
    let s = Setup::new();
    let server = s.signed_in();
    let page = server
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &query(FilmSort::Title, false, 0, 10),
        )
        .unwrap();
    let harbour = page.items.iter().find(|f| f.id == film()).unwrap();
    assert_eq!(harbour.year, Some(1998));
    assert_eq!(harbour.community_rating, Some(7.4));
    assert_eq!(harbour.runtime, Some(Duration::from_secs(110 * 60)));
    assert!(!harbour.watched);
    assert_eq!(harbour.resume, Some(Duration::from_secs(20 * 60)));
    assert_eq!(
        harbour.poster,
        Some(ImageRef {
            item: film(),
            kind: ImageKind::Primary,
            tag: "poster-qh".into()
        })
    );
    assert_eq!(harbour.backdrop.as_ref().unwrap().tag, "backdrop-qh");
    let amber = page
        .items
        .iter()
        .find(|f| f.title == "Amber Fields")
        .unwrap();
    assert!(amber.watched);
    assert_eq!(amber.resume, None);
    assert_eq!(amber.backdrop, None);
    let northbound = page.items.iter().find(|f| f.title == "Northbound").unwrap();
    assert_eq!(
        (northbound.poster.clone(), northbound.community_rating),
        (None, None)
    );
}

#[test]
fn what_the_user_started_is_films_and_episodes() {
    let s = Setup::new();
    let started = s.signed_in().continue_watching().unwrap();
    assert_eq!(started.len(), 2);
    let Title::Film(film) = &started[0] else {
        panic!("a film first: {started:?}");
    };
    assert_eq!(film.id, self::film());
    assert_eq!(film.resume, Some(Duration::from_secs(20 * 60)));
    let Title::Episode(episode) = &started[1] else {
        panic!("an episode second: {started:?}");
    };
    assert_eq!(episode.item.id, ItemId(EPISODE.into()));
    assert_eq!(episode.series_title, "Tidewater");
    assert_eq!(episode.item.resume, Some(Duration::from_secs(12 * 60)));
    // An episode played from Home needs its versions, without another request.
    assert_eq!(episode.versions.len(), 1);
    let asked = s.fake.last("GET", "/UserItems/Resume");
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("IncludeItemTypes"), Some("Movie,Episode"));
    assert!(asked.param("Fields").unwrap().contains("MediaSources"));
}

#[test]
fn next_up_across_all_series_is_asked_for_the_user_with_a_limit() {
    let s = Setup::new();
    let next = s.signed_in().next_episodes(12).unwrap();
    let titles: Vec<&str> = next.iter().map(|e| e.item.title.as_str()).collect();
    assert_eq!(titles, ["Low Water", "The Long Table"]);
    assert_eq!(next[1].series_title, "Glass Orchard");
    assert_eq!(next[1].season_number, Some(1));
    assert_eq!(next[1].number, Some(4));
    assert_eq!(next[1].versions.len(), 1);
    let asked = s.fake.last("GET", "/Shows/NextUp");
    assert_eq!(asked.param("SeriesId"), None);
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("Limit"), Some("12"));
    assert_eq!(asked.param("EnableResumable"), Some("true"));
    assert!(asked.param("Fields").unwrap().contains("MediaSources"));
}

#[test]
fn no_series_in_progress_has_no_next_episode() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().all_watched = true;
    assert_eq!(s.signed_in().next_episodes(12).unwrap(), Vec::new());
}

#[test]
fn the_latest_of_a_films_library_are_films() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(FILMS_LIBRARY.into()), 3)
        .unwrap();
    assert_eq!(latest.len(), 3);
    assert!(latest.iter().all(|title| matches!(title, Title::Film(_))));
    let Title::Film(first) = &latest[0] else {
        unreachable!()
    };
    assert_eq!(first.title, "The Quiet Harbour");
    assert_eq!(
        first.poster.as_ref().map(|p| p.tag.as_str()),
        Some("poster-qh")
    );
    let asked = s.fake.last("GET", "/Items/Latest");
    assert_eq!(asked.param("ParentId"), Some(FILMS_LIBRARY));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("Limit"), Some("3"));
}

#[test]
fn a_new_season_in_the_latest_is_listed_as_its_series() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let Title::Series(reach) = &latest[4] else {
        panic!("a series: {:?}", latest[4])
    };
    assert_eq!(reach.title, "Northern Reach");
    assert_eq!(reach.id, ItemId("5e000000000000000000000000000004".into()));
    assert_eq!(
        reach.poster.as_ref().map(|p| (&p.item, p.tag.as_str())),
        Some((&reach.id, "poster-nr"))
    );
    let season = reach.season.as_ref().expect("the season it opens on");
    assert_eq!(season.id, ItemId("5c000000000000000000000000000009".into()));
    assert_eq!(season.number, Some(9));
}

#[test]
fn a_new_season_without_a_series_picture_keeps_its_own_poster() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let Title::Series(lanterns) = &latest[5] else {
        panic!("a series: {:?}", latest[5])
    };
    assert_eq!(lanterns.title, "Paper Lanterns");
    assert_eq!(
        lanterns
            .poster
            .as_ref()
            .map(|p| (&p.item.0[..], p.tag.as_str())),
        Some(("5c000000000000000000000000000002", "season-pl-2"))
    );
}

#[test]
fn a_new_season_card_counts_the_episodes_left_in_that_season() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let Title::Series(reach) = &latest[4] else {
        panic!("a series: {:?}", latest[4])
    };
    assert_eq!(reach.unwatched, 8);
}

#[test]
fn a_lone_episode_in_the_latest_says_nothing_of_its_series_state() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let Title::Series(lights) = &latest[2] else {
        panic!("a series: {:?}", latest[2])
    };
    assert_eq!(lights.title, "Harbour Lights");
    // Whether the series is in the list or watched is not the episode's to say.
    assert_eq!((lights.favourite, lights.watched), (None, None));
    let Title::Series(tidewater) = &latest[0] else {
        panic!("a series: {:?}", latest[0])
    };
    assert!(tidewater.favourite.is_some() && tidewater.watched.is_some());
}

#[test]
fn a_series_and_its_own_new_season_in_the_latest_are_listed_once() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let reach = ItemId("5e000000000000000000000000000004".into());
    assert_eq!(
        latest.iter().filter(|title| title.id() == &reach).count(),
        1
    );
}

#[test]
fn a_season_without_its_series_is_left_out_of_the_latest() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let orphan = ItemId("5c000000000000000000000000000001".into());
    assert!(latest.iter().all(|title| title.id() != &orphan));
    assert_eq!(latest.len(), 6);
}

fn season_item() -> super::dto::Item {
    serde_json::from_value(serde_json::json!({
        "Id": "5c000000000000000000000000000009", "Name": "Season 9", "Type": "Season",
        "IndexNumber": 9, "SeriesId": "5e000000000000000000000000000004",
        "SeriesName": "Northern Reach", "SeriesPrimaryImageTag": "poster-nr",
        "ImageTags": { "Primary": "season-nr-9" }
    }))
    .unwrap()
}

#[test]
fn a_season_handed_as_a_title_is_its_series() {
    let Some(Title::Series(series)) = season_item().title() else {
        panic!("a series")
    };
    assert_eq!(series.id, ItemId("5e000000000000000000000000000004".into()));
    assert_eq!(series.title, "Northern Reach");
    assert_eq!(series.season.map(|season| season.number), Some(Some(9)));
}

#[test]
fn a_series_is_never_built_from_a_season_item() {
    let series = season_item().series();
    assert_eq!(series.id, ItemId("5e000000000000000000000000000004".into()));
    assert_eq!(series.title, "Northern Reach");
    assert_eq!(series.backdrop, None);
}

#[test]
fn the_latest_of_a_series_library_are_series() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 2)
        .unwrap();
    let names: Vec<&str> = latest
        .iter()
        .map(|title| match title {
            Title::Series(series) => series.title.as_str(),
            other => panic!("a series: {other:?}"),
        })
        .collect();
    assert_eq!(names, ["Tidewater", "Glass Orchard"]);
}

#[test]
fn the_latest_asks_for_the_series_picture_of_a_lone_new_episode() {
    let s = Setup::new();
    s.signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    // Without it Jellyfin may leave the series poster out of an episode that comes alone.
    assert_eq!(
        s.fake.last("GET", "/Items/Latest").param("Fields"),
        Some("SeriesPrimaryImage")
    );
}

#[test]
fn a_series_that_gained_one_episode_is_listed_as_the_series() {
    let s = Setup::new();
    let latest = s
        .signed_in()
        .latest(&ItemId(SERIES_LIBRARY.into()), 10)
        .unwrap();
    let names: Vec<&str> = latest
        .iter()
        .map(|title| match title {
            Title::Series(series) => series.title.as_str(),
            other => panic!("a series: {other:?}"),
        })
        .collect();
    // Two episodes of one series come as one series, at the place of the first.
    assert_eq!(
        names,
        [
            "Tidewater",
            "Glass Orchard",
            "Harbour Lights",
            "Bramble Line",
            "Northern Reach",
            "Paper Lanterns"
        ]
    );
    let Title::Series(lights) = &latest[2] else {
        unreachable!()
    };
    assert_eq!(lights.id, ItemId("5e000000000000000000000000000001".into()));
    assert_eq!(
        lights.poster.as_ref().map(|p| (&p.item, p.tag.as_str())),
        Some((&lights.id, "poster-hl"))
    );
}

#[test]
fn the_featured_titles_are_recent_unwatched_ones_with_a_backdrop() {
    let s = Setup::new();
    let featured = s.signed_in().featured(5).unwrap();
    assert_eq!(featured.len(), 3);
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("IncludeItemTypes"), Some("Movie,Series"));
    assert_eq!(asked.param("Recursive"), Some("true"));
    assert_eq!(asked.param("SortBy"), Some("DateCreated"));
    assert_eq!(asked.param("SortOrder"), Some("Descending"));
    assert_eq!(asked.param("IsPlayed"), Some("false"));
    assert_eq!(asked.param("ImageTypes"), Some("Backdrop"));
    assert_eq!(asked.param("Limit"), Some("5"));
    let fields = asked.param("Fields").unwrap();
    assert!(
        fields.contains("Overview") && fields.contains("OfficialRating"),
        "{fields}"
    );
}

#[test]
fn a_featured_film_says_what_the_header_shows() {
    let s = Setup::new();
    let featured = s.signed_in().featured(5).unwrap();
    let film = &featured[0];
    assert_eq!(film.kind, FeatureKind::Film);
    assert_eq!(film.id, self::film());
    assert_eq!(film.title, "The Quiet Harbour");
    assert_eq!(film.year, Some(1998));
    assert_eq!(film.community_rating, Some(7.4));
    assert_eq!(film.official_rating.as_deref(), Some("PG-13"));
    assert_eq!(film.runtime, Some(Duration::from_secs(110 * 60)));
    assert_eq!(film.resume, Some(Duration::from_secs(20 * 60)));
    assert_eq!(
        film.overview.as_deref(),
        Some("A lighthouse keeper finds the tide no longer goes out.")
    );
    assert_eq!(
        film.backdrop.as_ref().map(|b| b.tag.as_str()),
        Some("backdrop-qh")
    );
}

#[test]
fn a_featured_series_is_a_series_with_no_runtime() {
    let s = Setup::new();
    let featured = s.signed_in().featured(5).unwrap();
    let series = &featured[1];
    assert_eq!(series.kind, FeatureKind::Series);
    assert_eq!(series.id, ItemId(SERIES.into()));
    assert_eq!(series.runtime, None);
    assert_eq!(series.resume, None);
    assert_eq!(series.official_rating.as_deref(), Some("TV-MA"));
}

#[test]
fn a_featured_title_has_its_logo_when_the_server_has_one() {
    let s = Setup::new();
    let featured = s.signed_in().featured(5).unwrap();
    assert_eq!(
        featured[0].logo,
        Some(ImageRef {
            item: film(),
            kind: ImageKind::Logo,
            tag: "logo-qh".into(),
        })
    );
    // The title is drawn in text for the ones without.
    assert_eq!(featured[1].logo, None);
    assert_eq!(featured[2].logo, None);
}

#[test]
fn a_logo_is_asked_of_the_server_as_a_logo() {
    let s = Setup::new();
    let logo = ImageRef {
        item: film(),
        kind: ImageKind::Logo,
        tag: "logo-qh".into(),
    };
    assert_eq!(s.signed_in().image(&logo, 600).unwrap(), IMAGE);
    let asked = s.fake.last("GET", &format!("/Items/{FILM}/Images/Logo"));
    assert_eq!(asked.param("tag"), Some("logo-qh"));
    assert_eq!(asked.param("maxWidth"), Some("600"));
}

#[test]
fn detail_with_versions_and_tracks() {
    let s = Setup::new();
    let detail = s.signed_in().film(&film()).unwrap();
    assert_eq!(detail.versions.len(), 2);

    let uhd = &detail.versions[0];
    assert_eq!(uhd.id, MediaSourceId(SOURCE.into()));
    assert_eq!(uhd.name, "The Quiet Harbour - 2160p");
    assert_eq!(uhd.container.as_deref(), Some("mkv"));
    let video = uhd.video.as_ref().unwrap();
    assert_eq!(video.codec.as_deref(), Some("hevc"));
    assert_eq!((video.width, video.height), (Some(3840), Some(2160)));
    assert_eq!(video.range, VideoRange::Hdr10);
    assert!(video.range.is_hdr());

    assert_eq!(
        uhd.audio,
        [
            AudioTrack {
                index: 1,
                language: Some("eng".into()),
                codec: Some("truehd".into()),
                channels: Some(8),
                title: Some("English Atmos".into()),
                default: true,
            },
            AudioTrack {
                index: 2,
                language: Some("fre".into()),
                codec: Some("ac3".into()),
                channels: Some(6),
                title: Some("French - Dolby Digital - 5.1".into()),
                default: false,
            },
        ]
    );
    let subtitles: Vec<(i32, &str, bool, bool, bool)> = uhd
        .subtitles
        .iter()
        .map(|t| {
            (
                t.index,
                t.language.as_deref().unwrap(),
                t.forced,
                t.default,
                t.external,
            )
        })
        .collect();
    assert_eq!(
        subtitles,
        [
            (3, "eng", false, true, false),
            (4, "fre", true, false, false),
            (5, "spa", false, false, true),
            (6, "ger", true, false, true),
        ]
    );
    assert_eq!(uhd.subtitles[1].codec.as_deref(), Some("PGSSUB"));

    let hd = &detail.versions[1];
    assert_eq!(hd.video.as_ref().unwrap().range, VideoRange::Sdr);
    assert!(!hd.video.as_ref().unwrap().range.is_hdr());
    assert!(hd.subtitles.is_empty());
}

#[test]
fn the_server_says_which_tracks_each_version_starts_with_for_the_user() {
    let s = Setup::new();
    let detail = s.signed_in().film(&film()).unwrap();
    assert_eq!(detail.versions[0].default_audio, Some(2));
    assert_eq!(detail.versions[0].default_subtitle, Some(Some(4)));
    // A version the server says nothing of.
    assert_eq!(detail.versions[1].default_audio, None);
    assert_eq!(detail.versions[1].default_subtitle, None);
    // The server chooses for the user it is asked for.
    let asked = s.fake.last("GET", &format!("/Items/{FILM}"));
    assert_eq!(asked.param("userId"), Some(USER_ID));

    let episodes = s
        .signed_in()
        .episodes(&series(), &ItemId(SEASON.into()))
        .unwrap();
    // -1: no subtitles.
    assert_eq!(episodes[0].versions[0].default_audio, Some(1));
    assert_eq!(episodes[0].versions[0].default_subtitle, Some(None));
}

#[test]
fn detail_with_synopsis_genres_people_and_similar() {
    let s = Setup::new();
    let detail = s.signed_in().film(&film()).unwrap();
    assert_eq!(detail.film.title, "The Quiet Harbour");
    assert_eq!(
        detail.overview.as_deref(),
        Some("A lighthouse keeper finds a boat with no crew.")
    );
    assert_eq!(detail.genres, ["Drama", "Mystery"]);
    assert_eq!(detail.official_rating.as_deref(), Some("PG-13"));
    assert_eq!(detail.film.community_rating, Some(7.4));
    assert_eq!(detail.directors, ["Ines Varga"]);
    assert_eq!(
        detail.cast,
        [
            CastMember {
                id: ItemId("p0000000000000000000000000000002".into()),
                name: "Tomas Brell".into(),
                role: Some("Keeper".into()),
                photo: Some(ImageRef {
                    item: ItemId("p0000000000000000000000000000002".into()),
                    kind: ImageKind::Primary,
                    tag: "photo-tb".into(),
                }),
            },
            CastMember {
                id: ItemId("p0000000000000000000000000000003".into()),
                name: "Lena Sato".into(),
                role: None,
                photo: None,
            },
        ]
    );

    let similar: Vec<&str> = detail.similar.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(similar, ["Copper Sky", "Lanterns"]);
    assert_eq!(
        s.fake
            .last("GET", &format!("/Items/{FILM}"))
            .param("userId"),
        Some(USER_ID)
    );
}

const OTHER_FILM: &str = "f0000000000000000000000000000009";

fn boxset_lookups(s: &Setup) -> Vec<Seen> {
    s.fake
        .seen()
        .into_iter()
        .filter(|r| {
            r.path == "/Items"
                && (r.param("includeItemTypes") == Some("BoxSet")
                    || r.param("parentId").is_some_and(|p| p.starts_with("b000")))
        })
        .collect()
}

#[test]
fn the_detail_costs_no_collection_lookup() {
    let s = Setup::new();
    let server = s.signed_in();
    server.film(&film()).unwrap();
    assert!(boxset_lookups(&s).is_empty(), "{:?}", boxset_lookups(&s));
}

#[test]
fn the_collection_of_a_film_with_its_other_films() {
    let s = Setup::new();
    let collection = s
        .signed_in()
        .collection_of(&film())
        .unwrap()
        .expect("a collection");
    assert_eq!(collection.name, "Harbour Trilogy");
    let others: Vec<&str> = collection.others.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(others, ["Return to the Harbour", "Harbour Lights"]);
    assert_eq!(
        collection.others[0].poster.as_ref().map(|p| p.tag.as_str()),
        Some("poster-rh")
    );
}

#[test]
fn the_collection_lookup_asks_each_collection_for_ids_only_and_stops_at_the_first_match() {
    let s = Setup::new();
    s.signed_in().collection_of(&film()).unwrap();
    let asked: Vec<Seen> = boxset_lookups(&s);
    let scanned: Vec<&str> = asked.iter().filter_map(|r| r.param("parentId")).collect();
    // Sea Stories (no match), then Harbour Trilogy (the match, asked once more with its posters).
    // Lone Wolves comes after the match and is never asked.
    assert_eq!(
        scanned,
        [
            "b0000000000000000000000000000001",
            "b0000000000000000000000000000002",
            "b0000000000000000000000000000002",
        ]
    );
    for scan in &asked[1..3] {
        assert_eq!(scan.param("enableImages"), Some("false"), "{scan:?}");
        assert_eq!(scan.param("enableUserData"), Some("false"), "{scan:?}");
        assert!(scan.param("enableImageTypes").is_none(), "{scan:?}");
    }
    assert!(asked[3].param("enableImages").is_none(), "{:?}", asked[3]);
    assert_eq!(asked[3].param("enableImageTypes"), Some("Primary,Backdrop"));
}

#[test]
fn a_film_in_no_collection() {
    let s = Setup::new();
    let alone = s.signed_in().collection_of(&ItemId(OTHER_FILM.into()));
    assert_eq!(alone, Ok(None));
    // Every collection was looked through, none held it.
    assert_eq!(boxset_lookups(&s).len(), 1 + 4);
}

#[test]
fn a_failed_collection_lookup_is_an_error_of_its_own() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().forbidden = true;
    assert_eq!(server.collection_of(&film()), Err(ProviderError::Forbidden));
}

#[test]
fn direct_play_stream() {
    let s = Setup::new();
    let playback = s
        .signed_in()
        .play(&film(), &MediaSourceId(SOURCE.into()))
        .unwrap();
    assert_eq!(playback.session, PlaySessionId(PLAY_SESSION.into()));
    assert_eq!(
        (playback.film.clone(), playback.source.0.as_str()),
        (film(), SOURCE)
    );

    let url = playback.stream.expose_for_player();
    let prefix = format!("{}/Videos/{FILM}/stream?", s.fake.url);
    assert!(url.starts_with(&prefix), "{url}");
    for part in [
        "static=true".to_string(),
        format!("mediaSourceId={SOURCE}"),
        format!("playSessionId={PLAY_SESSION}"),
        format!("ApiKey={TOKEN}"),
    ] {
        assert!(url.contains(&part), "{part} missing");
    }
    assert!(!url.contains("api_key"), "the legacy parameter is not used");

    let asked = s.fake.last("POST", &format!("/Items/{FILM}/PlaybackInfo"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    let body = asked.json();
    assert_eq!(body["MediaSourceId"], SOURCE);
    assert_eq!(body["EnableDirectPlay"], true);
    assert_eq!(body["EnableTranscoding"], false);
}

#[test]
fn external_subtitle_urls() {
    let s = Setup::new();
    let playback = s
        .signed_in()
        .play(&film(), &MediaSourceId(SOURCE.into()))
        .unwrap();
    let urls: Vec<(i32, &str)> = playback
        .external_subtitles
        .iter()
        .map(|t| (t.index, t.url.expose_for_player()))
        .collect();
    let base = format!("{}/Videos/{FILM}/{SOURCE}/Subtitles", s.fake.url);
    assert_eq!(
        urls,
        [
            (5, format!("{base}/5/Stream.srt?ApiKey={TOKEN}").as_str()),
            (6, format!("{base}/6/Stream.ass?ApiKey={TOKEN}").as_str()),
        ]
    );
}

fn report(position: Duration, paused: bool) -> PlaybackReport {
    PlaybackReport {
        session: PlaySessionId(PLAY_SESSION.into()),
        film: film(),
        source: MediaSourceId(SOURCE.into()),
        position,
        paused,
        audio_track: Some(2),
        subtitle_track: None,
        method: PlayMethod::DirectPlay,
    }
}

#[test]
fn start_progress_and_stopped_reports_with_the_right_body() {
    let s = Setup::new();
    let server = s.signed_in();
    server
        .report_started(&report(Duration::from_secs(1200), false))
        .unwrap();
    server
        .report_progress(&report(Duration::from_millis(1_210_500), true))
        .unwrap();
    server
        .report_stopped(&report(Duration::from_secs(1300), false))
        .unwrap();

    let started = s.fake.last("POST", "/Sessions/Playing").json();
    assert_eq!(started["ItemId"], FILM);
    assert_eq!(started["MediaSourceId"], SOURCE);
    assert_eq!(started["PlaySessionId"], PLAY_SESSION);
    assert_eq!(started["PositionTicks"], 12_000_000_000_i64);
    assert_eq!(started["IsPaused"], false);
    assert_eq!(started["CanSeek"], true);
    assert_eq!(started["PlayMethod"], "DirectPlay");
    assert_eq!(started["AudioStreamIndex"], 2);
    assert!(started["SubtitleStreamIndex"].is_null());

    let progress = s.fake.last("POST", "/Sessions/Playing/Progress").json();
    assert_eq!(progress["PositionTicks"], 12_105_000_000_i64);
    assert_eq!(progress["IsPaused"], true);
    assert_eq!(progress["PlaySessionId"], PLAY_SESSION);
    assert_eq!(progress["PlayMethod"], "DirectPlay");

    let stopped = s.fake.last("POST", "/Sessions/Playing/Stopped").json();
    assert_eq!(stopped["ItemId"], FILM);
    assert_eq!(stopped["MediaSourceId"], SOURCE);
    assert_eq!(stopped["PlaySessionId"], PLAY_SESSION);
    assert_eq!(stopped["PositionTicks"], 13_000_000_000_i64);
    assert!(stopped.get("PlayMethod").is_none(), "{stopped}");

    for path in [
        "/Sessions/Playing",
        "/Sessions/Playing/Progress",
        "/Sessions/Playing/Stopped",
    ] {
        let header = s.fake.last("POST", path).authorization.unwrap();
        assert!(header.contains(&format!("Token=\"{TOKEN}\"")), "{path}");
    }
}

/// The device id the client sends in its authorization header.
fn device_id(s: &Setup) -> String {
    let header = s
        .fake
        .last("POST", "/Sessions/Playing/Stopped")
        .authorization
        .unwrap();
    header
        .split("DeviceId=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_string()
}

#[test]
fn the_first_play_asks_as_before_without_a_device_profile() {
    let s = Setup::new();
    s.signed_in()
        .play(&film(), &MediaSourceId(SOURCE.into()))
        .unwrap();
    let body = s
        .fake
        .last("POST", &format!("/Items/{FILM}/PlaybackInfo"))
        .json();
    assert_eq!(body["EnableDirectPlay"], true);
    assert_eq!(body["EnableDirectStream"], false);
    assert_eq!(body["EnableTranscoding"], false);
    assert_eq!(body["AutoOpenLiveStream"], false);
    assert!(body.get("DeviceProfile").is_none(), "{body}");
    assert!(body.get("MaxStreamingBitrate").is_none(), "{body}");
}

#[test]
fn the_first_play_plays_the_file_whatever_the_server_says() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().transcode_only = true;
    let playback = s
        .signed_in()
        .play(&film(), &MediaSourceId(SOURCE.into()))
        .unwrap();
    let url = playback.stream.expose_for_player();
    let prefix = format!("{}/Videos/{FILM}/stream?static=true&", s.fake.url);
    assert!(url.starts_with(&prefix), "{url}");
}

#[test]
fn transcoding_asks_the_server_without_direct_play_from_the_position() {
    let s = Setup::new();
    let playback = s
        .signed_in()
        .transcode(&TranscodeRequest {
            film: film(),
            source: MediaSourceId(SOURCE.into()),
            start: Duration::from_secs(1200),
            audio_track: Some(2),
            subtitle_track: Some(4),
            max_bitrate: None,
        })
        .unwrap();
    assert_eq!(playback.session, PlaySessionId(TRANSCODE_SESSION.into()));
    assert_eq!(
        (playback.film.clone(), playback.source.0.as_str()),
        (film(), SOURCE)
    );
    let url = playback.stream.expose_for_player();
    assert!(
        url.starts_with(&format!("{}/videos/{FILM}/master.m3u8?", s.fake.url)),
        "{url}"
    );
    // The external subtitles still come as files beside the converted picture.
    assert_eq!(playback.external_subtitles.len(), 2);

    let asked = s.fake.last("POST", &format!("/Items/{FILM}/PlaybackInfo"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    let body = asked.json();
    assert_eq!(body["MediaSourceId"], SOURCE);
    assert_eq!(body["EnableDirectPlay"], false);
    assert_eq!(body["EnableDirectStream"], false);
    assert_eq!(body["EnableTranscoding"], true);
    assert_eq!(body["StartTimeTicks"], 12_000_000_000_i64);
    assert_eq!(body["AudioStreamIndex"], 2);
    assert_eq!(body["SubtitleStreamIndex"], 4);
}

fn transcoded(s: &Setup, subtitle_track: Option<i32>) -> Playback {
    s.signed_in()
        .transcode(&TranscodeRequest {
            film: film(),
            source: MediaSourceId(SOURCE.into()),
            start: Duration::ZERO,
            audio_track: None,
            subtitle_track,
            max_bitrate: None,
        })
        .unwrap()
}

#[test]
fn an_embedded_text_subtitle_chosen_comes_as_a_file_beside_the_conversion() {
    let s = Setup::new();
    let playback = transcoded(&s, Some(3));
    let indexes: Vec<i32> = playback
        .external_subtitles
        .iter()
        .map(|e| e.index)
        .collect();
    assert_eq!(indexes, [5, 6, 3]);
    let url = playback.external_subtitles[2].url.expose_for_player();
    assert_eq!(
        url,
        format!(
            "{}/Videos/{FILM}/{SOURCE}/Subtitles/3/0/Stream.srt?ApiKey={TOKEN}",
            s.fake.url
        )
    );
    // Drawn into the picture, or not chosen: no file.
    assert_eq!(transcoded(&s, Some(4)).external_subtitles.len(), 2);
    assert_eq!(transcoded(&s, None).external_subtitles.len(), 2);
}

#[test]
fn a_quality_cap_asks_the_server_for_that_bitrate() {
    let s = Setup::new();
    s.signed_in()
        .transcode(&TranscodeRequest {
            film: film(),
            source: MediaSourceId(SOURCE.into()),
            start: Duration::ZERO,
            audio_track: None,
            subtitle_track: None,
            max_bitrate: Some(4_000_000),
        })
        .unwrap();
    let body = s
        .fake
        .last("POST", &format!("/Items/{FILM}/PlaybackInfo"))
        .json();
    assert_eq!(body["MaxStreamingBitrate"], 4_000_000);
    assert_eq!(body["DeviceProfile"]["MaxStreamingBitrate"], 4_000_000);
}

#[test]
fn the_transcode_request_carries_the_device_profile_with_the_embedded_subtitles() {
    let s = Setup::new();
    s.signed_in()
        .transcode(&TranscodeRequest {
            film: film(),
            source: MediaSourceId(SOURCE.into()),
            start: Duration::ZERO,
            audio_track: None,
            subtitle_track: None,
            max_bitrate: None,
        })
        .unwrap();
    let body = s
        .fake
        .last("POST", &format!("/Items/{FILM}/PlaybackInfo"))
        .json();
    assert!(body["MaxStreamingBitrate"].as_u64().unwrap() >= 400_000_000);
    let profile = &body["DeviceProfile"];
    let transcoding = &profile["TranscodingProfiles"][0];
    assert_eq!(transcoding["Type"], "Video");
    assert_eq!(transcoding["Container"], "ts");
    assert_eq!(transcoding["Protocol"], "hls");
    assert_eq!(transcoding["VideoCodec"], "h264");
    assert_eq!(transcoding["AudioCodec"], "aac");
    let subtitles = profile["SubtitleProfiles"].as_array().unwrap();
    for format in [
        "subrip", "srt", "ass", "ssa", "webvtt", "vtt", "mov_text", "pgssub", "dvdsub", "dvbsub",
    ] {
        assert!(
            subtitles
                .iter()
                .any(|p| p["Format"] == format && p["Method"] == "Embed"),
            "{format} is not embedded"
        );
    }
}

#[test]
fn an_absolute_delivery_url_is_used_as_it_is_without_a_token() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().absolute_delivery = true;
    let playback = transcoded(&s, Some(3));
    assert_eq!(
        playback.external_subtitles[2].url.expose_for_player(),
        "https://subtitles.example.test/3.srt?lang=en"
    );
}

#[test]
fn only_an_http_or_https_address_is_absolute() {
    use super::is_absolute;
    for absolute in [
        "http://subtitles.example.test/3.srt",
        "https://subtitles.example.test/3.srt",
        "HTTPS://subtitles.example.test/3.srt",
        "Http://subtitles.example.test/3.srt",
    ] {
        assert!(is_absolute(absolute), "{absolute}");
    }
    for relative in [
        "/Videos/1/Subtitles/3/0/Stream.srt",
        "httpd/Stream.srt",
        "http-stream.srt",
        "https",
        "ftp://subtitles.example.test/3.srt",
        "",
    ] {
        assert!(!is_absolute(relative), "{relative}");
    }
}

#[test]
fn a_version_the_server_cannot_convert_says_so() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().no_transcoding = true;
    let error = s
        .signed_in()
        .transcode(&TranscodeRequest {
            film: film(),
            source: MediaSourceId(SOURCE.into()),
            start: Duration::ZERO,
            audio_track: None,
            subtitle_track: None,
            max_bitrate: None,
        })
        .unwrap_err();
    assert!(matches!(error, ProviderError::BadAnswer(_)), "{error:?}");
}

#[test]
fn the_play_method_is_reported_while_playing_and_not_when_stopped() {
    let s = Setup::new();
    let server = s.signed_in();
    let transcoded = PlaybackReport {
        method: PlayMethod::Transcode,
        ..report(Duration::from_secs(1200), false)
    };
    server.report_started(&transcoded).unwrap();
    server.report_progress(&transcoded).unwrap();
    server.report_stopped(&transcoded).unwrap();
    for path in ["/Sessions/Playing", "/Sessions/Playing/Progress"] {
        let body = s.fake.last("POST", path).json();
        assert_eq!(body["PlayMethod"], "Transcode", "{path}");
        assert_eq!(body["PlaySessionId"], PLAY_SESSION, "{path}");
    }
    let stopped = s.fake.last("POST", "/Sessions/Playing/Stopped").json();
    assert_eq!(stopped["PlaySessionId"], PLAY_SESSION);
    assert!(stopped.get("PlayMethod").is_none(), "{stopped}");
}

#[test]
fn the_encoding_is_stopped_when_a_transcode_stops() {
    let s = Setup::new();
    let server = s.signed_in();
    server
        .report_stopped(&PlaybackReport {
            method: PlayMethod::Transcode,
            ..report(Duration::from_secs(1300), false)
        })
        .unwrap();
    let seen = s.fake.seen();
    let stopped = seen
        .iter()
        .position(|r| r.path == "/Sessions/Playing/Stopped")
        .unwrap();
    let deleted = seen
        .iter()
        .position(|r| r.method == "DELETE" && r.path == "/Videos/ActiveEncodings")
        .expect("the encoding is stopped");
    assert!(stopped < deleted, "told stopped first");
    let delete = &seen[deleted];
    assert_eq!(delete.param("playSessionId"), Some(PLAY_SESSION));
    assert_eq!(delete.param("deviceId"), Some(device_id(&s).as_str()));
}

#[test]
fn a_direct_play_stop_has_no_encoding_to_stop() {
    let s = Setup::new();
    let server = s.signed_in();
    server
        .report_stopped(&report(Duration::from_secs(1300), false))
        .unwrap();
    assert!(
        !s.fake
            .seen()
            .iter()
            .any(|r| r.path == "/Videos/ActiveEncodings")
    );
}

#[test]
fn watched_and_unwatched() {
    let s = Setup::new();
    let server = s.signed_in();
    server.set_watched(&film(), true).unwrap();
    let path = format!("/UserPlayedItems/{FILM}");
    assert_eq!(s.fake.last("POST", &path).param("userId"), Some(USER_ID));
    server.set_watched(&film(), false).unwrap();
    assert_eq!(s.fake.last("DELETE", &path).param("userId"), Some(USER_ID));
}

#[test]
fn a_series_is_marked_watched_as_a_whole() {
    let s = Setup::new();
    let server = s.signed_in();
    server.set_watched(&series(), true).unwrap();
    let path = format!("/UserPlayedItems/{SERIES}");
    assert_eq!(s.fake.last("POST", &path).param("userId"), Some(USER_ID));
    server.set_watched(&series(), false).unwrap();
    assert_eq!(s.fake.last("DELETE", &path).param("userId"), Some(USER_ID));
}

#[test]
fn image_at_the_requested_size() {
    let s = Setup::new();
    let server = s.signed_in();
    let poster = ImageRef {
        item: film(),
        kind: ImageKind::Primary,
        tag: "poster-qh".into(),
    };
    assert_eq!(server.image(&poster, 300).unwrap(), IMAGE);
    let asked = s.fake.last("GET", &format!("/Items/{FILM}/Images/Primary"));
    assert_eq!(asked.param("maxWidth"), Some("300"));
    assert_eq!(asked.param("tag"), Some("poster-qh"));

    let backdrop = ImageRef {
        kind: ImageKind::Backdrop,
        tag: "backdrop-qh".into(),
        ..poster
    };
    server.image(&backdrop, 1280).unwrap();
    let asked = s
        .fake
        .last("GET", &format!("/Items/{FILM}/Images/Backdrop"));
    assert_eq!(asked.param("maxWidth"), Some("1280"));
}

fn found_titles(results: &SearchResults) -> (Vec<&str>, Vec<&str>, Vec<&str>) {
    (
        results.films.iter().map(|f| f.title.as_str()).collect(),
        results.series.iter().map(|s| s.title.as_str()).collect(),
        results.people.iter().map(|p| p.name.as_str()).collect(),
    )
}

#[test]
fn a_search_finds_films_series_and_people_in_their_own_groups() {
    let s = Setup::new();
    let found = s.signed_in().search("harbour", 5).unwrap();
    assert_eq!(
        found_titles(&found),
        (
            vec!["The Quiet Harbour"],
            vec!["Harbour Lights", "Quiet Harbour Chronicles"],
            vec!["Ivo Harbourne"]
        )
    );
    assert_eq!(found.films[0].year, Some(1998));
    assert_eq!(found.films[0].poster.as_ref().unwrap().tag, "poster-qh");
    assert_eq!(found.series[0].year, Some(2016));
    assert_eq!(
        found.series[0].poster,
        Some(ImageRef {
            item: ItemId("5e000000000000000000000000000001".into()),
            kind: ImageKind::Primary,
            tag: "poster-hl".into()
        })
    );
    // A person without a photo on the server has none here.
    assert_eq!(found.people[0].photo, None);
}

#[test]
fn a_person_found_has_a_photo_when_the_server_has_one() {
    let s = Setup::new();
    let found = s.signed_in().search("mara", 5).unwrap();
    assert_eq!(found.people.len(), 1);
    assert_eq!(
        found.people[0].id,
        ItemId("9e000000000000000000000000000003".into())
    );
    assert_eq!(found.people[0].photo.as_ref().unwrap().tag, "photo-mq");
}

#[test]
fn a_search_is_limited_per_group() {
    let s = Setup::new();
    let found = s.signed_in().search("a", 1).unwrap();
    assert_eq!(
        (found.films.len(), found.series.len(), found.people.len()),
        (1, 1, 1)
    );
    for (path, types) in [("/Items", "Movie"), ("/Items", "Series"), ("/Persons", "")] {
        let asked = s
            .fake
            .seen()
            .into_iter()
            .rev()
            .find(|r| {
                r.path == path && (types.is_empty() || r.param("IncludeItemTypes") == Some(types))
            })
            .unwrap_or_else(|| panic!("no {path} {types}"));
        assert_eq!(asked.param("Limit"), Some("1"), "{path} {types}");
    }
}

#[test]
fn a_search_asks_by_term_for_the_user() {
    let s = Setup::new();
    s.signed_in().search("harbour", 5).unwrap();
    let films = s
        .fake
        .seen()
        .into_iter()
        .find(|r| r.param("IncludeItemTypes") == Some("Movie"))
        .unwrap();
    assert_eq!(films.param("SearchTerm"), Some("harbour"));
    assert_eq!(films.param("userId"), Some(USER_ID));
    assert_eq!(films.param("Recursive"), Some("true"));
    assert_eq!(
        s.fake.last("GET", "/Persons").param("userId"),
        Some(USER_ID)
    );
}

#[test]
fn a_search_with_no_match_finds_nothing() {
    let s = Setup::new();
    let found = s.signed_in().search("zzzz", 5).unwrap();
    assert_eq!(found, SearchResults::default());
}

#[test]
fn the_term_reaches_the_server_as_typed() {
    let s = Setup::new();
    let term = "l'été & co/+ %20";
    s.signed_in().search(term, 5).unwrap();
    assert_eq!(
        s.fake.last("GET", "/Persons").param("SearchTerm"),
        Some(term)
    );
}

#[test]
fn a_search_with_a_revoked_token_asks_to_sign_in_again() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().revoked = true;
    assert_eq!(
        server.search("harbour", 5).unwrap_err(),
        ProviderError::SessionExpired
    );
}

#[test]
fn what_a_person_took_part_in_is_split_into_films_and_series() {
    let s = Setup::new();
    let person = ItemId("9e000000000000000000000000000001".into());
    let found = s.signed_in().person_titles(&person).unwrap();
    let films: Vec<_> = found.films.iter().map(|f| f.title.as_str()).collect();
    let series: Vec<_> = found.series.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(films.len(), 4);
    assert_eq!(series.len(), 2);
    let asked = s
        .fake
        .seen()
        .into_iter()
        .find(|r| r.param("PersonIds").is_some())
        .unwrap();
    assert_eq!(asked.param("PersonIds"), Some(person.0.as_str()));
    assert_eq!(asked.param("Recursive"), Some("true"));
}

#[test]
fn titles_of_the_same_year_read_a_to_z() {
    let s = Setup::new();
    let person = ItemId("9e000000000000000000000000000001".into());
    let found = s.signed_in().person_titles(&person).unwrap();
    let films: Vec<_> = found.films.iter().map(|f| f.title.as_str()).collect();
    let series: Vec<_> = found.series.iter().map(|f| f.title.as_str()).collect();
    // Newest year first; within a year A to Z whatever the case; no year last.
    assert_eq!(
        films,
        [
            "amber Coast",
            "The Quiet Harbour",
            "Copper Sky",
            "Undated Reel"
        ]
    );
    assert_eq!(series, ["Dockside", "Harbour Lights"]);
}

#[test]
fn a_person_with_many_titles_is_asked_by_pages() {
    let s = Setup::new();
    let person = ItemId(PERSON_WITH_MANY_TITLES.into());
    let found = s.signed_in().person_titles(&person).unwrap();
    assert_eq!(found.films.len(), 250, "every title of the person comes");
    let asked: Vec<_> = s
        .fake
        .seen()
        .into_iter()
        .filter(|r| r.param("PersonIds").is_some())
        .collect();
    assert!(asked.len() >= 3, "{} requests", asked.len());
    let starts: Vec<_> = asked.iter().map(|r| r.param("StartIndex")).collect();
    assert_eq!(starts[..3], [Some("0"), Some("100"), Some("200")]);
    for r in &asked {
        let limit: u32 = r
            .param("Limit")
            .expect("a limit on every page")
            .parse()
            .unwrap();
        assert!(limit <= 100, "page of {limit}");
    }
}

#[test]
fn a_person_with_endless_titles_stops_at_the_cap() {
    let s = Setup::new();
    let person = ItemId(PERSON_WITH_ENDLESS_TITLES.into());
    let found = s.signed_in().person_titles(&person).unwrap();
    assert_eq!(found.films.len(), 1000);
    let asked = s
        .fake
        .seen()
        .into_iter()
        .filter(|r| r.param("PersonIds").is_some())
        .count();
    assert_eq!(asked, 10);
}

#[test]
fn a_persons_series_says_whether_it_is_in_the_list_and_watched_unless_found_by_an_episode() {
    let s = Setup::new();
    let state = |person: &str| {
        let found = s.signed_in().person_titles(&ItemId(person.into())).unwrap();
        found
            .series
            .into_iter()
            .map(|x| (x.title, x.favourite, x.watched))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        state("9e000000000000000000000000000001"),
        [
            ("Dockside".to_string(), Some(true), Some(true)),
            ("Harbour Lights".to_string(), Some(false), Some(false)),
        ]
    );
    // An episode says nothing of its series.
    assert!(
        state(PERSON_IN_EPISODES)
            .iter()
            .all(|(_, favourite, watched)| favourite.is_none() && watched.is_none())
    );
}

#[test]
fn a_person_seen_only_in_episodes_has_their_series() {
    let s = Setup::new();
    let person = ItemId(PERSON_IN_EPISODES.into());
    let found = s.signed_in().person_titles(&person).unwrap();
    assert!(found.films.is_empty());
    // One entry per series, however many of its episodes the person is in; the poster is the
    // series', not the episode's.
    let series: Vec<_> = found
        .series
        .iter()
        .map(|x| (x.id.0.as_str(), x.title.as_str()))
        .collect();
    assert_eq!(
        series,
        [
            ("5e000000000000000000000000000001", "Harbour Lights"),
            ("5e000000000000000000000000000003", "Northern Tide"),
        ]
    );
    let poster = found.series[0].poster.as_ref().unwrap();
    assert_eq!(poster.item.0, "5e000000000000000000000000000001");
    assert_eq!(poster.tag, "poster-hl");
    assert!(found.series[1].poster.is_none());
    let asked = s
        .fake
        .seen()
        .into_iter()
        .find(|r| r.param("PersonIds").is_some())
        .unwrap();
    assert!(
        asked
            .param("IncludeItemTypes")
            .unwrap()
            .split(',')
            .any(|t| t == "Episode")
    );
}

#[test]
fn a_series_seen_both_whole_and_in_episodes_counts_once() {
    let s = Setup::new();
    let person = ItemId("9e000000000000000000000000000001".into());
    let found = s.signed_in().person_titles(&person).unwrap();
    let lights: Vec<_> = found
        .series
        .iter()
        .filter(|x| x.title == "Harbour Lights")
        .collect();
    assert_eq!(lights.len(), 1);
    assert_eq!(lights[0].year, Some(2016), "the series' own entry is kept");
}

#[test]
fn the_three_groups_are_asked_at_the_same_time() {
    let s = Setup::new();
    let server = s.signed_in();
    let alone = server.search("harbour", 5).unwrap();
    s.fake.state.lock().unwrap().search_gate = 3;
    let found = server.search("harbour", 5).unwrap();
    // The fake holds each request until the three have arrived: sent one after another, the
    // first would be let go alone.
    assert_eq!(s.fake.state.lock().unwrap().gate_met, [true, true, true]);
    assert_eq!(found, alone);
    assert!(!found.films.is_empty() && !found.series.is_empty() && !found.people.is_empty());
}

#[test]
fn a_search_still_fails_whole_when_one_group_fails() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().forbidden = true;
    assert!(server.search("harbour", 5).is_err());
}

#[test]
fn the_token_never_in_debug_or_error_output() {
    let s = Setup::new();
    let server = s.signed_in();
    let playback = server.play(&film(), &MediaSourceId(SOURCE.into())).unwrap();
    let detail = server.film(&film()).unwrap();
    let mut shown = format!("{playback:?} {playback:#?} {detail:?}");
    let transcoded = server
        .transcode(&TranscodeRequest {
            film: film(),
            source: MediaSourceId(SOURCE.into()),
            start: Duration::ZERO,
            audio_track: None,
            subtitle_track: None,
            max_bitrate: None,
        })
        .unwrap();
    shown += &format!("{transcoded:?} {transcoded:#?}");

    s.fake.state.lock().unwrap().revoked = true;
    let expired = server.libraries().unwrap_err();
    shown += &format!("{expired} {expired:?}");

    // A new start, so no connection to the stopped server is left open to wait on.
    let address = s.fake.url.clone();
    let server = s.restart().restore().unwrap().unwrap();
    drop(s.fake);
    for e in [
        server.libraries().unwrap_err(),
        server
            .play(&film(), &MediaSourceId(SOURCE.into()))
            .unwrap_err(),
        server
            .image(&detail.film.poster.clone().unwrap(), 300)
            .unwrap_err(),
        s.connector.login(&address, USER, PASSWORD).err().unwrap(),
    ] {
        assert!(matches!(e, ProviderError::Unreachable(_)), "{e:?}");
        shown += &format!("{e} {e:?}");
    }

    assert!(!shown.contains(TOKEN), "{shown}");
    assert!(!shown.contains(PASSWORD), "{shown}");
    assert!(
        !shown.contains(&address),
        "the address stays out of errors: {shown}"
    );
}

fn series_library() -> ItemId {
    ItemId(SERIES_LIBRARY.into())
}

fn series() -> ItemId {
    ItemId(SERIES.into())
}

#[test]
fn series_of_a_library_are_paged_and_sorted_like_films() {
    let s = Setup::new();
    let server = s.signed_in();
    let first = server
        .series(&series_library(), &query(FilmSort::Title, false, 0, 2))
        .unwrap();
    let rest = server
        .series(&series_library(), &query(FilmSort::Title, false, 2, 2))
        .unwrap();
    let names = |page: &Page<Series>| -> Vec<String> {
        page.items.iter().map(|s| s.title.clone()).collect()
    };
    assert_eq!(names(&first), ["Bramble Line", "Glass Orchard"]);
    assert_eq!(names(&rest), ["Tidewater"]);
    assert_eq!(first.total, 3);
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("ParentId"), Some(SERIES_LIBRARY));
    assert_eq!(asked.param("IncludeItemTypes"), Some("Series"));
    assert_eq!(asked.param("Recursive"), Some("true"));
    assert_eq!(asked.param("SortBy"), Some("SortName"));
    assert_eq!(asked.param("StartIndex"), Some("2"));
    assert_eq!(asked.param("userId"), Some(USER_ID));

    let newest = server
        .series(&series_library(), &query(FilmSort::DateAdded, true, 0, 10))
        .unwrap();
    assert_eq!(
        names(&newest),
        ["Tidewater", "Glass Orchard", "Bramble Line"]
    );
}

#[test]
fn series_can_be_sorted_by_when_their_latest_episode_came_in() {
    let s = Setup::new();
    s.signed_in()
        .series(&series_library(), &query(FilmSort::LastAdded, true, 0, 10))
        .unwrap();
    let asked = s.fake.last("GET", "/Items");
    assert_eq!(asked.param("SortBy"), Some("DateLastContentAdded,SortName"));
    assert_eq!(asked.param("SortOrder"), Some("Descending,Ascending"));
}

#[test]
fn a_title_in_a_list_says_whether_it_is_in_the_users_list() {
    let s = Setup::new();
    let server = s.signed_in();
    let films = server
        .films(
            &ItemId(FILMS_LIBRARY.into()),
            &query(FilmSort::Title, false, 0, 10),
        )
        .unwrap();
    let favourite = |title: &str| {
        films
            .items
            .iter()
            .find(|f| f.title == title)
            .unwrap()
            .favourite
    };
    assert!(favourite("Amber Fields"));
    assert!(!favourite("The Quiet Harbour"));
    let shows = server
        .series(&series_library(), &query(FilmSort::Title, false, 0, 10))
        .unwrap();
    let tidewater = shows.items.iter().find(|s| s.id == series()).unwrap();
    assert_eq!(tidewater.favourite, Some(true));
    assert_eq!(shows.items[1].favourite, Some(false));
}

#[test]
fn a_series_in_a_list_says_whether_all_of_it_is_watched() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .series(&series_library(), &query(FilmSort::Title, false, 0, 10))
        .unwrap();
    let watched = |title: &str| {
        page.items
            .iter()
            .find(|s| s.title == title)
            .unwrap()
            .watched
    };
    assert_eq!(watched("Glass Orchard"), Some(true));
    assert_eq!(watched("Tidewater"), Some(false));
}

#[test]
fn a_series_in_a_list_has_its_years_rating_poster_and_unwatched_count() {
    let s = Setup::new();
    let page = s
        .signed_in()
        .series(&series_library(), &query(FilmSort::Title, false, 0, 10))
        .unwrap();
    let tidewater = page.items.iter().find(|s| s.id == series()).unwrap();
    assert_eq!(
        (tidewater.year, tidewater.end_year),
        (Some(2021), Some(2024))
    );
    assert_eq!(tidewater.community_rating, Some(8.4));
    assert_eq!(tidewater.unwatched, 7);
    assert_eq!(tidewater.poster.as_ref().unwrap().tag, "poster-tw");
    assert_eq!(tidewater.backdrop.as_ref().unwrap().tag, "backdrop-tw");
    let orchard = &page.items[1];
    assert_eq!((orchard.end_year, orchard.unwatched), (None, 0));
    let bramble = &page.items[0];
    assert_eq!((bramble.poster.clone(), bramble.unwatched), (None, 0));
}

#[test]
fn series_detail_with_synopsis_genres_cast_and_similar_series() {
    let s = Setup::new();
    let detail = s.signed_in().series_detail(&series()).unwrap();
    assert_eq!(detail.series.title, "Tidewater");
    assert_eq!(
        (detail.series.year, detail.series.end_year),
        (Some(2021), Some(2024))
    );
    assert_eq!(
        detail.overview.as_deref(),
        Some("A ferry crew keeps a drowned town's secrets.")
    );
    assert_eq!(detail.genres, ["Drama", "Mystery"]);
    assert_eq!(detail.official_rating.as_deref(), Some("TV-14"));
    let cast: Vec<(&str, Option<&str>)> = detail
        .cast
        .iter()
        .map(|p| (p.name.as_str(), p.role.as_deref()))
        .collect();
    assert_eq!(
        cast,
        [
            ("Mara Quill", Some("Captain Rook")),
            ("Jon Ashby", Some("Deckhand"))
        ]
    );
    assert_eq!(detail.similar.len(), 1);
    assert_eq!(detail.similar[0].title, "Bramble Line");
    assert_eq!(detail.similar[0].unwatched, 10);
    let asked = s.fake.last("GET", &format!("/Items/{SERIES}/Similar"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
}

#[test]
fn the_seasons_of_a_series_in_order() {
    let s = Setup::new();
    let seasons = s.signed_in().seasons(&series()).unwrap();
    let found: Vec<(&str, Option<u32>, u32)> = seasons
        .iter()
        .map(|s| (s.name.as_str(), s.number, s.unwatched))
        .collect();
    assert_eq!(found, [("Season 1", Some(1), 0), ("Season 2", Some(2), 7)]);
    assert_eq!(seasons[1].id, ItemId(SEASON.into()));
    let asked = s.fake.last("GET", &format!("/Shows/{SERIES}/Seasons"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    // A season the library only knows by name, without a file, is not offered.
    assert_eq!(asked.param("isMissing"), Some("false"));
}

#[test]
fn the_episodes_of_a_season_with_still_runtime_state_and_versions() {
    let s = Setup::new();
    let episodes = s
        .signed_in()
        .episodes(&series(), &ItemId(SEASON.into()))
        .unwrap();
    assert_eq!(episodes.len(), 2);
    let first = &episodes[0];
    assert_eq!(first.item.id, ItemId(EPISODE.into()));
    assert_eq!(first.item.title, "Low Water");
    assert_eq!((first.season_number, first.number), (Some(2), Some(1)));
    assert_eq!(first.season, ItemId(SEASON.into()));
    assert_eq!(
        (first.series.clone(), first.series_title.as_str()),
        (series(), "Tidewater")
    );
    assert_eq!(
        first.overview.as_deref(),
        Some("The ferry runs aground on a road that should not be there.")
    );
    assert_eq!(first.item.runtime, Some(Duration::from_secs(52 * 60)));
    assert_eq!(first.item.resume, Some(Duration::from_secs(12 * 60)));
    assert!(!first.item.watched);
    assert_eq!(first.item.poster.as_ref().unwrap().tag, "still-lw");
    assert_eq!(first.versions.len(), 1);
    assert_eq!(first.versions[0].audio[0].language.as_deref(), Some("eng"));
    assert!(first.versions[0].subtitles[0].external);
    assert_eq!(episodes[1].item.poster, None);

    let asked = s.fake.last("GET", &format!("/Shows/{SERIES}/Episodes"));
    assert_eq!(asked.param("SeasonId"), Some(SEASON));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    // Nor is an episode without a file: auto-next would choose it.
    assert_eq!(asked.param("isMissing"), Some("false"));
    let fields = asked.param("Fields").unwrap();
    assert!(
        fields.contains("MediaSources") && fields.contains("Overview"),
        "{fields}"
    );
}

#[test]
fn the_next_episode_of_a_series_is_next_up() {
    let s = Setup::new();
    let next = s.signed_in().next_up(&series()).unwrap().unwrap();
    assert_eq!(next.item.id, ItemId(EPISODE.into()));
    assert_eq!(next.item.resume, Some(Duration::from_secs(12 * 60)));
    assert_eq!(next.versions.len(), 1);
    let asked = s.fake.last("GET", "/Shows/NextUp");
    assert_eq!(asked.param("SeriesId"), Some(SERIES));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("EnableResumable"), Some("true"));
    assert!(asked.param("Fields").unwrap().contains("MediaSources"));
}

#[test]
fn a_series_watched_to_the_end_has_no_next_episode() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().all_watched = true;
    assert_eq!(s.signed_in().next_up(&series()).unwrap(), None);
}

#[test]
fn an_episode_plays_like_a_film() {
    let s = Setup::new();
    let source = MediaSourceId("5a000000000000000000000000000021".into());
    let playback = s
        .signed_in()
        .play(&ItemId(EPISODE.into()), &source)
        .unwrap();
    let url = playback.stream.expose_for_player();
    assert!(
        url.starts_with(&format!("{}/Videos/{EPISODE}/stream?", s.fake.url)),
        "{url}"
    );
    assert_eq!(playback.external_subtitles.len(), 1);
    assert_eq!(playback.session, PlaySessionId(PLAY_SESSION.into()));
}

#[test]
fn an_episode_or_a_whole_season_is_marked_watched_and_unwatched() {
    let s = Setup::new();
    let server = s.signed_in();
    for id in [EPISODE, SEASON] {
        let path = format!("/UserPlayedItems/{id}");
        server.set_watched(&ItemId(id.into()), true).unwrap();
        assert_eq!(s.fake.last("POST", &path).param("userId"), Some(USER_ID));
        server.set_watched(&ItemId(id.into()), false).unwrap();
        assert_eq!(s.fake.last("DELETE", &path).param("userId"), Some(USER_ID));
    }
}

const HARBOUR_TRILOGY: &str = "b0000000000000000000000000000002";

#[test]
fn the_collections_of_the_server_with_their_posters() {
    let s = Setup::new();
    let cards = s.signed_in().collections().unwrap();
    let names: Vec<&str> = cards.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Sea Stories",
            "Harbour Trilogy",
            "Lone Wolves",
            "Paper Moons"
        ]
    );
    assert_eq!(cards[1].id, ItemId(HARBOUR_TRILOGY.into()));
    assert_eq!(cards[1].films, Some(3));
    assert_eq!(
        cards[1].poster.as_ref().map(|p| p.tag.as_str()),
        Some("poster-ht")
    );
    // A collection with no count says so rather than inventing one.
    assert_eq!(cards[2].films, None);
}

const LONE_WOLVES: &str = "b0000000000000000000000000000003";
const PAPER_MOONS: &str = "b0000000000000000000000000000004";

#[test]
fn the_collections_are_read_without_asking_for_any_film() {
    let s = Setup::new();
    let cards = s.signed_in().collections().unwrap();
    // The grid shows from this one request: the posters of the bare cards come after.
    assert_eq!(boxset_lookups(&s).len(), 1, "{:?}", boxset_lookups(&s));
    assert_eq!(cards[2].name, "Lone Wolves");
    assert_eq!(cards[2].poster, None);
}

#[test]
fn a_collection_with_no_picture_shows_the_poster_of_its_first_film() {
    let s = Setup::new();
    let poster = s.signed_in().collection_poster(&ItemId(LONE_WOLVES.into()));
    // The earliest release that has a poster: the 2009 film has none, the 2011 one has.
    assert_eq!(
        poster,
        Some(ImageRef {
            item: ItemId("f00000000000000000000000000000a1".into()),
            kind: ImageKind::Primary,
            tag: "poster-hd".into()
        })
    );
    let asked = &boxset_lookups(&s)[0];
    assert_eq!(asked.param("parentId"), Some(LONE_WOLVES));
    assert_eq!(
        asked.param("sortBy"),
        Some("ProductionYear,PremiereDate,SortName")
    );
    assert_eq!(asked.param("limit"), Some("1"));
    assert_eq!(asked.param("imageTypes"), Some("Primary"));
    assert_eq!(asked.param("includeItemTypes"), Some("Movie"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
}

#[test]
fn a_collection_whose_films_have_no_poster_either_stays_without() {
    let s = Setup::new();
    let poster = s.signed_in().collection_poster(&ItemId(PAPER_MOONS.into()));
    assert_eq!(poster, None);
}

#[test]
fn a_failing_poster_request_leaves_the_card_bare_and_the_grid_shown() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().broken_parent = Some(LONE_WOLVES.into());
    let server = s.signed_in();
    let cards = server.collections().unwrap();
    assert_eq!(cards.len(), 4);
    assert_eq!(server.collection_poster(&cards[2].id), None);
    s.fake.state.lock().unwrap().broken_parent = None;
    assert!(server.collection_poster(&cards[2].id).is_some());
}

#[test]
fn the_collections_are_asked_by_name_for_the_user() {
    let s = Setup::new();
    s.signed_in().collections().unwrap();
    let asked = boxset_lookups(&s);
    let asked = &asked[0];
    assert_eq!(asked.param("includeItemTypes"), Some("BoxSet"));
    assert_eq!(asked.param("recursive"), Some("true"));
    assert_eq!(asked.param("sortBy"), Some("SortName"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("fields"), Some("ChildCount"));
    assert_eq!(asked.param("enableImageTypes"), Some("Primary"));
}

#[test]
fn a_collection_lists_its_films_by_release_year() {
    let s = Setup::new();
    let collection = s
        .signed_in()
        .collection(&ItemId(HARBOUR_TRILOGY.into()))
        .unwrap();
    assert_eq!(collection.name, "Harbour Trilogy");
    assert_eq!(
        collection.overview.as_deref(),
        Some("A lighthouse, a return, and the lights that follow.")
    );
    assert_eq!(
        collection
            .backdrop
            .as_ref()
            .map(|b| (b.kind, b.tag.as_str())),
        Some((ImageKind::Backdrop, "backdrop-ht"))
    );
    let films: Vec<(&str, Option<i32>)> = collection
        .films
        .iter()
        .map(|f| (f.title.as_str(), f.year))
        .collect();
    assert_eq!(
        films,
        [
            ("The Quiet Harbour", Some(1998)),
            ("Return to the Harbour", Some(2002)),
            ("Harbour Lights", Some(2006)),
        ]
    );
    assert!(collection.films[1].watched);
    let asked = s
        .fake
        .seen()
        .into_iter()
        .find(|r| r.param("parentId") == Some(HARBOUR_TRILOGY))
        .expect("the films were asked");
    assert_eq!(
        asked.param("sortBy"),
        Some("ProductionYear,PremiereDate,SortName")
    );
    assert_eq!(asked.param("userId"), Some(USER_ID));
}

#[test]
fn a_series_in_a_collection_is_not_listed_as_a_film() {
    let s = Setup::new();
    let collection = s
        .signed_in()
        .collection(&ItemId(HARBOUR_TRILOGY.into()))
        .unwrap();
    let titles: Vec<&str> = collection.films.iter().map(|f| f.title.as_str()).collect();
    assert!(!titles.contains(&"Harbour Keepers"), "{titles:?}");
    assert_eq!(titles.len(), 3);
}

#[test]
fn the_other_films_of_a_collection_leave_out_its_series() {
    let s = Setup::new();
    let collection = s.signed_in().collection_of(&film()).unwrap().unwrap();
    let titles: Vec<&str> = collection.others.iter().map(|f| f.title.as_str()).collect();
    assert!(!titles.contains(&"Harbour Keepers"), "{titles:?}");
    assert_eq!(titles.len(), 2);
}

#[test]
fn a_collection_the_server_does_not_know_is_not_found() {
    let s = Setup::new();
    let missing = s
        .signed_in()
        .collection(&ItemId("b0000000000000000000000000000099".into()));
    assert_eq!(missing, Err(ProviderError::NotFound));
}

#[test]
fn the_list_holds_films_and_series_together_by_title() {
    let s = Setup::new();
    let list = s.signed_in().favourites().unwrap();
    let titles: Vec<&str> = list
        .iter()
        .map(|favourite| match favourite {
            Favourite::Film(film) => film.title.as_str(),
            Favourite::Series(series) => series.title.as_str(),
        })
        .collect();
    assert_eq!(
        titles,
        ["Harbour Lights", "Return to the Harbour", "Tidewater"]
    );
    assert!(matches!(list[2], Favourite::Series(_)));
    assert!(matches!(list[0], Favourite::Film(_)));
}

#[test]
fn the_list_is_asked_as_the_users_favourites_of_both_kinds() {
    let s = Setup::new();
    s.signed_in().favourites().unwrap();
    let asked = s
        .fake
        .seen()
        .into_iter()
        .find(|r| r.param("filters") == Some("IsFavorite"))
        .expect("the favourites were asked");
    assert_eq!(asked.param("includeItemTypes"), Some("Movie,Series"));
    assert_eq!(asked.param("recursive"), Some("true"));
    assert_eq!(asked.param("sortBy"), Some("SortName"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
}

#[test]
fn a_film_and_a_series_say_whether_they_are_in_the_list() {
    let s = Setup::new();
    let server = s.signed_in();
    assert!(!server.film(&film()).unwrap().favourite);
    assert!(
        !server
            .series_detail(&ItemId(SERIES.into()))
            .unwrap()
            .favourite
    );
    s.fake.state.lock().unwrap().favourites = vec![FILM.into(), SERIES.into()];
    assert!(server.film(&film()).unwrap().favourite);
    assert!(
        server
            .series_detail(&ItemId(SERIES.into()))
            .unwrap()
            .favourite
    );
}

#[test]
fn an_item_is_put_in_the_list_and_taken_out_for_the_user() {
    let s = Setup::new();
    let server = s.signed_in();
    let path = format!("/UserFavoriteItems/{FILM}");
    server.set_favourite(&film(), true).unwrap();
    assert_eq!(s.fake.last("POST", &path).param("userId"), Some(USER_ID));
    assert!(server.film(&film()).unwrap().favourite);
    server.set_favourite(&film(), false).unwrap();
    assert_eq!(s.fake.last("DELETE", &path).param("userId"), Some(USER_ID));
    assert!(!server.film(&film()).unwrap().favourite);
}

#[test]
fn the_list_with_a_revoked_token_asks_to_sign_in_again() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake.state.lock().unwrap().revoked = true;
    assert_eq!(server.favourites(), Err(ProviderError::SessionExpired));
    assert_eq!(
        server.set_favourite(&film(), true),
        Err(ProviderError::SessionExpired)
    );
}

fn source(id: &str) -> MediaSourceId {
    MediaSourceId(id.into())
}

#[test]
fn the_trickplay_of_a_version_is_read_with_its_item() {
    let s = Setup::new();
    let trickplay = s
        .signed_in()
        .trickplay(&film(), &source(SOURCE))
        .unwrap()
        .expect("the film has trickplay");
    // Three sizes on offer: the smallest one wide enough.
    assert_eq!(
        trickplay,
        Trickplay {
            width: 320,
            height: 180,
            columns: 10,
            rows: 10,
            count: 660,
            interval: Duration::from_secs(10),
        }
    );
    let asked = s.fake.last("GET", &format!("/Items/{FILM}"));
    assert_eq!(asked.param("fields"), Some("Trickplay"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
}

#[test]
fn each_version_has_its_own_trickplay() {
    let s = Setup::new();
    let second = s
        .signed_in()
        .trickplay(&film(), &source("5a000000000000000000000000000002"))
        .unwrap()
        .expect("the second version has trickplay");
    // Its only size is narrower than wanted: it is the one shown, at its own interval.
    assert_eq!(
        (second.width, second.columns, second.count, second.interval),
        (240, 8, 66, Duration::from_secs(60))
    );
}

#[test]
fn an_item_without_trickplay_has_none() {
    let s = Setup::new();
    s.fake.state.lock().unwrap().no_trickplay = true;
    assert_eq!(s.signed_in().trickplay(&film(), &source(SOURCE)), Ok(None));
}

#[test]
fn a_version_the_trickplay_does_not_list_has_none() {
    let s = Setup::new();
    assert_eq!(
        s.signed_in().trickplay(&film(), &source("nowhere")),
        Ok(None)
    );
}

#[test]
fn a_tile_is_asked_by_its_number_in_the_size_shown() {
    let s = Setup::new();
    let server = s.signed_in();
    let trickplay = server.trickplay(&film(), &source(SOURCE)).unwrap().unwrap();
    let tile = server
        .trickplay_tile(&film(), &source(SOURCE), &trickplay, 3)
        .unwrap();
    assert_eq!(tile, TILE);
    let asked = s
        .fake
        .last("GET", &format!("/Videos/{FILM}/Trickplay/320/3.jpg"));
    assert_eq!(asked.param("mediaSourceId"), Some(SOURCE));
}

#[test]
fn a_tile_request_carries_the_token_in_its_header_never_in_its_address() {
    let s = Setup::new();
    let server = s.signed_in();
    let trickplay = server.trickplay(&film(), &source(SOURCE)).unwrap().unwrap();
    server
        .trickplay_tile(&film(), &source(SOURCE), &trickplay, 0)
        .unwrap();
    let asked = s
        .fake
        .last("GET", &format!("/Videos/{FILM}/Trickplay/320/0.jpg"));
    assert!(asked.authorization.unwrap().contains(TOKEN));
    assert!(
        asked.query.iter().all(|(_, value)| !value.contains(TOKEN)),
        "{:?}",
        asked.query
    );
}

#[test]
fn a_tile_the_server_does_not_have_is_not_found() {
    let s = Setup::new();
    let server = s.signed_in();
    let missing = Trickplay {
        width: 480,
        ..server.trickplay(&film(), &source(SOURCE)).unwrap().unwrap()
    };
    assert_eq!(
        server.trickplay_tile(&film(), &source(SOURCE), &missing, 0),
        Err(ProviderError::NotFound)
    );
}

#[test]
fn the_segments_of_a_film_are_read_typed_and_in_order() {
    let s = Setup::new();
    let segments = s.signed_in().media_segments(&film()).unwrap();
    let seconds = |start: u64, end: u64| (Duration::from_secs(start), Duration::from_secs(end));
    let found: Vec<(SegmentKind, (Duration, Duration))> = segments
        .iter()
        .map(|segment| (segment.kind, (segment.start, segment.end)))
        .collect();
    assert_eq!(
        found,
        [
            (SegmentKind::Recap, seconds(0, 90)),
            (SegmentKind::Intro, seconds(90, 150)),
            (SegmentKind::Preview, seconds(7080, 7140)),
            (SegmentKind::Outro, seconds(7140, 7860)),
        ]
    );
    let asked = s.fake.last("GET", &format!("/MediaSegments/{FILM}"));
    let types: Vec<&str> = asked
        .query
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("includeSegmentTypes"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(types, ["Intro", "Outro", "Recap", "Preview"]);
}

#[test]
fn a_server_without_media_segments_has_none_and_no_error() {
    let s = Setup::new();
    s.fake
        .state
        .lock()
        .unwrap()
        .broken
        .push((format!("/MediaSegments/{FILM}"), 404));
    assert_eq!(s.signed_in().media_segments(&film()).unwrap(), []);
}

#[test]
fn an_item_without_segments_has_none() {
    let s = Setup::new();
    let other = ItemId("f0000000000000000000000000000099".into());
    assert_eq!(s.signed_in().media_segments(&other).unwrap(), []);
}

#[test]
fn a_segments_route_that_fails_is_an_error_left_to_the_caller() {
    let s = Setup::new();
    let server = s.signed_in();
    s.fake
        .state
        .lock()
        .unwrap()
        .broken
        .push((format!("/MediaSegments/{FILM}"), 500));
    assert_eq!(
        server.media_segments(&film()),
        Err(ProviderError::Server(500))
    );
}

#[test]
fn a_film_and_a_series_have_their_tmdb_ids() {
    let setup = Setup::new();
    let server = setup.signed_in();
    let detail = server.film(&film()).unwrap();
    assert_eq!(detail.tmdb, Some(9001));
    assert_eq!(detail.tmdb_collection, Some(500));
    let series = server.series_detail(&ItemId(SERIES.into())).unwrap();
    assert_eq!(series.tmdb, Some(7001));
}

#[test]
fn the_library_is_indexed_by_tmdb_id_in_one_request() {
    let setup = Setup::new();
    let server = setup.signed_in();
    let index = server.tmdb_index().unwrap();
    assert_eq!(index.films.get(&9001), Some(&ItemId(FILM.into())));
    assert_eq!(
        index.films.get(&9002),
        Some(&ItemId("f0000000000000000000000000000002".into()))
    );
    assert_eq!(
        index.films.len(),
        2,
        "a film without a TMDb id is not in it"
    );
    assert_eq!(index.series.get(&7001), Some(&ItemId(SERIES.into())));
    let asked = setup.fake.last("GET", "/Items");
    assert_eq!(asked.param("Recursive"), Some("true"));
    assert_eq!(asked.param("IncludeItemTypes"), Some("Movie,Series"));
    assert_eq!(asked.param("Fields"), Some("ProviderIds"));
    assert_eq!(asked.param("userId"), Some(USER_ID));
}
