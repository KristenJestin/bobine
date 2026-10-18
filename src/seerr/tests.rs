//! Each scenario of the request port against the fake Seerr server.

use super::SeerrClient;
use super::fake::*;
use crate::provider::Token;
use crate::requests::*;

fn client(fake: &FakeSeerr) -> SeerrClient {
    client_as(fake, KEY, VIEWER_JELLYFIN_ID)
}

fn client_as(fake: &FakeSeerr, key: &str, viewer: &str) -> SeerrClient {
    SeerrClient::with_images(&fake.url, Token::new(key.into()), viewer, fake.url.clone()).unwrap()
}

fn titles(found: &[Discovered]) -> Vec<&str> {
    found.iter().map(|t| t.title.as_str()).collect()
}

#[test]
fn a_configured_connection_is_tested_and_shows_its_version() {
    let fake = FakeSeerr::start();
    let check = client(&fake).check().unwrap();
    assert_eq!(check.version, VERSION);
    assert_eq!(check.acting_as, ActingAs::Viewer("camille".into()));
}

#[test]
fn a_wrong_key_says_so() {
    let fake = FakeSeerr::start();
    let wrong = client_as(&fake, "not-the-key", VIEWER_JELLYFIN_ID);
    assert_eq!(wrong.check(), Err(RequestError::KeyRefused));
}

#[test]
fn a_read_refused_once_the_key_worked_is_a_right_the_viewer_lacks() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().discover_forbidden = true;
    let seerr = client(&fake);
    assert!(seerr.check().is_ok());
    assert_eq!(
        seerr.discover(DiscoverRow::Trending),
        Err(RequestError::Forbidden)
    );
}

#[test]
fn an_address_where_no_seerr_answers_says_so() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().not_seerr = true;
    assert_eq!(client(&fake).check(), Err(RequestError::NotARequestServer));
    assert_eq!(
        SeerrClient::new("ftp://seerr.example.invalid", Token::new(KEY.into()), "x").err(),
        Some(RequestError::InvalidAddress)
    );
}

#[test]
fn an_address_without_a_scheme_is_reached_over_https() {
    assert_eq!(
        super::with_scheme(" seerr.example.org/ "),
        "https://seerr.example.org"
    );
    assert_eq!(
        super::with_scheme("http://seerr.example.org:5055"),
        "http://seerr.example.org:5055"
    );
    assert_eq!(super::with_scheme(""), "");
    assert!(SeerrClient::new("seerr.example.invalid", Token::new(KEY.into()), "x").is_ok());
}

#[test]
fn a_redirect_is_not_followed_and_the_key_never_reaches_another_server() {
    let fake = FakeSeerr::start();
    let elsewhere = FakeSeerr::start();
    fake.state.lock().unwrap().redirect_to = Some(elsewhere.url.clone());
    let seerr = client(&fake);
    assert_eq!(seerr.check(), Err(RequestError::Redirected));
    assert_eq!(
        seerr.discover(DiscoverRow::Trending),
        Err(RequestError::Redirected)
    );
    assert_eq!(
        seerr.request(&Ask::Movie(HARBOUR_NIGHTS)),
        Err(RequestError::Redirected)
    );
    assert!(
        elsewhere.seen().is_empty(),
        "nothing reached the other server"
    );
}

#[test]
fn the_viewer_is_found_among_the_users_and_requests_go_as_them() {
    let fake = FakeSeerr::start();
    let seerr = client(&fake);
    seerr.request(&Ask::Movie(HARBOUR_NIGHTS)).unwrap();
    let post = fake
        .seen()
        .into_iter()
        .find(|s| s.method == "POST")
        .unwrap();
    assert_eq!(post.api_user.as_deref(), Some(VIEWER.to_string().as_str()));
    // The users are read with the key alone, page after page until the viewer.
    let pages: Vec<_> = fake
        .seen()
        .into_iter()
        .filter(|s| s.path == "/api/v1/user")
        .collect();
    assert!(pages.len() > 1, "{} page(s)", pages.len());
    assert!(pages.iter().all(|p| p.api_user.is_none()));
}

