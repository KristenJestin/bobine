//! The words of the Lists page and of a playlist, and what they keep of the server's answer.

use bobine::provider::{EntryId, ImageRef, PlaylistEntry, Title};

use crate::lists::plural;
use crate::series_page::{self, Next};

/// How many posters a card's mosaic shows: two by two.
pub const MOSAIC: usize = 4;

/// `0 titles`, `1 title`, `3 titles`.
pub fn titles(count: usize) -> String {
    plural(count, "title", "titles")
}

/// The picture of a title in a list: a film's or a series' poster, an episode's still.
pub fn poster(title: &Title) -> Option<&ImageRef> {
    match title {
        Title::Film(film) => film.poster.as_ref(),
        Title::Series(series) => series.poster.as_ref(),
        Title::Episode(episode) => episode.item.poster.as_ref(),
    }
}

/// The posters a card's mosaic shows: those of its first titles that have one, four at most.
pub fn mosaic<'a>(posters: impl IntoIterator<Item = Option<&'a ImageRef>>) -> Vec<ImageRef> {
    posters
        .into_iter()
        .flatten()
        .take(MOSAIC)
        .cloned()
        .collect()
}

/// The posters of a playlist's card.
pub fn playlist_mosaic(entries: &[PlaylistEntry]) -> Vec<ImageRef> {
    mosaic(entries.iter().map(|entry| poster(&entry.title)))
}

/// A title's wide picture, for the card of the next one: a film's or a series' backdrop, else
/// its poster; an episode's still.
pub fn still(title: &Title) -> Option<&ImageRef> {
    match title {
        Title::Film(film) => film.backdrop.as_ref().or(film.poster.as_ref()),
        Title::Series(series) => series.backdrop.as_ref().or(series.poster.as_ref()),
        Title::Episode(episode) => episode.item.poster.as_ref(),
    }
}

/// A title in one line: a film's or a series' name; an episode's series, then `S1 · E2 · Le
/// Signal`.
pub fn name(title: &Title) -> String {
    match title {
        Title::Film(film) => film.title.clone(),
        Title::Series(series) => series.title.clone(),
        Title::Episode(episode) => {
            let (series, episode) = series_page::player_title(episode);
            format!("{series} · {episode}")
        }
    }
}

/// What the line under a title of a playlist says: a film's year, `Series`, an episode's code.
pub fn caption(title: &Title) -> String {
    match title {
        Title::Film(film) => film.year.map(|year| year.to_string()).unwrap_or_default(),
        Title::Series(series) => series_page::years(series)
            .map(|years| format!("Series · {years}"))
            .unwrap_or_else(|| "Series".into()),
        Title::Episode(episode) => series_page::code(episode).unwrap_or_default(),
    }
}

/// What the card offers once the title at `at` ended: the one after it, if any.
pub fn after(entries: &[PlaylistEntry], at: usize) -> Option<Next> {
    let next = at + 1;
    entries
        .get(next)
        .map(|entry| Next::Entry(next, entry.clone()))
}

/// Takes `entry` out of what the page shows, at once: where it was, to put it back if the
/// server refuses.
pub fn take_out(
    entries: &mut Vec<PlaylistEntry>,
    entry: &EntryId,
) -> Option<(usize, PlaylistEntry)> {
    let at = entries.iter().position(|shown| &shown.id == entry)?;
    Some((at, entries.remove(at)))
}

/// The server refused to take `entry` out: it is back where it was.
pub fn put_back(entries: &mut Vec<PlaylistEntry>, at: usize, entry: PlaylistEntry) {
    entries.insert(at.min(entries.len()), entry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeServer;
    use bobine::provider::{ImageKind, ItemId, MediaServer};

    fn watchlist(server: &FakeServer) -> Vec<PlaylistEntry> {
        server
            .playlist(&ItemId("playlist-watchlist".into()))
            .unwrap()
    }

    #[test]
    fn the_title_after_one_is_the_next_in_the_playlist_and_none_after_the_last() {
        let entries = watchlist(&FakeServer::new(12, false));
        assert_eq!(after(&entries, 0), Some(Next::Entry(1, entries[1].clone())));
        assert_eq!(after(&entries, entries.len() - 1), None);
    }

    #[test]
    fn an_entry_taken_out_goes_back_to_its_place_when_refused() {
        let mut entries = watchlist(&FakeServer::new(12, false));
        let before = entries.clone();
        let (at, gone) = take_out(&mut entries, &before[1].id).unwrap();
        assert_eq!(at, 1);
        assert_eq!(entries.len(), before.len() - 1);
        assert!(!entries.contains(&gone));
        put_back(&mut entries, at, gone);
        assert_eq!(entries, before);
        assert!(take_out(&mut entries, &EntryId("gone".into())).is_none());
    }

    #[test]
    fn a_title_of_a_playlist_says_what_it_is() {
        let entries = watchlist(&FakeServer::new(12, false));
        let lines: Vec<(String, String)> = entries
            .iter()
            .map(|entry| (name(&entry.title), caption(&entry.title)))
            .collect();
        assert!(matches!(entries[1].title, Title::Episode(_)));
        assert!(lines[1].0.contains(" · S1 · E2 · "), "{lines:?}");
        assert_eq!(lines[1].1, "S1 · E2");
        assert!(lines[2].1.starts_with("Series"), "{lines:?}");
    }

    #[test]
    fn a_list_counts_its_titles_in_english() {
        assert_eq!(titles(0), "0 titles");
        assert_eq!(titles(1), "1 title");
        assert_eq!(titles(3), "3 titles");
    }

    fn image(of: &str) -> ImageRef {
        ImageRef {
            item: ItemId(of.into()),
            kind: ImageKind::Primary,
            tag: "t".into(),
        }
    }

    #[test]
    fn the_mosaic_shows_the_first_four_posters_skipping_the_titles_without_one() {
        let posters = [
            Some(image("a")),
            None,
            Some(image("b")),
            Some(image("c")),
            Some(image("d")),
            Some(image("e")),
        ];
        let shown = mosaic(posters.iter().map(Option::as_ref));
        assert_eq!(shown, ["a", "b", "c", "d"].map(image));
        assert!(mosaic([None, None]).is_empty());
    }

    #[test]
    fn a_playlist_card_shows_the_posters_of_its_first_titles_in_order() {
        let server = FakeServer::new(12, false);
        let late = server
            .playlists()
            .unwrap()
            .into_iter()
            .find(|p| p.name == "Late night")
            .unwrap();
        let shown: Vec<ItemId> = playlist_mosaic(&late.entries)
            .into_iter()
            .map(|poster| poster.item)
            .collect();
        let first: Vec<ItemId> = ["film-5", "film-3", "film-8", "film-4"]
            .map(|id| ItemId(id.into()))
            .to_vec();
        assert_eq!(shown, first);
    }
}
