//! The words of the Lists page and of a playlist, and what they keep of the server's answer.

use bobine::provider::{EntryId, ImageRef, ItemId, Playlist, PlaylistEntry, ProviderError, Title};

use crate::lists::plural;
use crate::series_page::{self, Next};
use crate::shell::Loadable;

/// What the window says when the server did not take a change of a playlist.
pub const FAILED: &str = "The playlist could not be changed.";

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

/// Whether `entries` hold `item`: the title itself, or, for a series, one of its episodes (a
/// series put in a playlist is put in as its episodes).
pub fn holds(entries: &[PlaylistEntry], item: &ItemId) -> bool {
    entries.iter().any(|entry| match &entry.title {
        Title::Episode(episode) => &episode.item.id == item || &episode.series == item,
        title => title.id() == item,
    })
}

/// A line of the "Add to playlist" menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// None for a playlist being made, until the server gives its id.
    pub id: Option<ItemId>,
    pub name: String,
    /// The playlist holds the title: the line takes no press.
    pub checked: bool,
}

/// A playlist asked to be made from the menu, with the title in it.
#[derive(Debug, Clone)]
struct Making {
    token: u32,
    name: String,
    id: Option<ItemId>,
}

/// The "Add to playlist" menu of one title: the playlists, a check on those that hold it. A
/// press shows the check at once and asks the server; a refusal takes it back and says why.
pub struct Picker {
    pub item: ItemId,
    pub playlists: Option<Loadable<Vec<Playlist>>>,
    /// The playlists the title was put in from here: checked while asked, and after.
    added: Vec<ItemId>,
    /// Those whose answer has not come.
    pending: Vec<ItemId>,
    making: Vec<Making>,
    made: u32,
    /// Why the last change did not take.
    pub error: Option<&'static str>,
}

impl Picker {
    pub fn new(item: ItemId) -> Self {
        Self {
            item,
            playlists: Some(Loadable::Loading),
            added: Vec::new(),
            pending: Vec::new(),
            making: Vec::new(),
            made: 0,
            error: None,
        }
    }

    /// The lines: the playlists by name, then those being made.
    pub fn lines(&self) -> Vec<Line> {
        let found = match &self.playlists {
            Some(Loadable::Ready(found)) => found.as_slice(),
            _ => &[],
        };
        found
            .iter()
            .map(|playlist| Line {
                id: Some(playlist.id.clone()),
                name: playlist.name.clone(),
                checked: self.added.contains(&playlist.id) || holds(&playlist.entries, &self.item),
            })
            .chain(self.making.iter().map(|making| Line {
                id: making.id.clone(),
                name: making.name.clone(),
                checked: true,
            }))
            .collect()
    }

    /// A press on the playlist `id`: checked at once. Returns the playlist to ask the server to
    /// put the title in; none when it already holds it.
    pub fn pick(&mut self, id: &ItemId) -> Option<ItemId> {
        let line = self
            .lines()
            .into_iter()
            .find(|line| line.id.as_ref() == Some(id))?;
        if line.checked {
            return None;
        }
        self.error = None;
        self.added.push(id.clone());
        self.pending.push(id.clone());
        Some(id.clone())
    }

    /// The server answered the press on `id`. A refusal takes the check back; returns what to
    /// tell the viewer then.
    pub fn picked(
        &mut self,
        id: &ItemId,
        answer: &Result<(), ProviderError>,
    ) -> Option<&'static str> {
        self.pending.retain(|pending| pending != id);
        if answer.is_ok() {
            return None;
        }
        self.added.retain(|added| added != id);
        self.error = Some(FAILED);
        self.error
    }

    /// "New playlist…" with `name` typed: the line shows at once, checked. Returns what to ask
    /// the server, with the token its answer comes back with; none for a name of only spaces.
    pub fn make(&mut self, name: &str) -> Option<(u32, String)> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.error = None;
        self.made += 1;
        self.making.push(Making {
            token: self.made,
            name: name.into(),
            id: None,
        });
        Some((self.made, name.into()))
    }

    /// The server answered the playlist made with `token`: the line takes its id, or goes on a
    /// refusal; returns what to tell the viewer then.
    pub fn made(
        &mut self,
        token: u32,
        answer: &Result<ItemId, ProviderError>,
    ) -> Option<&'static str> {
        match answer {
            Ok(id) => {
                if let Some(making) = self.making.iter_mut().find(|m| m.token == token) {
                    making.id = Some(id.clone());
                }
                None
            }
            Err(_) => {
                self.making.retain(|making| making.token != token);
                self.error = Some(FAILED);
                self.error
            }
        }
    }
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

    fn picker_of(item: &str) -> Picker {
        let mut picker = Picker::new(ItemId(item.into()));
        let server = FakeServer::new(12, false);
        picker.playlists = Some(Loadable::Ready(server.playlists().unwrap()));
        picker
    }

    fn checked(picker: &Picker) -> Vec<(String, bool)> {
        picker
            .lines()
            .into_iter()
            .map(|line| (line.name, line.checked))
            .collect()
    }

    #[test]
    fn the_menu_checks_the_playlists_that_already_hold_the_title() {
        let picker = picker_of("film-3");
        assert_eq!(
            checked(&picker),
            [("Late night".into(), true), ("Watchlist".into(), false)]
        );
        // A series is held through its episodes as well.
        let picker = picker_of("series-1");
        assert_eq!(
            checked(&picker),
            [("Late night".into(), false), ("Watchlist".into(), true)]
        );
    }

    #[test]
    fn a_pick_is_checked_at_once_and_unchecked_when_refused() {
        let mut picker = picker_of("film-6");
        let watchlist = ItemId("playlist-watchlist".into());
        assert_eq!(picker.pick(&watchlist), Some(watchlist.clone()));
        assert!(checked(&picker)[1].1);
        assert_eq!(picker.pick(&watchlist), None, "already checked");
        assert_eq!(
            picker.picked(&watchlist, &Err(ProviderError::Forbidden)),
            Some(FAILED)
        );
        assert!(!checked(&picker)[1].1);
        assert_eq!(picker.error, Some(FAILED));
        // Picked again, it is asked again, and the word goes.
        assert_eq!(picker.pick(&watchlist), Some(watchlist.clone()));
        assert_eq!(picker.error, None);
        assert_eq!(picker.picked(&watchlist, &Ok(())), None);
        assert!(checked(&picker)[1].1);
    }

    #[test]
    fn a_new_playlist_shows_at_once_and_goes_when_refused() {
        let mut picker = picker_of("film-6");
        assert_eq!(picker.make("   "), None);
        let (first, name) = picker.make("  Road trip ").unwrap();
        assert_eq!(name, "Road trip");
        assert_eq!(checked(&picker)[2], ("Road trip".into(), true));
        assert_eq!(picker.lines()[2].id, None);
        assert_eq!(picker.made(first, &Ok(ItemId("new".into()))), None);
        assert_eq!(picker.lines()[2].id, Some(ItemId("new".into())));
        let (second, _) = picker.make("Snow days").unwrap();
        assert_eq!(
            picker.made(second, &Err(ProviderError::Server(500))),
            Some(FAILED)
        );
        assert_eq!(picker.lines().len(), 3, "the refused one is gone");
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
