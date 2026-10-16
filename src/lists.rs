//! The words of the Collections and Ma liste pages, and what they keep of the server's answer.

use std::collections::VecDeque;

use bobine::provider::{CollectionCard, CollectionDetail, Film, ImageRef, ItemId};

/// What the page says when the list holds nothing: where the list comes from.
pub const EMPTY_LIST: &str = "Votre liste est vide. Ouvrez un film ou une série et choisissez « Ma liste » pour l'ajouter ici.";

/// What the page says when the server has no collection.
pub const NO_COLLECTION: &str = "Aucune collection sur ce serveur.";

/// How many poster requests of bare collections are in flight at once: few enough to leave the
/// server and the connections to what the user is looking at, enough to not wait on each round
/// trip.
pub const POSTERS_IN_FLIGHT: usize = 4;

/// The collections that have no picture of their own, whose poster is asked after the grid shows.
#[derive(Default)]
pub struct PosterQueue {
    waiting: VecDeque<ItemId>,
}

impl PosterQueue {
    /// The bare cards, in the order of the grid.
    pub fn of(cards: &[CollectionCard]) -> Self {
        Self {
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

    /// The next to ask, when one request was answered.
    pub fn next(&mut self) -> Option<ItemId> {
        self.waiting.pop_front()
    }
}

/// A collection's poster landed: its card wears it.
pub fn wear(cards: &mut [CollectionCard], collection: &ItemId, poster: ImageRef) {
    if let Some(card) = cards.iter_mut().find(|card| &card.id == collection) {
        card.poster = Some(poster);
    }
}

/// `1 film`, `3 films`.
pub fn films(count: u32) -> String {
    format!("{count} {}", if count > 1 { "films" } else { "film" })
}

/// `1 collection`, `12 collections`.
pub fn collections(count: usize) -> String {
    format!(
        "{count} {}",
        if count > 1 {
            "collections"
        } else {
            "collection"
        }
    )
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
    fn a_collection_counts_its_films_in_french() {
        assert_eq!(films(1), "1 film");
        assert_eq!(films(3), "3 films");
        assert_eq!(films(0), "0 film");
        assert_eq!(collections(1), "1 collection");
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

    #[test]
    fn only_the_bare_collections_wait_for_a_poster() {
        let cards = cards_with_posters(&[true, false, true, false]);
        let mut queue = PosterQueue::of(&cards);
        assert_eq!(queue.next(), Some(ItemId("c1".into())));
        assert_eq!(queue.next(), Some(ItemId("c3".into())));
        assert_eq!(queue.next(), None);
    }

    #[test]
    fn no_more_posters_than_the_bound_are_asked_at_once() {
        let cards = cards_with_posters(&[false; 3 * POSTERS_IN_FLIGHT]);
        let mut queue = PosterQueue::of(&cards);
        let started = queue.start();
        assert_eq!(started.len(), POSTERS_IN_FLIGHT);
        // Each answer lets one more go, in the order of the grid.
        assert_eq!(queue.next(), Some(ItemId(format!("c{POSTERS_IN_FLIGHT}"))));
        let few = cards_with_posters(&[false, false]);
        assert_eq!(PosterQueue::of(&few).start().len(), 2);
        assert!(PosterQueue::default().start().is_empty());
    }

    #[test]
    fn a_poster_that_lands_is_worn_by_its_card_alone() {
        let mut cards = cards_with_posters(&[false, false]);
        let poster = ImageRef {
            item: ItemId("film".into()),
            kind: ImageKind::Primary,
            tag: "p".into(),
        };
        wear(&mut cards, &ItemId("c1".into()), poster.clone());
        assert_eq!(cards[0].poster, None);
        assert_eq!(cards[1].poster, Some(poster.clone()));
        // A card that left (the server changed) is no error.
        wear(&mut cards, &ItemId("gone".into()), poster);
    }

    #[test]
    fn the_collections_are_listed_by_name() {
        let names: Vec<String> = all_cards().into_iter().map(|c| c.name).collect();
        assert!(names.is_sorted(), "{names:?}");
    }

    #[test]
    fn the_empty_list_says_how_to_add_a_title() {
        assert!(EMPTY_LIST.contains("Ma liste"));
        assert!(EMPTY_LIST.contains("film") && EMPTY_LIST.contains("série"));
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
