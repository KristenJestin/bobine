//! The words of the Lists page and of a playlist, and what they keep of the server's answer.

use bobine::provider::{EntryId, ImageRef, ItemId, Playlist, PlaylistEntry, ProviderError, Title};

use crate::lists::plural;
use crate::series_page::{self, Next};
use crate::shell::Loadable;

/// What the window says when the server did not take a change of a playlist.
pub const FAILED: &str = "The playlist could not be changed.";
/// What it says when the server answers that this user may not change it.
pub const FORBIDDEN: &str = "You can't change this playlist.";
/// What the Lists say when the user has no playlist.
pub const NONE: &str =
    "No playlists yet. Choose “Add to playlist” on a movie or a series to make one.";
/// What a playlist's page says when it holds nothing.
pub const EMPTY: &str = "Add a movie, a series or an episode from its page.";

/// Why a change of a playlist did not take, for the viewer.
pub fn refusal(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::Forbidden => FORBIDDEN,
        _ => FAILED,
    }
}

/// What the menu says once a series went in, as its episodes; nothing for a movie or an
/// episode, whatever the server counted.
pub fn added(series: bool, count: usize) -> Option<String> {
    (series && count > 1).then(|| format!("Added {count} episodes."))
}

/// How many posters a card's mosaic shows: two by two.
pub const MOSAIC: usize = 4;

