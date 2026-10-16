//! The words of the Collections and My list pages, and what they keep of the server's answer.

use std::collections::VecDeque;

use bobine::provider::{CollectionCard, CollectionDetail, Film, ImageRef, ItemId};

/// What the page says when the list holds nothing: where the list comes from.
pub const EMPTY_LIST: &str =
    "Your list is empty. Open a film or a series and choose “My list” to add it here.";

/// What the page says when the server has no collection.
pub const NO_COLLECTION: &str = "No collections on this server.";

/// How many poster requests of bare collections are in flight at once: few enough to leave the
/// server and the connections to what the user is looking at, enough to not wait on each round
/// trip. The chain goes on after the page is left: the requests already asked are not taken
/// back, and the cards are worn for when the user returns.
pub const POSTERS_IN_FLIGHT: usize = 4;

/// The collections that have no picture of their own, whose poster is asked after the grid shows.
#[derive(Default)]
pub struct PosterQueue {
    /// The load whose cards these are: the answers of an earlier one are not for this grid. A
    /// generation is never 0, which a reset queue keeps.
    generation: u64,
    waiting: VecDeque<ItemId>,
}

impl PosterQueue {
    /// The bare cards, in the order of the grid.
    pub fn of(cards: &[CollectionCard], generation: u64) -> Self {
        Self {
            generation,
            waiting: cards
                .iter()
                .filter(|card| card.poster.is_none())
                .map(|card| card.id.clone())
                .collect(),
        }
    }

    /// The first requests to send, at most `POSTERS_IN_FLIGHT`.
    pub fn start(&mut self) -> Vec<ItemId> {
        (0..POSTERS_IN_FLIGHT).map_while(|_| self.next()).collect()
    }

    fn next(&mut self) -> Option<ItemId> {
        self.waiting.pop_front()
    }

    /// A request of the load numbered `generation` was answered: the card wears the poster if
    /// there is one, and the id to ask next is returned. An answer of another load is no
    /// business of this queue: it changes nothing and starts nothing, so the requests in flight
    /// stay at most `POSTERS_IN_FLIGHT` after a new sign-in.
    pub fn answered(
        &mut self,
        cards: &mut [CollectionCard],
        generation: u64,
        collection: &ItemId,
        poster: Option<ImageRef>,
    ) -> Option<ItemId> {
        if generation != self.generation {
            return None;
        }
        if let Some(poster) = poster {
            wear(cards, collection, poster);
        }
        self.next()
    }
}

/// A collection's poster landed: its card wears it.
fn wear(cards: &mut [CollectionCard], collection: &ItemId, poster: ImageRef) {
    if let Some(card) = cards.iter_mut().find(|card| &card.id == collection) {
        card.poster = Some(poster);
    }
}

/// `0 films`, `1 film`, `3 films`: English keeps the singular for exactly one.
pub fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// `0 films`, `1 film`, `3 films`.
pub fn films(count: u32) -> String {
    plural(count as usize, "film", "films")
}

/// `0 collections`, `1 collection`, `12 collections`.
pub fn collections(count: usize) -> String {
    plural(count, "collection", "collections")
}

/// The line under a collection's poster: how many films it holds, when the server says.
pub fn card_caption(card: &CollectionCard) -> String {
    card.films.map(films).unwrap_or_default()
}

/// `1998`, or `1998 – 2006` from the first release to the last.
pub fn years(films: &[Film]) -> Option<String> {
    let mut years = films.iter().filter_map(|film| film.year);
    let first = years.next()?;
    let (first, last) = years.fold((first, first), |(first, last), year| {
        (first.min(year), last.max(year))
    });
    Some(if first == last {
        first.to_string()
    } else {
        format!("{first} – {last}")
    })
}