#[test]
fn without_a_seerr_user_for_the_viewer_requests_go_as_the_key_owner() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().no_viewer = true;
    let seerr = client(&fake);
    assert_eq!(
        seerr.check().unwrap().acting_as,
        ActingAs::Owner("Owner".into())
    );
    seerr.request(&Ask::Movie(HARBOUR_NIGHTS)).unwrap();
    let post = fake
        .seen()
        .into_iter()
        .find(|s| s.method == "POST")
        .unwrap();
    assert_eq!(post.api_user, None);
}

#[test]
fn a_search_shows_results_with_their_status() {
    let fake = FakeSeerr::start();
    let found = client(&fake).search("harbour").unwrap();
    assert_eq!(
        titles(&found),
        ["Harbour Lights", "Harbour Nights", "Harbour Dawn"]
    );
    let states: Vec<Availability> = found.iter().map(|t| t.availability).collect();
    assert_eq!(
        states,
        [
            Availability::Available,
            Availability::Missing,
            Availability::Missing
        ]
    );
    let all = client(&fake).search("").unwrap();
    assert!(
        all.iter().all(|t| t.title != "Ines Varga"),
        "people are left out"
    );
    let pending = all.iter().find(|t| t.tmdb == PAPER_MOONS).unwrap();
    assert_eq!(pending.availability, Availability::Pending);
    assert_eq!(pending.year, Some(2023));
    assert_eq!(
        pending.poster,
        Some(TmdbImage(format!("/poster-{PAPER_MOONS}.jpg")))
    );
}

#[test]
fn a_search_query_is_encoded_strictly() {
    let fake = FakeSeerr::start();
    let found = client(&fake).search("Dawn + (cut)!").unwrap();
    assert!(found.is_empty());
    let asked = fake
        .seen()
        .into_iter()
        .find(|s| s.path == "/api/v1/search")
        .unwrap();
    assert_eq!(asked.param("query"), Some("Dawn + (cut)!"));
    assert!(
        asked.raw_query.contains("Dawn%20%2B%20%28cut%29%21"),
        "{}",
        asked.raw_query
    );
}

#[test]
fn a_film_is_requested_and_shows_as_pending() {
    let fake = FakeSeerr::start();
    let seerr = client(&fake);
    let before = seerr.title(MediaKind::Movie, HARBOUR_NIGHTS).unwrap();
    assert_eq!(before.title.availability, Availability::Missing);
    seerr.request(&Ask::Movie(HARBOUR_NIGHTS)).unwrap();
    assert_eq!(
        fake.posts(),
        [serde_json::json!({ "mediaType": "movie", "mediaId": HARBOUR_NIGHTS, "is4k": false })]
    );
    let after = seerr.title(MediaKind::Movie, HARBOUR_NIGHTS).unwrap();
    assert_eq!(after.title.availability, Availability::Pending);
}

#[test]
fn a_film_page_has_its_cast_its_runtime_and_its_collection() {
    let fake = FakeSeerr::start();
    let film = client(&fake).title(MediaKind::Movie, HARBOUR_DAWN).unwrap();
    assert_eq!(film.title.title, "Harbour Dawn");
    assert_eq!(film.runtime, Some(std::time::Duration::from_secs(112 * 60)));
    assert_eq!(film.genres, ["Drama"]);
    assert_eq!(
        film.collection,
        Some(CollectionRef {
            id: COLLECTION,
            name: "Harbour Saga".into()
        })
    );
    assert_eq!(film.cast[0].name, "Ines Varga");
    assert_eq!(film.cast[0].photo, Some(TmdbImage("/ines.jpg".into())));
    assert_eq!(film.cast[1].photo, None);
}

#[test]
fn a_film_lists_its_best_trailer() {
    let fake = FakeSeerr::start();
    let film = client(&fake).title(MediaKind::Movie, LIGHTHOUSE).unwrap();
    assert_eq!(
        film.trailer.as_deref(),
        Some("https://www.youtube.com/watch?v=offTrl01"),
        "the official one, not the fan cut listed before it"
    );
}

#[test]
fn a_series_lists_its_trailer_too() {
    let fake = FakeSeerr::start();
    let series = client(&fake).title(MediaKind::Tv, NORTHERN_LINE).unwrap();
    assert_eq!(
        series.trailer.as_deref(),
        Some("https://www.youtube.com/watch?v=lineTrl1")
    );
}