/// `0 titles`, `1 title`, `3 titles`.
pub fn titles(count: usize) -> String {
    plural(count, "title", "titles")
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

/// The posters of a playlist's card: an episode shows its series' poster.
pub fn playlist_mosaic(entries: &[PlaylistEntry]) -> Vec<ImageRef> {
    mosaic(entries.iter().map(|entry| entry.poster.as_ref()))
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

/// What the card offers once `entry` ended: the entry after it, as the page shows them now;
/// none after the last, or once `entry` is no longer there.
pub fn after(entries: &[PlaylistEntry], entry: &EntryId) -> Option<Next> {
    let at = entries.iter().position(|shown| &shown.id == entry)?;
    entries.get(at + 1).cloned().map(Next::Entry)
}

/// Takes `entry` out of what the page shows, at once; whether it was there.
pub fn take_out(entries: &mut Vec<PlaylistEntry>, entry: &EntryId) -> bool {
    let before = entries.len();
    entries.retain(|shown| &shown.id != entry);
    entries.len() < before
}

/// Whether `entries` hold `item` itself. A series is never held: it goes in as its episodes,
/// and one of them in a playlist does not put the series there.
pub fn holds(entries: &[PlaylistEntry], item: &ItemId) -> bool {
    entries.iter().any(|entry| entry.title.id() == item)
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
    /// The title is a series: it goes in as its episodes.
    series: bool,
    pub playlists: Option<Loadable<Vec<Playlist>>>,
    /// The playlists the title was put in from here: checked while asked, and after.
    added: Vec<ItemId>,
    /// Those whose answer has not come.
    pending: Vec<ItemId>,
    making: Vec<Making>,
    made: u32,
    /// Why the last change did not take.
    pub error: Option<&'static str>,
    /// What the last change put in, when it was more than the title: a series' episodes.
    pub told: Option<String>,
}

impl Picker {
    pub fn new(item: ItemId, series: bool) -> Self {
        Self {
            item,
            series,
            playlists: Some(Loadable::Loading),
            added: Vec::new(),
            pending: Vec::new(),
            making: Vec::new(),
            made: 0,
            error: None,
            told: None,
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
        self.told = None;
        self.added.push(id.clone());
        self.pending.push(id.clone());
        Some(id.clone())
    }

    /// The server answered the press on `id`: how many entries it put in, said when a series
    /// went in as its episodes. A refusal takes the check back; returns what to tell the viewer
    /// then.
    pub fn picked(
        &mut self,
        id: &ItemId,
        answer: &Result<usize, ProviderError>,
    ) -> Option<&'static str> {
        self.pending.retain(|pending| pending != id);
        match answer {
            Ok(count) => {
                self.told = added(self.series, *count);
                None
            }
            Err(error) => {
                self.added.retain(|added| added != id);
                self.error = Some(refusal(error));
                self.error
            }
        }
    }

    /// "New playlist…" with `name` typed: the line shows at once, checked. Returns what to ask
    /// the server, with the token its answer comes back with; none for a name of only spaces.
    pub fn make(&mut self, name: &str) -> Option<(u32, String)> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        self.error = None;
        self.told = None;
        self.made += 1;
        self.making.push(Making {
            token: self.made,
            name: name.into(),
            id: None,
        });
        Some((self.made, name.into()))
    }

    /// The server answered the playlist made with `token`: the line takes its id, and a series
    /// says how many episodes went in; or the line goes on a refusal: returns what to tell the
    /// viewer then.
    pub fn made(
        &mut self,
        token: u32,
        answer: &Result<(ItemId, usize), ProviderError>,
    ) -> Option<&'static str> {
        match answer {
            Ok((id, count)) => {
                self.told = added(self.series, *count);
                if let Some(making) = self.making.iter_mut().find(|m| m.token == token) {
                    making.id = Some(id.clone());
                }
                None
            }
            Err(error) => {
                self.making.retain(|making| making.token != token);
                self.error = Some(refusal(error));
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
        assert_eq!(
            after(&entries, &entries[0].id),
            Some(Next::Entry(entries[1].clone()))
        );
        assert_eq!(after(&entries, &entries.last().unwrap().id), None);
        assert_eq!(after(&entries, &EntryId("gone".into())), None);
    }

    #[test]
    fn an_entry_taken_out_leaves_the_page_at_once() {
        let mut entries = watchlist(&FakeServer::new(12, false));
        let before = entries.clone();
        assert!(take_out(&mut entries, &before[1].id));
        assert_eq!(entries.len(), before.len() - 1);
        assert!(!entries.contains(&before[1]));
        assert!(!take_out(&mut entries, &EntryId("gone".into())));
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
        assert_eq!(lines[2].1, "S1 · E1");
    }

    fn picker_of(item: &str, series: bool) -> Picker {
        let mut picker = Picker::new(ItemId(item.into()), series);
        let server = FakeServer::new(12, false);
        picker.playlists = Some(Loadable::Ready(server.playlists(None).unwrap()));
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
        let picker = picker_of("film-3", false);
        assert_eq!(
            checked(&picker),
            [("Late night".into(), true), ("Watchlist".into(), false)]
        );
        let picker = picker_of("series-1-s1-e2", false);
        assert_eq!(
            checked(&picker),
            [("Late night".into(), false), ("Watchlist".into(), true)]
        );
    }

    #[test]
    fn an_episode_in_a_playlist_does_not_check_its_series() {
        let picker = picker_of("series-1", true);
        assert_eq!(
            checked(&picker),
            [("Late night".into(), false), ("Watchlist".into(), false)]
        );
    }

    #[test]
    fn a_series_added_says_how_many_episodes_went_in() {
        let mut picker = picker_of("series-3", true);
        let late = ItemId("playlist-late".into());
        picker.pick(&late);
        assert_eq!(picker.picked(&late, &Ok(24)), None);
        assert_eq!(picker.told.as_deref(), Some("Added 24 episodes."));
    }

    #[test]
    fn a_movie_or_an_episode_never_says_added_episodes_whatever_the_count() {
        let late = ItemId("playlist-late".into());
        for item in ["film-6", "series-1-s1-e1"] {
            let mut picker = picker_of(item, false);
            picker.pick(&late);
            picker.picked(&late, &Ok(24));
            assert_eq!(picker.told, None, "{item}");
            let (token, _) = picker.make("Road trip").unwrap();
            picker.made(token, &Ok((ItemId("new".into()), 24)));
            assert_eq!(picker.told, None, "{item}, new playlist");
        }
    }

    #[test]
    fn a_new_playlist_made_from_a_series_says_how_many_episodes_went_in() {
        let mut picker = picker_of("series-3", true);
        let (token, _) = picker.make("Road trip").unwrap();
        assert_eq!(picker.made(token, &Ok((ItemId("new".into()), 24))), None);
        assert_eq!(picker.told.as_deref(), Some("Added 24 episodes."));
    }

    #[test]
    fn a_refusal_says_you_cant_change_this_playlist_on_a_403() {
        assert_eq!(
            refusal(&ProviderError::Forbidden),
            "You can't change this playlist."
        );
        assert_eq!(refusal(&ProviderError::Server(500)), FAILED);
        assert_eq!(refusal(&ProviderError::NotFound), FAILED);
    }

    #[test]
    fn a_pick_is_checked_at_once_and_unchecked_when_refused() {
        let mut picker = picker_of("film-6", false);
        let watchlist = ItemId("playlist-watchlist".into());
        assert_eq!(picker.pick(&watchlist), Some(watchlist.clone()));
        assert!(checked(&picker)[1].1);
        assert_eq!(picker.pick(&watchlist), None, "already checked");
        assert_eq!(
            picker.picked(&watchlist, &Err(ProviderError::Forbidden)),
            Some(FORBIDDEN)
        );
        assert!(!checked(&picker)[1].1);
        assert_eq!(picker.error, Some(FORBIDDEN));
        // Picked again, it is asked again, and the word goes.
        assert_eq!(picker.pick(&watchlist), Some(watchlist.clone()));
        assert_eq!(picker.error, None);
        assert_eq!(picker.picked(&watchlist, &Ok(1)), None);
        assert!(checked(&picker)[1].1);
    }

    #[test]
    fn a_new_playlist_shows_at_once_and_goes_when_refused() {
        let mut picker = picker_of("film-6", false);
        assert_eq!(picker.make("   "), None);
        let (first, name) = picker.make("  Road trip ").unwrap();
        assert_eq!(name, "Road trip");
        assert_eq!(checked(&picker)[2], ("Road trip".into(), true));
        assert_eq!(picker.lines()[2].id, None);
        assert_eq!(picker.made(first, &Ok((ItemId("new".into()), 1))), None);
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
            .playlists(None)
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

    #[test]
    fn an_episode_shows_its_series_poster_in_the_mosaic() {
        let entries = watchlist(&FakeServer::new(12, false));
        let shown: Vec<ItemId> = playlist_mosaic(&entries)
            .into_iter()
            .map(|poster| poster.item)
            .collect();
        let posters: Vec<ItemId> = ["film-1", "series-1", "series-2", "film-0"]
            .map(|id| ItemId(id.into()))
            .to_vec();
        assert_eq!(shown, posters);
    }
}