/// The facts under a collection's title: `3 films`, `1998 – 2006`.
pub fn meta(collection: &CollectionDetail) -> Vec<String> {
    [
        Some(films(collection.films.len() as u32)),
        years(&collection.films),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeServer;
    use crate::favourite;
    use bobine::provider::{Favourite, ImageKind, MediaServer};

    /// The title of an item of the list.
    fn title(favourite: &Favourite) -> &str {
        match favourite {
            Favourite::Film(film) => &film.title,
            Favourite::Series(series) => &series.title,
        }
    }

    fn all_cards() -> Vec<CollectionCard> {
        FakeServer::new(12, false).collections().unwrap()
    }

    #[test]
    fn the_empty_messages_are_plural_english() {
        assert_eq!(NO_COLLECTION, "No collections on this server.");
    }

    #[test]
    fn a_collection_counts_its_films_in_english() {
        assert_eq!(films(1), "1 film");
        assert_eq!(films(3), "3 films");
        assert_eq!(films(0), "0 films");
        assert_eq!(films(2), "2 films");
        assert_eq!(collections(0), "0 collections");
        assert_eq!(collections(1), "1 collection");
        assert_eq!(collections(2), "2 collections");
        assert_eq!(plural(0, "series", "series"), "0 series");
        assert_eq!(plural(1, "series", "series"), "1 series");
        assert_eq!(collections(12), "12 collections");
    }

    #[test]
    fn a_collection_card_says_how_many_films_it_holds() {
        let card = &all_cards()[0];
        assert_eq!(card_caption(card), films(card.films.unwrap()));
        let unknown = CollectionCard {
            films: None,
            ..card.clone()
        };
        assert_eq!(card_caption(&unknown), "");
    }

    #[test]
    fn a_collection_runs_from_its_first_release_to_its_last() {
        let server = FakeServer::new(12, false);
        let saga = server.collection(&ItemId("saga".into())).unwrap();
        assert_eq!(years(&saga.films).as_deref(), Some("2019 – 2025"));
        assert_eq!(years(&saga.films[..1]).as_deref(), Some("2019"));
        assert_eq!(years(&[]), None);
        assert_eq!(meta(&saga), ["2 films", "2019 – 2025"]);
    }

    #[test]
    fn films_of_a_collection_come_in_release_order() {
        let server = FakeServer::new(12, false);
        for card in server.collections().unwrap() {
            let collection = server.collection(&card.id).unwrap();
            let years: Vec<Option<i32>> = collection.films.iter().map(|f| f.year).collect();
            assert!(years.is_sorted(), "{}: {years:?}", collection.name);
            assert_eq!(Some(collection.films.len() as u32), card.films);
        }
    }

    fn bare(cards: Vec<CollectionCard>) -> Vec<CollectionCard> {
        cards
            .into_iter()
            .filter(|card| card.poster.is_none())
            .collect()
    }

    #[test]
    fn a_collection_with_no_picture_of_its_own_comes_bare_then_wears_its_first_films_poster() {
        let server = FakeServer::new(12, false);
        let bare = bare(server.collections().unwrap());
        assert!(
            !bare.is_empty(),
            "the fake holds collections with no picture"
        );
        for card in bare {
            let first = server.collection(&card.id).unwrap().films.remove(0);
            assert!(first.poster.is_some(), "{}", card.name);
            assert_eq!(server.collection_poster(&card.id), first.poster);
        }
    }

    fn cards_with_posters(posters: &[bool]) -> Vec<CollectionCard> {
        posters
            .iter()
            .enumerate()
            .map(|(n, has)| CollectionCard {
                id: ItemId(format!("c{n}")),
                name: format!("Collection {n}"),
                films: Some(2),
                poster: has.then(|| ImageRef {
                    item: ItemId(format!("c{n}")),
                    kind: ImageKind::Primary,
                    tag: "t".into(),
                }),
            })
            .collect()
    }

    fn poster(of: &str) -> ImageRef {
        ImageRef {
            item: ItemId(of.into()),
            kind: ImageKind::Primary,
            tag: "p".into(),
        }
    }

    fn id(n: usize) -> ItemId {
        ItemId(format!("c{n}"))
    }

    /// The generation of the load the tests are about.
    const LOAD: u64 = 7;

    #[test]
    fn only_the_bare_collections_wait_for_a_poster() {
        let cards = cards_with_posters(&[true, false, true, false]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        assert_eq!(queue.start(), [id(1), id(3)]);
    }

    #[test]
    fn no_more_posters_than_the_bound_are_asked_at_once() {
        let cards = cards_with_posters(&[false; 3 * POSTERS_IN_FLIGHT]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        assert_eq!(queue.start().len(), POSTERS_IN_FLIGHT);
        let few = cards_with_posters(&[false, false]);
        assert_eq!(PosterQueue::of(&few, LOAD).start().len(), 2);
        assert!(PosterQueue::default().start().is_empty());
    }

    #[test]
    fn an_answer_with_no_poster_still_starts_the_next_request() {
        let mut cards = cards_with_posters(&[false; 2 * POSTERS_IN_FLIGHT]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        queue.start();
        let next = queue.answered(&mut cards, LOAD, &id(0), None);
        assert_eq!(next, Some(id(POSTERS_IN_FLIGHT)));
        assert_eq!(cards[0].poster, None);
    }

    #[test]
    fn a_poster_that_lands_is_worn_by_its_card_alone() {
        let mut cards = cards_with_posters(&[false, false]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        queue.start();
        queue.answered(&mut cards, LOAD, &id(1), Some(poster("film")));
        assert_eq!(cards[0].poster, None);
        assert_eq!(cards[1].poster, Some(poster("film")));
    }

    #[test]
    fn an_answer_of_a_previous_load_changes_nothing_and_starts_nothing() {
        let mut cards = cards_with_posters(&[false; 2 * POSTERS_IN_FLIGHT]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        queue.start();
        let before = cards.clone();
        for old in [LOAD - 1, LOAD + 1] {
            let next = queue.answered(&mut cards, old, &id(0), Some(poster("old")));
            assert_eq!(next, None, "generation {old}");
        }
        assert_eq!(cards, before);
        // The queue still holds what the current load has to ask, untouched by the stale answers.
        assert_eq!(
            queue.answered(&mut cards, LOAD, &id(0), None),
            Some(id(POSTERS_IN_FLIGHT))
        );
    }

    #[test]
    fn an_answer_for_an_unknown_card_starts_the_next_and_wears_nothing() {
        let mut cards = cards_with_posters(&[false; 2 * POSTERS_IN_FLIGHT]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        queue.start();
        let before = cards.clone();
        let gone = ItemId("gone".into());
        let next = queue.answered(&mut cards, LOAD, &gone, Some(poster("film")));
        assert_eq!(next, Some(id(POSTERS_IN_FLIGHT)));
        assert_eq!(cards, before);
    }

    #[test]
    fn a_whole_drain_never_has_more_requests_in_flight_than_the_bound() {
        let total = 5 * POSTERS_IN_FLIGHT + 1;
        let mut cards = cards_with_posters(&vec![false; total]);
        let mut queue = PosterQueue::of(&cards, LOAD);
        let mut in_flight = queue.start();
        let mut most = in_flight.len();
        let mut asked = in_flight.len();
        // A previous load's answers come in between: they must not add a request.
        while !in_flight.is_empty() {
            let answered = in_flight.remove(0);
            assert_eq!(
                queue.answered(&mut cards, LOAD - 1, &answered, None),
                None,
                "a stale answer"
            );
            // The answers come with a poster, then without, in turn.
            let poster = asked.is_multiple_of(2).then(|| poster("film"));
            in_flight.extend(queue.answered(&mut cards, LOAD, &answered, poster));
            asked = total - queue.waiting.len();
            most = most.max(in_flight.len());
        }
        assert_eq!(most, POSTERS_IN_FLIGHT);
        assert_eq!(asked, total, "every bare card was asked once");
        assert!(queue.waiting.is_empty());
    }

    #[test]
    fn the_collections_are_listed_by_name() {
        let names: Vec<String> = all_cards().into_iter().map(|c| c.name).collect();
        assert!(names.is_sorted(), "{names:?}");
    }

    #[test]
    fn the_empty_list_says_how_to_add_a_title() {
        assert!(EMPTY_LIST.contains("My list"));
        assert!(EMPTY_LIST.contains("film") && EMPTY_LIST.contains("series"));
    }

    #[test]
    fn a_title_added_elsewhere_shows_in_the_list_when_it_is_asked_again() {
        let server = FakeServer::new(12, false);
        let before = server.favourites().unwrap();
        let film = ItemId("film-0".into());
        assert!(
            !before
                .iter()
                .any(|f| matches!(f, Favourite::Film(x) if x.id == film))
        );
        favourite::ask(&server, &film, true).unwrap();
        let after = server.favourites().unwrap();
        assert_eq!(after.len(), before.len() + 1);
        assert!(
            after
                .iter()
                .any(|f| matches!(f, Favourite::Film(x) if x.id == film))
        );
        // Films and series together, by title.
        let titles: Vec<String> = after.iter().map(|f| title(f).to_lowercase()).collect();
        assert!(titles.is_sorted(), "{titles:?}");
        assert!(after.iter().any(|f| matches!(f, Favourite::Series(_))));
        // Taken out again, it leaves.
        favourite::ask(&server, &film, false).unwrap();
        assert_eq!(server.favourites().unwrap().len(), before.len());
    }
}