#[test]
fn a_vimeo_trailer_is_listed_and_a_video_on_another_site_is_left_out() {
    let fake = FakeSeerr::start();
    let series = client(&fake).title(MediaKind::Tv, COPPER_COAST).unwrap();
    assert_eq!(
        series.trailer.as_deref(),
        Some("https://vimeo.com/12345678")
    );
}

#[test]
fn a_title_with_only_teasers_or_featurettes_lists_no_trailer() {
    let fake = FakeSeerr::start();
    let film = client(&fake)
        .title(MediaKind::Movie, HARBOUR_LIGHTS)
        .unwrap();
    assert_eq!(film.trailer, None);
}

#[test]
fn a_title_with_no_videos_lists_no_trailer() {
    let fake = FakeSeerr::start();
    let film = client(&fake)
        .title(MediaKind::Movie, HARBOUR_NIGHTS)
        .unwrap();
    assert_eq!(film.trailer, None);
}

#[test]
fn a_series_offers_only_the_seasons_not_available_nor_requested() {
    let fake = FakeSeerr::start();
    let series = client(&fake).title(MediaKind::Tv, NORTHERN_LINE).unwrap();
    let offered: Vec<(u32, bool)> = series.seasons.iter().map(|s| (s.number, s.taken)).collect();
    // No specials; the first is available, the second requested, the third was declined.
    assert_eq!(offered, [(1, true), (2, true), (3, false)]);
    assert_eq!(series.title.availability, Availability::PartiallyAvailable);
}

#[test]
fn a_series_is_requested_with_chosen_seasons() {
    let fake = FakeSeerr::start();
    client(&fake)
        .request(&Ask::Tv(COPPER_COAST, vec![1, 3]))
        .unwrap();
    assert_eq!(
        fake.posts(),
        [serde_json::json!({
            "mediaType": "tv",
            "mediaId": COPPER_COAST,
            "seasons": [1, 3],
            "is4k": false,
        })]
    );
}

/// Asks for `films` as the collection page does: the quota, then each film once the one before
/// was answered.
fn request_films(seerr: &SeerrClient, films: &[TmdbId]) -> CollectionOutcome {
    let mut run = CollectionRun::default();
    run.quota(seerr.quota_reached(MediaKind::Movie), films);
    while let Some(film) = run.next(films) {
        run.answered(film, seerr.request(&Ask::Movie(film)), films);
    }
    run.outcome
}

#[test]
fn a_collection_with_two_films_in_the_library_and_two_missing_requests_only_the_two_missing_ones() {
    let fake = FakeSeerr::start();
    let seerr = client(&fake);
    let collection = seerr.collection(COLLECTION).unwrap();
    assert_eq!(collection.name, "Harbour Saga");
    assert_eq!(collection.films.len(), 4);
    let library = [LIGHTHOUSE, HARBOUR_LIGHTS];
    let wanted = missing(&collection.films, |kind, id| {
        kind == MediaKind::Movie && library.contains(&id)
    });
    assert_eq!(wanted, [HARBOUR_NIGHTS, HARBOUR_DAWN]);
    let outcome = request_films(&seerr, &wanted);
    assert_eq!(outcome.requested, wanted);
    let asked: Vec<u64> = fake
        .posts()
        .iter()
        .map(|p| p["mediaId"].as_u64().unwrap())
        .collect();
    assert_eq!(asked, [HARBOUR_NIGHTS, HARBOUR_DAWN]);
}

#[test]
fn a_collection_request_stops_at_the_quota_and_says_which_films_were_not_requested() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().movie_quota = Some(1);
    let seerr = client(&fake);
    let outcome = request_films(&seerr, &[HARBOUR_NIGHTS, HARBOUR_DAWN]);
    assert_eq!(outcome.requested, [HARBOUR_NIGHTS]);
    assert_eq!(outcome.not_requested, [HARBOUR_DAWN]);
    assert_eq!(outcome.failure, Some(RequestError::Quota));
    assert_eq!(fake.posts().len(), 2, "the second one met the quota");
    // A quota already reached sends nothing.
    let posts = fake.posts().len();
    let outcome = request_films(&seerr, &[PAPER_MOONS]);
    assert_eq!(outcome.failure, Some(RequestError::Quota));
    assert_eq!(fake.posts().len(), posts);
}

