//! The logic of Discover and the request pages (#63): what a title's pill says, what is in the
//! library, which seasons a request offers, and the lines that say how a request went. No window
//! here: the views read it, the tests drive it.

use bobine::provider::{ItemId, TmdbIndex};
use std::time::Duration;

use bobine::requests::{
    Availability, CollectionOutcome, Discovered, Download, MediaKind, MyRequest, Progress,
    RequestError, RequestStatus, SeasonOffer, ServerCheck, TmdbId, progress,
};

/// The library page of a title of the request server, when the library holds it.
pub fn in_library(index: Option<&TmdbIndex>, kind: MediaKind, tmdb: TmdbId) -> Option<ItemId> {
    let index = index?;
    match kind {
        MediaKind::Movie => index.films.get(&tmdb),
        MediaKind::Tv => index.series.get(&tmdb),
    }
    .cloned()
}

/// "Similar, not in your library": the titles the library does not hold.
pub fn not_in_library(titles: Vec<Discovered>, index: Option<&TmdbIndex>) -> Vec<Discovered> {
    titles
        .into_iter()
        .filter(|title| in_library(index, title.kind, title.tmdb).is_none())
        .collect()
}

/// The small pill of a card or a page; none for a title nobody asked for.
pub fn status_label(availability: Availability, in_library: bool) -> Option<&'static str> {
    if in_library {
        return Some("In library");
    }
    match availability {
        Availability::Missing => None,
        Availability::Pending => Some("Pending"),
        Availability::Processing => Some("Processing"),
        Availability::PartiallyAvailable => Some("Partially available"),
        Availability::Available => Some("Available"),
        Availability::Blocked => Some("Blocked"),
    }
}

/// How long a download has left, as a pill says it: "2 min left".
pub fn left_text(left: Duration) -> String {
    // Rounded up, so that a download never has "0 min" left.
    let minutes = left.as_secs().div_ceil(60);
    match minutes {
        _ if left.as_secs() < 60 => "less than a minute left".into(),
        1..=59 => format!("{minutes} min left"),
        60..=1439 => match (minutes / 60, minutes % 60) {
            (hours, 0) => format!("{hours} h left"),
            (hours, minutes) => format!("{hours} h {minutes} min left"),
        },
        _ => format!("{} d {} h left", minutes / 1440, minutes % 1440 / 60),
    }
}

/// Whether the downloads count for a title in this state: it is on its way, not there.
fn is_downloading(availability: Availability, in_library: bool) -> bool {
    !in_library
        && matches!(
            availability,
            Availability::Processing | Availability::PartiallyAvailable
        )
}

/// What the downloads of a title come to, while they count.
pub fn download_progress(
    availability: Availability,
    in_library: bool,
    downloads: &[Download],
) -> Option<Progress> {
    progress(downloads).filter(|_| is_downloading(availability, in_library))
}

/// The pill of a request page: the library's word, else what is downloading, else the status.
/// Downloads count while the title is on its way (processing, or partly available with more
/// coming); a title that is available says so whatever the queue still holds.
pub fn page_status(
    kind: MediaKind,
    availability: Availability,
    in_library: bool,
    downloads: &[Download],
) -> Option<String> {
    if let Some(sum) = download_progress(availability, in_library, downloads) {
        return Some(match (kind, sum.entries, sum.time_left) {
            (MediaKind::Tv, 2.., _) => {
                format!("Downloading {} episodes · {} %", sum.entries, sum.percent)
            }
            (_, _, Some(left)) => {
                format!("Downloading · {} % · {}", sum.percent, left_text(left))
            }
            _ => format!("Downloading · {} %", sum.percent),
        });
    }
    match (availability, in_library) {
        (Availability::Processing, false) => Some("Approved, waiting for a download".into()),
        _ => status_label(availability, in_library).map(str::to_string),
    }
}

/// The short pill of a card of the viewer's requests while the title downloads.
pub fn card_status(downloads: &[Download]) -> Option<String> {
    progress(downloads).map(|sum| format!("Downloading · {} %", sum.percent))
}

