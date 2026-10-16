//! The words of the Collections and Ma liste pages, and what they keep of the server's answer.

use bobine::provider::{CollectionCard, CollectionDetail, Film};

/// What the page says when the list holds nothing: where the list comes from.
pub const EMPTY_LIST: &str = "Votre liste est vide. Ouvrez un film ou une série et choisissez « Ma liste » pour l'ajouter ici.";

/// What the page says when the server has no collection.
pub const NO_COLLECTION: &str = "Aucune collection sur ce serveur.";

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
    use bobine::provider::{Favourite, ItemId, MediaServer};

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

    #[test]
    fn a_collection_with_no_picture_of_its_own_wears_its_first_films_poster() {
        let server = FakeServer::new(12, false);
        let bare: Vec<CollectionCard> = server
            .collections()
            .unwrap()
            .into_iter()
            .filter(|card| server.collection(&card.id).unwrap().backdrop.is_none())
            .collect();
        assert!(
            !bare.is_empty(),
            "the fake holds collections with no picture"
        );
        for card in bare {
            let first = server.collection(&card.id).unwrap().films.remove(0);
            assert_eq!(card.poster, first.poster, "{}", card.name);
            assert!(card.poster.is_some(), "{}", card.name);
        }
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