#[test]
fn the_rows_load() {
    let fake = FakeSeerr::start();
    let seerr = client(&fake);
    for row in DiscoverRow::ALL {
        assert!(!seerr.discover(row).unwrap().is_empty(), "{row:?}");
    }
    let paths: Vec<String> = fake.seen().into_iter().map(|s| s.path).collect();
    for path in [
        "/api/v1/discover/trending",
        "/api/v1/discover/movies",
        "/api/v1/discover/tv",
        "/api/v1/discover/movies/upcoming",
    ] {
        assert!(paths.iter().any(|p| p == path), "{path}");
    }
    let series = seerr.discover(DiscoverRow::PopularSeries).unwrap();
    assert!(series.iter().all(|t| t.kind == MediaKind::Tv));
}

#[test]
fn my_requests_are_the_viewers_with_their_titles_and_status() {
    let fake = FakeSeerr::start();
    let mine = client(&fake).my_requests().unwrap();
    let shown: Vec<(&str, RequestStatus)> = mine
        .iter()
        .map(|r| (r.title.title.as_str(), r.status))
        .collect();
    assert_eq!(
        shown,
        [
            ("Northern Line", RequestStatus::Pending),
            ("Paper Moons", RequestStatus::Approved)
        ]
    );
    let asked = fake
        .seen()
        .into_iter()
        .find(|s| s.path == "/api/v1/request")
        .unwrap();
    assert_eq!(
        asked.param("requestedBy"),
        Some(VIEWER.to_string().as_str())
    );
}

#[test]
fn a_request_whose_title_cannot_be_read_is_left_out_and_the_others_still_show() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().failing_titles = vec![NORTHERN_LINE];
    let shown = client(&fake)
        .my_requests()
        .map(|mine| mine.into_iter().map(|r| r.title.title).collect::<Vec<_>>());
    assert_eq!(shown, Ok(vec!["Paper Moons".to_string()]));
}

#[test]
fn requests_none_of_whose_titles_can_be_read_are_an_error_not_an_empty_list() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().failing_titles = vec![NORTHERN_LINE, PAPER_MOONS];
    let shown = client(&fake)
        .my_requests()
        .map(|mine| mine.into_iter().map(|r| r.title.title).collect::<Vec<_>>());
    assert_eq!(shown, Err(RequestError::Server(500)));
}

/// A queue as Seerr's tracker holds it: `size` bytes, `left` of them to go, the time left a .NET
/// time span.
fn queued(id: &str, size: u64, left: u64, time: &str) -> serde_json::Value {
    serde_json::json!({
        "mediaType": "movie", "externalId": 31, "size": size, "sizeLeft": left,
        "status": "downloading", "timeLeft": time, "title": "Invented.Release.2160p",
        "downloadId": id, "estimatedCompletionTime": "2026-10-17T20:00:00Z",
    })
}

#[test]
fn a_title_downloading_lists_its_queue_entries_with_their_percentage_and_time_left() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().queue.push((
        9006,
        serde_json::json!({ "downloadStatus": [queued("d1", 10_000, 3_800, "00:02:00")] }),
    ));
    let detail = client(&fake).title(MediaKind::Movie, 9006).unwrap();
    assert_eq!(detail.title.availability, Availability::Processing);
    let progress = progress(&detail.downloads).unwrap();
    assert_eq!(progress.percent, 62);
    assert_eq!(
        progress.time_left,
        Some(std::time::Duration::from_secs(120))
    );
}

#[test]
fn a_title_with_nothing_in_the_queue_lists_no_downloads() {
    let fake = FakeSeerr::start();
    let detail = client(&fake).title(MediaKind::Movie, 9006).unwrap();
    assert!(detail.downloads.is_empty());
}

