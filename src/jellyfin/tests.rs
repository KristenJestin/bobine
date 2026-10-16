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
    assert_eq!(asked.param("SortOrder"), Some("Descending"));
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
    assert_eq!(asked.param("SortOrder"), Some("Ascending"));
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
fn continue_watching() {
    let s = Setup::new();
    let films = s.signed_in().continue_watching().unwrap();
    assert_eq!(films.len(), 1);
    assert_eq!(films[0].id, film());
    assert_eq!(films[0].resume, Some(Duration::from_secs(20 * 60)));
    let asked = s.fake.last("GET", "/UserItems/Resume");
    assert_eq!(asked.param("userId"), Some(USER_ID));
    assert_eq!(asked.param("IncludeItemTypes"), Some("Movie"));
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
    assert_eq!(boxset_lookups(&s).len(), 1 + 3);
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

    for path in [
        "/Sessions/Playing",
        "/Sessions/Playing/Progress",
        "/Sessions/Playing/Stopped",
    ] {
        let header = s.fake.last("POST", path).authorization.unwrap();
        assert!(header.contains(&format!("Token=\"{TOKEN}\"")), "{path}");
    }
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
    assert_eq!(names, ["Sea Stories", "Harbour Trilogy", "Lone Wolves"]);
    assert_eq!(cards[1].id, ItemId(HARBOUR_TRILOGY.into()));
    assert_eq!(cards[1].films, Some(3));
    assert_eq!(
        cards[1].poster.as_ref().map(|p| p.tag.as_str()),
        Some("poster-ht")
    );
    // A collection with no poster and no count says so rather than inventing them.
    assert_eq!((cards[2].films, cards[2].poster.clone()), (None, None));
}

#[test]
fn the_collections_are_asked_by_name_for_the_user() {
    let s = Setup::new();
    s.signed_in().collections().unwrap();
    let asked = boxset_lookups(&s);
    assert_eq!(asked.len(), 1, "{asked:?}");
    let asked = &asked[0];
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
