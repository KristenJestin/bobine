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
fn detail_with_synopsis_genres_people_collection_and_similar() {
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

    let collection = detail.collection.expect("a collection");
    assert_eq!(collection.name, "Harbour Trilogy");
    let others: Vec<&str> = collection.others.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(others, ["Return to the Harbour", "Harbour Lights"]);

    let similar: Vec<&str> = detail.similar.iter().map(|f| f.title.as_str()).collect();
    assert_eq!(similar, ["Copper Sky", "Lanterns"]);
    assert_eq!(
        s.fake
            .last("GET", &format!("/Items/{FILM}"))
            .param("userId"),
        Some(USER_ID)
    );
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
        format!("api_key={TOKEN}"),
    ] {
        assert!(url.contains(&part), "{part} missing");
    }

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
            (5, format!("{base}/5/Stream.srt?api_key={TOKEN}").as_str()),
            (6, format!("{base}/6/Stream.ass?api_key={TOKEN}").as_str()),
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