/// The pill of a card in the viewer's requests: the short download status of an approved one
/// that downloads, else the request's own word.
pub fn my_request_label(request: &MyRequest) -> String {
    let downloading = (request.status == RequestStatus::Approved)
        .then(|| card_status(&request.downloads))
        .flatten();
    downloading.unwrap_or_else(|| request_label(request.status).to_string())
}

/// Whether a request page keeps reading its title: from the first download seen until the title
/// is available. A title still waiting for its download is not watched, and one the library
/// holds is done.
pub fn keeps_watching(availability: Availability, in_library: bool, downloading: bool) -> bool {
    downloading && is_downloading(availability, in_library)
}

/// What a request of the viewer's list says of itself.
pub fn request_label(status: RequestStatus) -> &'static str {
    match status {
        RequestStatus::Pending => "Pending",
        RequestStatus::Approved => "Requested",
        RequestStatus::Declined => "Declined",
        RequestStatus::Failed => "Failed",
        // As Seerr shows it: by its title, which came.
        RequestStatus::Completed => "Available",
    }
}

/// Whether a film can still be asked for.
pub fn can_request(availability: Availability, in_library: bool) -> bool {
    availability == Availability::Missing && !in_library
}

/// The seasons a series request lists: those not had nor asked for, all checked at first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeasonPicker {
    pub offered: Vec<SeasonOffer>,
    checked: Vec<u32>,
}

impl SeasonPicker {
    pub fn new(seasons: &[SeasonOffer]) -> Self {
        let offered: Vec<SeasonOffer> = seasons.iter().filter(|s| !s.taken).cloned().collect();
        let checked = offered.iter().map(|s| s.number).collect();
        Self { offered, checked }
    }

    pub fn is_checked(&self, number: u32) -> bool {
        self.checked.contains(&number)
    }

    pub fn toggle(&mut self, number: u32) {
        if !self.offered.iter().any(|s| s.number == number) {
            return;
        }
        match self.checked.iter().position(|n| *n == number) {
            Some(at) => {
                self.checked.remove(at);
            }
            None => {
                self.checked.push(number);
                self.checked.sort_unstable();
            }
        }
    }

    pub fn chosen(&self) -> Vec<u32> {
        self.checked.clone()
    }

    /// The button that sends the request.
    pub fn button(&self) -> String {
        match self.checked.as_slice() {
            [] => "Choose a season".into(),
            [one] => format!("Request season {one}"),
            many => format!("Request {} seasons", many.len()),
        }
    }
}

/// The movies a collection request may ask for, those checked, and the button that sends them.
/// Every one is checked at first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilmPicker {
    offered: Vec<TmdbId>,
    checked: Vec<TmdbId>,
}

impl FilmPicker {
    pub fn new(offered: Vec<TmdbId>) -> Self {
        Self {
            checked: offered.clone(),
            offered,
        }
    }

    /// Whether any movie is left to ask for.
    pub fn offers_any(&self) -> bool {
        !self.offered.is_empty()
    }

    pub fn is_offered(&self, film: TmdbId) -> bool {
        self.offered.contains(&film)
    }

    pub fn is_checked(&self, film: TmdbId) -> bool {
        self.checked.contains(&film)
    }

    pub fn toggle(&mut self, film: TmdbId) {
        if !self.is_offered(film) {
            return;
        }
        if self.is_checked(film) {
            self.checked.retain(|checked| *checked != film);
        } else {
            self.checked.push(film);
        }
    }

    /// The films checked, in the collection's order.
    pub fn chosen(&self) -> Vec<TmdbId> {
        (self.offered.iter())
            .filter(|film| self.is_checked(**film))
            .copied()
            .collect()
    }

    pub fn button(&self) -> String {
        match self.checked.len() {
            0 => "Choose a movie".into(),
            all if all == self.offered.len() => "Request collection".into(),
            1 => "Request 1 movie".into(),
            many => format!("Request {many} movies"),
        }
    }

