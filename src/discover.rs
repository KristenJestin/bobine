//! The logic of Discover and the request pages (#63): what a title's pill says, what is in the
//! library, which seasons a request offers, and the lines that say how a request went. No window
//! here: the views read it, the tests drive it.

use bobine::provider::{ItemId, TmdbIndex};
use bobine::requests::{
    Availability, CollectionOutcome, Discovered, MediaKind, RequestError, RequestStatus,
    SeasonOffer, ServerCheck, TmdbId,
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
        };
        assert_eq!(
            collection_line(&quota, &winter_saga()),
            "Requested: Winter Light. Not requested (quota reached): Winter's End, Spring."
        );
        let all = CollectionOutcome {
            requested: vec![3, 4],
            not_requested: Vec::new(),
            failure: None,
        };
        assert_eq!(
            collection_line(&all, &winter_saga()),
            "Requested: Winter Light, Winter's End."
        );
        let down = CollectionOutcome {
            requested: Vec::new(),
            not_requested: vec![3, 4],
            failure: Some(RequestError::Unreachable("down".into())),
        };
        assert_eq!(
            collection_line(&down, &winter_saga()),
            "Not requested (Seerr cannot be reached): Winter Light, Winter's End."
        );
        let refused = CollectionOutcome {
            requested: vec![3],
            not_requested: vec![4],
            failure: Some(RequestError::Forbidden),
        };
        assert_eq!(
            collection_line(&refused, &winter_saga()),
            "Requested: Winter Light. Not requested (refused for your account): Winter's End."
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
}