#[test]
fn the_episodes_of_a_series_are_summed_in_one_progress() {
    let fake = FakeSeerr::start();
    let episode = |number: u32| {
        let mut entry = queued("pack", 3_000, 1_800, "00:10:00");
        entry["mediaType"] = "tv".into();
        entry["episode"] =
            serde_json::json!({ "seasonNumber": 1, "episodeNumber": number, "id": number });
        entry
    };
    fake.state.lock().unwrap().queue.push((
        NORTHERN_LINE,
        serde_json::json!({ "downloadStatus": [episode(1), episode(2), episode(3)] }),
    ));
    let detail = client(&fake).title(MediaKind::Tv, NORTHERN_LINE).unwrap();
    let progress = progress(&detail.downloads).unwrap();
    assert_eq!((progress.entries, progress.percent), (3, 40));
}

#[test]
fn my_requests_carry_the_downloads_of_their_titles() {
    let fake = FakeSeerr::start();
    fake.state.lock().unwrap().queue.push((
        PAPER_MOONS,
        serde_json::json!({ "downloadStatus": [queued("d2", 1_000, 250, "00:05:00")] }),
    ));
    let mine = client(&fake).my_requests().unwrap();
    let moons = mine.iter().find(|r| r.title.tmdb == PAPER_MOONS).unwrap();
    assert_eq!(progress(&moons.downloads).unwrap().percent, 75);
    let line = mine.iter().find(|r| r.title.tmdb == NORTHERN_LINE).unwrap();
    assert!(line.downloads.is_empty());
}

#[test]
fn recommendations_are_read_for_a_film_and_a_series() {
    let fake = FakeSeerr::start();
    let seerr = client(&fake);
    let films = seerr.recommendations(MediaKind::Movie, LIGHTHOUSE).unwrap();
    assert_eq!(
        titles(&films),
        ["Harbour Lights", "Paper Moons", "Glass Rivers"]
    );
    let series = seerr.recommendations(MediaKind::Tv, NORTHERN_LINE).unwrap();
    assert_eq!(titles(&series), ["Copper Coast"]);
}

#[test]
fn tmdb_pictures_are_fetched_without_the_key() {
    let fake = FakeSeerr::start();
    let picture = client(&fake)
        .image(&TmdbImage("/poster-9001.jpg".into()), 300)
        .unwrap();
    assert_eq!(picture, PICTURE);
    let asked = fake
        .seen()
        .into_iter()
        .find(|s| s.path.starts_with("/t/p/"))
        .unwrap();
    assert_eq!(asked.path, "/t/p/w342/poster-9001.jpg");
    assert_eq!(asked.api_key, None);
    assert_eq!(asked.api_user, None);
}

#[test]
fn the_api_key_never_appears_in_a_log_an_error_or_debug_output() {
    let planted = "pl4nted-k3y-0123456789";
    let fake = FakeSeerr::start();
    let refused = client_as(&fake, planted, VIEWER_JELLYFIN_ID);
    let mut said = vec![format!("{refused:?}")];
    let mut errors = vec![
        refused.check().unwrap_err(),
        refused.discover(DiscoverRow::Trending).unwrap_err(),
        refused.request(&Ask::Movie(HARBOUR_NIGHTS)).unwrap_err(),
        refused.my_requests().unwrap_err(),
    ];
    // Nothing answers at this port any more.
    let gone = {
        let other = FakeSeerr::start();
        other.url.clone()
    };
    let unreachable = SeerrClient::with_images(
        &gone,
        Token::new(planted.into()),
        VIEWER_JELLYFIN_ID,
        gone.clone(),
    )
    .unwrap();
    said.push(format!("{unreachable:?}"));
    errors.push(unreachable.check().unwrap_err());
    errors.push(
        unreachable
            .image(&TmdbImage("/x.jpg".into()), 300)
            .unwrap_err(),
    );
    fake.state.lock().unwrap().not_seerr = true;
    errors.push(refused.check().unwrap_err());
    errors.push(
        SeerrClient::new("ftp://nowhere", Token::new(planted.into()), "x")
            .err()
            .unwrap(),
    );
    for error in &errors {
        said.push(format!("{error} {error:?} {error:#?}"));
    }
    for line in said {
        assert!(!line.contains(planted), "{line}");
    }
    assert!(errors.contains(&RequestError::KeyRefused));
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, RequestError::Unreachable(_)))
    );
}