    /// Those just asked for are offered no more.
    pub fn asked(&mut self, films: &[TmdbId]) {
        self.offered.retain(|film| !films.contains(film));
        self.checked.retain(|film| !films.contains(film));
    }
}

/// Why the films of a collection request were not asked for, in the brackets of its line.
fn not_requested_because(error: &RequestError) -> &'static str {
    match error {
        RequestError::Quota => "quota reached",
        RequestError::AlreadyRequested => "requested already",
        RequestError::Forbidden => "refused for your account",
        RequestError::KeyRefused => "API key refused",
        RequestError::NotFound => "unknown to Seerr",
        RequestError::Unreachable(_) => "Seerr cannot be reached",
        RequestError::Redirected => "Seerr answered with a redirect",
        RequestError::InvalidAddress
        | RequestError::NotARequestServer
        | RequestError::Server(_)
        | RequestError::BadAnswer(_) => "unexpected answer",
    }
}

/// How a collection request went, movie by movie, by their titles: "Requested: A, B. Not
/// requested (quota reached): C."
pub fn collection_line(outcome: &CollectionOutcome, films: &[Discovered]) -> String {
    let named = |ids: &[TmdbId]| -> String {
        let titles: Vec<&str> = ids
            .iter()
            .filter_map(|id| films.iter().find(|film| film.tmdb == *id))
            .map(|film| film.title.as_str())
            .collect();
        titles.join(", ")
    };
    let mut parts = Vec::new();
    if !outcome.requested.is_empty() {
        parts.push(format!("Requested: {}.", named(&outcome.requested)));
    }
    if !outcome.already_requested.is_empty() {
        let already = named(&outcome.already_requested);
        parts.push(format!("Already requested: {already}."));
    }
    if !outcome.not_requested.is_empty() {
        let missed = named(&outcome.not_requested);
        parts.push(match &outcome.failure {
            Some(why) => format!("Not requested ({}): {missed}.", not_requested_because(why)),
            None => format!("Not requested: {missed}."),
        });
    }
    parts.join(" ")
}

/// A failure of the request server, in a line of the page.
pub fn error_line(error: &RequestError) -> &'static str {
    match error {
        RequestError::InvalidAddress => "The address must start with http:// or https://.",
        RequestError::NotARequestServer => "No Seerr server answers at this address.",
        RequestError::Redirected => "Seerr answered with a redirect: check the address.",
        RequestError::KeyRefused => "Seerr refused the API key.",
        RequestError::Quota => "Your request quota is reached.",
        RequestError::Forbidden => "Seerr refused this action for your account.",
        RequestError::AlreadyRequested => "It was requested already.",
        RequestError::NotFound => "Seerr does not know this title.",
        RequestError::Unreachable(_) => "Seerr cannot be reached.",
        RequestError::Server(_) | RequestError::BadAnswer(_) => "Seerr gave an unexpected answer.",
    }
}

/// What a test of the connection says, and the line that says whom requests go as when it is
/// not the viewer.
pub fn check_lines(check: &ServerCheck) -> (String, Option<String>) {
    let owner = match &check.acting_as {
        bobine::requests::ActingAs::Viewer(_) => None,
        bobine::requests::ActingAs::Owner(name) => Some(format!(
            "No Seerr user has your Jellyfin account: requests go as {name}."
        )),
    };
    (format!("Connected to Seerr {}.", check.version), owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobine::requests::{ActingAs, TmdbImage};

    fn titled(tmdb: TmdbId, title: &str, availability: Availability) -> Discovered {
        Discovered {
            kind: MediaKind::Movie,
            tmdb,
            title: title.into(),
            year: None,
            rating: None,
            overview: None,
            poster: Some(TmdbImage("/p.jpg".into())),
            backdrop: None,
            availability,
        }
    }

    fn index() -> TmdbIndex {
        let mut index = TmdbIndex::default();
        index.films.insert(1, ItemId("film-1".into()));
        index.series.insert(1, ItemId("series-1".into()));
        index
    }

    #[test]
    fn a_title_is_in_the_library_by_its_kind_and_tmdb_id() {
        let index = index();
        assert_eq!(
            in_library(Some(&index), MediaKind::Movie, 1),
            Some(ItemId("film-1".into()))
        );
        assert_eq!(
            in_library(Some(&index), MediaKind::Tv, 1),
            Some(ItemId("series-1".into()))
        );
        assert_eq!(in_library(Some(&index), MediaKind::Movie, 2), None);
        assert_eq!(in_library(None, MediaKind::Movie, 1), None);
    }

    #[test]
    fn similar_not_in_your_library_leaves_out_what_is_in_the_library() {
        let titles = vec![
            titled(1, "Held", Availability::Available),
            titled(2, "Not held", Availability::Missing),
        ];
        let kept = not_in_library(titles, Some(&index()));
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "Not held");
    }

    #[test]
    fn a_pill_says_in_library_requested_or_available_and_nothing_for_a_missing_title() {
        assert_eq!(
            status_label(Availability::Missing, true),
            Some("In library")
        );
        assert_eq!(status_label(Availability::Missing, false), None);
        assert_eq!(status_label(Availability::Pending, false), Some("Pending"));
        assert_eq!(
            status_label(Availability::Processing, false),
            Some("Processing")
        );
        assert_eq!(
            status_label(Availability::PartiallyAvailable, false),
            Some("Partially available")
        );
        assert_eq!(
            status_label(Availability::Available, false),
            Some("Available")
        );
        assert_eq!(status_label(Availability::Blocked, false), Some("Blocked"));
        assert_eq!(request_label(RequestStatus::Approved), "Requested");
        assert_eq!(request_label(RequestStatus::Pending), "Pending");
        assert_eq!(request_label(RequestStatus::Declined), "Declined");
        assert_eq!(request_label(RequestStatus::Failed), "Failed");
        // Seerr shows a completed request by its title's status: it came.
        assert_eq!(request_label(RequestStatus::Completed), "Available");
    }

    fn season(number: u32, taken: bool) -> SeasonOffer {
        SeasonOffer {
            number,
            name: format!("Season {number}"),
            episodes: 8,
            taken,
        }
    }

    #[test]
    fn a_series_request_lists_the_seasons_not_had_all_checked() {
        let mut picker = SeasonPicker::new(&[season(1, true), season(2, false), season(3, false)]);
        let offered: Vec<u32> = picker.offered.iter().map(|s| s.number).collect();
        assert_eq!(offered, [2, 3]);
        assert_eq!(picker.chosen(), [2, 3]);
        assert_eq!(picker.button(), "Request 2 seasons");
        picker.toggle(2);
        assert_eq!(picker.chosen(), [3]);
        assert_eq!(picker.button(), "Request season 3");
        // A season had already cannot be checked.
        picker.toggle(1);
        assert_eq!(picker.chosen(), [3]);
        picker.toggle(3);
        assert_eq!(picker.button(), "Choose a season");
        picker.toggle(2);
        picker.toggle(3);
        assert_eq!(picker.chosen(), [2, 3]);
    }

    #[test]
    fn a_collection_request_checks_every_movie_it_may_ask_for() {
        let mut picker = FilmPicker::new(vec![3, 4]);
        assert_eq!(picker.chosen(), [3, 4]);
        assert_eq!(picker.button(), "Request collection");
        picker.toggle(4);
        assert_eq!(picker.chosen(), [3]);
        assert_eq!(picker.button(), "Request 1 movie");
        // A movie in the library or requested is not offered: it cannot be checked.
        picker.toggle(1);
        assert_eq!(picker.chosen(), [3]);
        assert!(!picker.is_offered(1));
        picker.toggle(3);
        assert_eq!(picker.button(), "Choose a movie");
        picker.toggle(4);
        picker.toggle(3);
        assert_eq!(picker.chosen(), [3, 4], "in the collection's order");
        let three = FilmPicker::new(vec![3, 4, 5]);
        let mut two = three.clone();
        two.toggle(5);
        assert_eq!(two.button(), "Request 2 movies");
        // Once asked for, a movie is offered no more; one left unchecked stays so.
        two.asked(&[3]);
        assert_eq!(two.chosen(), [4]);
        assert!(!two.is_offered(3));
        assert!(two.is_offered(5) && !two.is_checked(5));
    }

    fn winter_saga() -> Vec<Discovered> {
        vec![
            titled(3, "Winter Light", Availability::Missing),
            titled(4, "Winter's End", Availability::Missing),
            titled(5, "Spring", Availability::Missing),
        ]
    }

    #[test]
    fn a_collection_request_names_the_movies_requested_and_those_not_with_why() {
        let quota = CollectionOutcome {
            requested: vec![3],
            not_requested: vec![4, 5],
            failure: Some(RequestError::Quota),
            ..CollectionOutcome::default()
        };
        assert_eq!(
            collection_line(&quota, &winter_saga()),
            "Requested: Winter Light. Not requested (quota reached): Winter's End, Spring."
        );
        let all = CollectionOutcome {
            requested: vec![3, 4],
            not_requested: Vec::new(),
            failure: None,
            ..CollectionOutcome::default()
        };
        assert_eq!(
            collection_line(&all, &winter_saga()),
            "Requested: Winter Light, Winter's End."
        );
        let down = CollectionOutcome {
            requested: Vec::new(),
            not_requested: vec![3, 4],
            failure: Some(RequestError::Unreachable("down".into())),
            ..CollectionOutcome::default()
        };
        assert_eq!(
            collection_line(&down, &winter_saga()),
            "Not requested (Seerr cannot be reached): Winter Light, Winter's End."
        );
        let refused = CollectionOutcome {
            requested: vec![3],
            not_requested: vec![4],
            failure: Some(RequestError::Forbidden),
            ..CollectionOutcome::default()
        };
        assert_eq!(
            collection_line(&refused, &winter_saga()),
            "Requested: Winter Light. Not requested (refused for your account): Winter's End."
        );
        let already = CollectionOutcome {
            requested: vec![4],
            already_requested: vec![3],
            ..CollectionOutcome::default()
        };
        assert_eq!(
            collection_line(&already, &winter_saga()),
            "Requested: Winter's End. Already requested: Winter Light."
        );
    }

    #[test]
    fn the_test_of_a_connection_says_the_version_and_whom_requests_go_as() {
        let viewer = ServerCheck {
            version: "2.7.3".into(),
            acting_as: ActingAs::Viewer("camille".into()),
        };
        assert_eq!(
            check_lines(&viewer),
            ("Connected to Seerr 2.7.3.".into(), None)
        );
        let owner = ServerCheck {
            acting_as: ActingAs::Owner("Owner".into()),
            ..viewer
        };
        assert_eq!(
            check_lines(&owner).1.as_deref(),
            Some("No Seerr user has your Jellyfin account: requests go as Owner.")
        );
    }

    #[test]
    fn a_refusal_once_the_key_worked_says_it_is_the_account() {
        assert_eq!(
            error_line(&RequestError::Forbidden),
            "Seerr refused this action for your account."
        );
    }

    #[test]
    fn a_film_had_or_asked_for_cannot_be_requested() {
        assert!(can_request(Availability::Missing, false));
        assert!(!can_request(Availability::Missing, true));
        assert!(!can_request(Availability::Pending, false));
        assert!(!can_request(Availability::Blocked, false));
    }

    fn download(id: &str, size: u64, left: u64, seconds: Option<u64>) -> Download {
        Download {
            id: Some(id.to_string()),
            size,
            size_left: left,
            time_left: seconds.map(Duration::from_secs),
        }
    }

    #[test]
    fn a_time_left_is_said_in_minutes_then_hours_then_days() {
        assert_eq!(left_text(Duration::from_secs(0)), "less than a minute left");
        assert_eq!(
            left_text(Duration::from_secs(59)),
            "less than a minute left"
        );
        assert_eq!(left_text(Duration::from_secs(60)), "1 min left");
        assert_eq!(left_text(Duration::from_secs(61)), "2 min left");
        assert_eq!(left_text(Duration::from_secs(120)), "2 min left");
        assert_eq!(left_text(Duration::from_secs(3600)), "1 h left");
        assert_eq!(left_text(Duration::from_secs(4800)), "1 h 20 min left");
        assert_eq!(left_text(Duration::from_secs(90_000)), "1 d 1 h left");
    }

    #[test]
    fn a_title_downloading_says_its_percentage_and_its_time_left() {
        let one = [download("a", 1000, 380, Some(120))];
        assert_eq!(
            page_status(MediaKind::Movie, Availability::Processing, false, &one).as_deref(),
            Some("Downloading · 62 % · 2 min left")
        );
        // A queue that gives no time says the percentage only.
        let unknown = [download("a", 1000, 380, None)];
        assert_eq!(
            page_status(MediaKind::Movie, Availability::Processing, false, &unknown).as_deref(),
            Some("Downloading · 62 %")
        );
        // A single episode is one download like a film.
        assert_eq!(
            page_status(MediaKind::Tv, Availability::Processing, false, &one).as_deref(),
            Some("Downloading · 62 % · 2 min left")
        );
    }

    #[test]
    fn the_episodes_of_a_series_downloading_are_one_line() {
        let pack = [
            download("pack", 3000, 1800, Some(600)),
            download("pack", 3000, 1800, Some(600)),
            download("pack", 3000, 1800, Some(600)),
        ];
        assert_eq!(
            page_status(MediaKind::Tv, Availability::Processing, false, &pack).as_deref(),
            Some("Downloading 3 episodes · 40 %")
        );
        // A film in HD and in 4K is still a film: no episodes.
        assert_eq!(
            page_status(MediaKind::Movie, Availability::Processing, false, &pack).as_deref(),
            Some("Downloading · 40 % · 10 min left")
        );
    }

    #[test]
    fn without_a_download_the_status_is_worded_by_what_seerr_says() {
        let words = |availability| page_status(MediaKind::Movie, availability, false, &[]);
        assert_eq!(
            words(Availability::Processing).as_deref(),
            Some("Approved, waiting for a download")
        );
        assert_eq!(words(Availability::Pending).as_deref(), Some("Pending"));
        assert_eq!(words(Availability::Missing), None);
        assert_eq!(words(Availability::Blocked).as_deref(), Some("Blocked"));
        assert_eq!(
            words(Availability::PartiallyAvailable).as_deref(),
            Some("Partially available")
        );
    }

    #[test]
    fn a_title_that_is_there_does_not_say_downloading() {
        let one = [download("a", 1000, 380, Some(120))];
        let said =
            |availability, in_library| page_status(MediaKind::Tv, availability, in_library, &one);
        assert_eq!(
            said(Availability::Available, false).as_deref(),
            Some("Available")
        );
        assert_eq!(
            said(Availability::Processing, true).as_deref(),
            Some("In library")
        );
        // A series with a season here and another coming.
        assert_eq!(
            said(Availability::PartiallyAvailable, false).as_deref(),
            Some("Downloading · 62 % · 2 min left")
        );
    }

    #[test]
    fn a_card_of_the_viewer_s_requests_says_the_short_status() {
        let one = [download("a", 1000, 380, Some(120))];
        assert_eq!(card_status(&one).as_deref(), Some("Downloading · 62 %"));
        assert_eq!(card_status(&[]), None);
    }

    #[test]
    fn a_page_is_watched_from_the_first_download_until_the_title_is_available() {
        use Availability::*;
        assert!(keeps_watching(Processing, false, true));
        assert!(keeps_watching(PartiallyAvailable, false, true));
        // Done downloading, not yet available: the page waits for it.
        assert!(keeps_watching(Processing, false, true));
        assert!(!keeps_watching(Available, false, true));
        assert!(!keeps_watching(Processing, true, true));
        // Never watched without a download seen: waiting for approval or for a grab.
        assert!(!keeps_watching(Processing, false, false));
        assert!(!keeps_watching(Pending, false, false));
        assert!(!keeps_watching(Missing, false, true));
        assert!(!keeps_watching(Blocked, false, true));
    }
}
