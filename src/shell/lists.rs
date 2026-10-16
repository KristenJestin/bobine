//! The Lists (the favorites and the playlists) and the collections: what the pages show of
//! them, and how it comes from the server. Every call runs off the window's thread, like the
//! others.

use bobine::provider::{
    CollectionCard, CollectionDetail, Favourite, ImageRef, ItemId, Playlist, ProviderError,
};
use gpui_kit::{Context, ScrollHandle, UniformListScrollHandle};

use super::Shell;
use super::pages::Loadable;
use crate::favourite::{self, ListPage, Toggle};
use crate::lists::PosterQueue;
use crate::navigation::Screen;
use crate::playlists;
use crate::poster_actions::Action;
use crate::series_page::Refreshes;

/// The collections of the server.
#[derive(Default)]
pub struct CollectionsPage {
    pub cards: Option<Loadable<Vec<CollectionCard>>>,
    pub scroll: UniformListScrollHandle,
    /// The row offered instead when the server has no collection.
    pub suggestions: ScrollHandle,
    /// The bare cards whose poster is still to ask, once the grid shows.
    posters: PosterQueue,
}

/// One collection and its films.
pub struct CollectionPage {
    pub id: ItemId,
    pub detail: Loadable<CollectionDetail>,
    pub scroll: ScrollHandle,
}

/// The user's list: films and series together.
#[derive(Default)]
pub struct MyListPage {
    pub items: Option<Loadable<Vec<Favourite>>>,
    pub scroll: UniformListScrollHandle,
    /// The row offered instead when the list holds nothing.
    pub suggestions: ScrollHandle,
    /// Each time the list is asked it is numbered: a read sent before a change was answered may
    /// land after the read that follows it, and must not overwrite it.
    asked: Refreshes,
}

impl MyListPage {
    /// The list is asked: its number, taken from `counter` like the series pages' refreshes.
    fn ask(&mut self, counter: &mut u64) -> u64 {
        self.asked.ask(counter)
    }

    /// The answer to the ask `number` came: it shows, unless a newer ask was made since.
    fn landed(&mut self, number: u64, items: Loadable<Vec<Favourite>>) -> bool {
        let newest = self.asked.is_newest(number);
        if newest {
            self.items = Some(items);
        }
        newest
    }
}

/// The Lists: the favorites' card, then a card for each playlist.
#[derive(Default)]
pub struct ListsPage {
    pub playlists: Option<Loadable<Vec<Playlist>>>,
    pub scroll: UniformListScrollHandle,
    /// Each time the playlists are asked, numbered like the favorites.
    asked: Refreshes,
}

/// What a card of the Lists opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opens {
    Favorites,
    Playlist(ItemId),
}

/// A card of the Lists, as it is drawn: a mosaic of its first posters, its name and its count.
#[derive(Clone, Debug)]
pub struct ListCard {
    pub name: String,
    pub caption: String,
    pub posters: Vec<ImageRef>,
    pub opens: Opens,
}

impl ListsPage {
    /// The cards: the favorites first, then the playlists the server gave, by name.
    pub fn cards(&self, favourites: Option<&Loadable<Vec<Favourite>>>) -> Vec<ListCard> {
        let favourites = match favourites {
            Some(Loadable::Ready(items)) => Some(items),
            _ => None,
        };
        let favorites = ListCard {
            name: "Favorites".into(),
            caption: favourites
                .map(|items| playlists::titles(items.len()))
                .unwrap_or_default(),
            posters: favourites
                .map(|items| {
                    playlists::mosaic(items.iter().map(|favourite| match favourite {
                        Favourite::Film(film) => film.poster.as_ref(),
                        Favourite::Series(series) => series.poster.as_ref(),
                    }))
                })
                .unwrap_or_default(),
            opens: Opens::Favorites,
        };
        let found = match &self.playlists {
            Some(Loadable::Ready(found)) => found.as_slice(),
            _ => &[],
        };
        std::iter::once(favorites)
            .chain(found.iter().map(|playlist| ListCard {
                name: playlist.name.clone(),
                caption: playlists::titles(playlist.entries.len()),
                posters: playlists::playlist_mosaic(&playlist.entries),
                opens: Opens::Playlist(playlist.id.clone()),
            }))
            .collect()
    }
}

/// The "My list" button of the player: the film playing, or the series of the episode.
pub struct PlayerList {
    pub item: ItemId,
    generation: u64,
    pub toggle: Toggle,
    /// Why the last press did not take.
    pub error: Option<&'static str>,
}

impl ListPage for PlayerList {
    fn item(&self) -> &ItemId {
        &self.item
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn button(&mut self) -> (&mut Toggle, &mut Option<&'static str>) {
        (&mut self.toggle, &mut self.error)
    }
}

impl<T> Loadable<T> {
    /// What shows while the server is asked again: what was drawn stays, a first or a failed
    /// load shows the wait.
    fn asking(slot: &mut Option<Loadable<T>>) {
        if !matches!(slot, Some(Loadable::Ready(_))) {
            *slot = Some(Loadable::Loading);
        }
    }
}

impl Shell {
    /// The "My list" button of the film or series page shown, or of the player: the button
    /// changes at once, the server is asked, and a failure puts the button back and says so.
    pub fn toggle_favourite(&mut self, cx: &mut Context<Self>) {
        let page: &mut dyn ListPage = match self.navigator.current() {
            Screen::Player => match self.playback.as_mut().and_then(|p| p.list.as_mut()) {
                Some(list) => list,
                None => return,
            },
            Screen::FilmDetail => match self.details.last_mut() {
                Some(page) => page,
                None => return,
            },
            Screen::SeriesDetail => match self.series_pages.last_mut() {
                Some(page) => page,
                None => return,
            },
            _ => return,
        };
        let Some(pressed) = favourite::press(page) else {
            return;
        };
        cx.notify();
        let asked = pressed.item.clone();
        let wanted = pressed.wanted;
        self.ask(
            cx,
            move |server| favourite::ask(server, &asked, wanted),
            move |shell, answer, cx| {
                if let Err(error) = &answer {
                    // An expired session brings the login back; the page says what failed.
                    shell.failed(error.clone(), cx);
                }
                let pages = shell
                    .details
                    .iter_mut()
                    .map(|page| page as &mut dyn ListPage)
                    .chain(
                        shell
                            .series_pages
                            .iter_mut()
                            .map(|page| page as &mut dyn ListPage),
                    )
                    .chain(
                        shell
                            .playback
                            .as_mut()
                            .and_then(|playback| playback.list.as_mut())
                            .map(|list| list as &mut dyn ListPage),
                    );
                let current = shell.navigator.current();
                if answer.is_ok() {
                    shell
                        .marks
                        .follow(&pressed.item, Action::MyList, pressed.wanted);
                }
                if favourite::answered(pages, current, &pressed, &answer) {
                    shell.load_my_list(cx);
                }
                // The page that pressed went back to what it showed, which another page's
                // change may have made out of date: it reads the server again.
                if let Some(page) = favourite::reread(&pressed, &answer) {
                    shell.read_again(page, cx);
                }
                cx.notify();
            },
        );
    }

    /// The player's "My list" button is for `item`: whether it is in the list is asked of the
    /// server.
    pub(super) fn follow_list(&mut self, item: ItemId, cx: &mut Context<Self>) {
        let generation = self.next_generation();
        let Some(playback) = &mut self.playback else {
            return;
        };
        let list = PlayerList {
            item,
            generation,
            toggle: Toggle::default(),
            error: None,
        };
        let read_at = list.toggle.changes();
        playback.list = Some(list);
        self.ask(
            cx,
            |server| server.favourites(),
            move |shell, found, cx| {
                // The list failing to load leaves the button out of the list: a press still works.
                let Ok(found) = found else {
                    return;
                };
                let Some(list) = shell
                    .playback
                    .as_mut()
                    .and_then(|playback| playback.list.as_mut())
                    .filter(|list| list.generation == generation)
                else {
                    return;
                };
                let listed = found.iter().any(|favourite| match favourite {
                    Favourite::Film(film) => film.id == list.item,
                    Favourite::Series(series) => series.id == list.item,
                });
                list.toggle.sync(listed, read_at);
                cx.notify();
            },
        );
    }

    /// Reads the film or series page numbered `generation` again, if it is still open.
    fn read_again(&mut self, generation: u64, cx: &mut Context<Self>) {
        if let Some(depth) = self
            .details
            .iter()
            .position(|p| p.generation() == generation)
        {
            self.load_detail(depth, false, cx);
        } else if let Some(depth) = self
            .series_pages
            .iter()
            .position(|p| p.generation() == generation)
        {
            self.load_series(depth, false, None, cx);
        }
    }

    /// The collections, once; asked again when the first try failed.
    pub fn load_collections(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.collections.cards,
            Some(Loadable::Ready(_) | Loadable::Loading)
        ) {
            return;
        }
        self.collections.cards = Some(Loadable::Loading);
        self.ask(
            cx,
            |server| server.collections(),
            |shell, cards, cx| match cards {
                Ok(cards) => {
                    let generation = shell.next_generation();
                    shell.collections.posters = PosterQueue::of(&cards, generation);
                    shell.collections.cards = Some(Loadable::Ready(cards));
                    for collection in shell.collections.posters.start() {
                        shell.load_collection_poster(collection, generation, cx);
                    }
                }
                Err(error) => {
                    shell.collections.cards = Some(Loadable::Failed(shell.failed(error, cx)));
                }
            },
        );
    }

    /// The poster of a bare card, asked after the grid shows. Each answer, a poster or not,
    /// sends the next request, so a slow server never has more than a few in flight. The chain
    /// belongs to the load numbered `generation`: it goes on after the page is left, and an
    /// answer of an earlier load (signed in again) neither wears a card nor sends a request.
    fn load_collection_poster(
        &mut self,
        collection: ItemId,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let asked = collection.clone();
        // Without a server (the login is shown) `ask` calls nothing back and the chain stops
        // there: the only way out of the login is `signed_in`, which resets the queue.
        self.ask(
            cx,
            move |server| server.collection_poster(&asked),
            move |shell, poster: Option<ImageRef>, cx| {
                let Some(Loadable::Ready(cards)) = &mut shell.collections.cards else {
                    return;
                };
                let next =
                    shell
                        .collections
                        .posters
                        .answered(cards, generation, &collection, poster);
                if let Some(next) = next {
                    shell.load_collection_poster(next, generation, cx);
                }
            },
        );
    }

    /// The user's list, each time it shows: a title put in it from a page shows at once.
    pub fn load_my_list(&mut self, cx: &mut Context<Self>) {
        Loadable::asking(&mut self.my_list.items);
        let number = self.my_list.ask(&mut self.refreshes_asked);
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            |server| server.favourites(),
            move |shell, items, cx| {
                let items = match items {
                    Ok(items) => {
                        shell.marks.listed(stamp, &items);
                        Loadable::Ready(items)
                    }
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                shell.my_list.landed(number, items);
            },
        );
    }

    /// The Lists, each time they show: the favorites, and the playlists.
    pub fn load_lists(&mut self, cx: &mut Context<Self>) {
        self.load_my_list(cx);
        Loadable::asking(&mut self.lists.playlists);
        let number = self.lists.asked.ask(&mut self.refreshes_asked);
        self.ask(
            cx,
            |server| server.playlists(),
            move |shell, found, cx| {
                let found = match found {
                    Ok(found) => Loadable::Ready(found),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                if shell.lists.asked.is_newest(number) {
                    shell.lists.playlists = Some(found);
                }
            },
        );
    }

    /// The favorites' card of the Lists.
    pub fn open_favorites(&mut self, cx: &mut Context<Self>) {
        self.forward.clear();
        self.navigator.open(Screen::Favorites);
        self.load_my_list(cx);
        cx.notify();
    }

    pub fn open_collection(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.forward.clear();
        self.collection_pages.push(CollectionPage {
            id,
            detail: Loadable::Loading,
            scroll: ScrollHandle::new(),
        });
        self.navigator.open(Screen::CollectionDetail);
        self.load_collection_page(self.collection_pages.len() - 1, cx);
        cx.notify();
    }

    /// Back on a collection page from a film: its films' marks are asked again.
    pub(super) fn reload_collection(&mut self, cx: &mut Context<Self>) {
        if let Some(depth) = self.collection_pages.len().checked_sub(1) {
            self.load_collection_page(depth, cx);
        }
    }

    fn load_collection_page(&mut self, depth: usize, cx: &mut Context<Self>) {
        let id = self.collection_pages[depth].id.clone();
        let asked = id.clone();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            move |server| server.collection(&asked),
            move |shell, detail: Result<CollectionDetail, ProviderError>, cx| {
                let detail = match detail {
                    Ok(detail) => {
                        shell.marks.listed(stamp, &detail.films);
                        Loadable::Ready(detail)
                    }
                    // What was drawn stays when the refresh fails.
                    Err(error) => {
                        let said = shell.failed(error, cx);
                        match shell.collection_pages.get(depth) {
                            Some(page) if matches!(page.detail, Loadable::Ready(_)) => return,
                            _ => Loadable::Failed(said),
                        }
                    }
                };
                if let Some(page) = shell
                    .collection_pages
                    .get_mut(depth)
                    .filter(|page| page.id == id)
                {
                    page.detail = detail;
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeServer;
    use crate::navigation::NavEntry;
    use gpui_kit::{TestAppContext, WindowHandle};
    use std::sync::Arc;

    /// The Home of a fake server, signed in.
    fn signed_in(cx: &mut TestAppContext) -> (FakeServer, WindowHandle<Shell>) {
        let server = FakeServer::new(12, true);
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        (server, window)
    }

    #[gpui_kit::test]
    fn favorites_and_playlists_are_listed_on_the_lists_page(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::MyList, cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.navigator.current(), Screen::MyList);
                let cards = shell.lists.cards(shell.my_list.items.as_ref());
                let names: Vec<&str> = cards.iter().map(|card| card.name.as_str()).collect();
                // The favorites first, then each playlist by name.
                assert_eq!(names, ["Favorites", "Late night", "Watchlist"]);
                assert_eq!(cards[0].caption, "4 titles");
                assert_eq!(cards[1].caption, "5 titles");
                assert_eq!(cards[1].posters.len(), 4, "a two by two mosaic");
                assert_eq!(cards[2].caption, "4 titles");
                assert_eq!(cards[0].opens, Opens::Favorites);
                assert!(matches!(&cards[1].opens, Opens::Playlist(id) if id.0 == "playlist-late"));
            })
            .unwrap();
    }

    fn shown(page: &MyListPage) -> Option<usize> {
        match &page.items {
            Some(Loadable::Ready(items)) => Some(items.len()),
            _ => None,
        }
    }

    #[test]
    fn my_list_shows_the_change_made_just_before() {
        let mut counter = 0;
        let mut page = MyListPage::default();
        // My list → film → remove → Escape: the list is asked while the server takes the
        // change, and asked again when the change is answered.
        let during = page.ask(&mut counter);
        let after = page.ask(&mut counter);
        // The read after the change comes first, the one sent during it lands late.
        assert!(page.landed(after, Loadable::Ready(Vec::new())));
        assert!(!page.landed(during, Loadable::Failed("old")));
        assert_eq!(shown(&page), Some(0), "the late old read is dropped");
    }

    #[test]
    fn the_lists_asked_in_order_show_the_last() {
        let mut counter = 0;
        let mut page = MyListPage::default();
        let first = page.ask(&mut counter);
        assert!(page.landed(first, Loadable::Ready(Vec::new())));
        assert_eq!(shown(&page), Some(0));
        let second = page.ask(&mut counter);
        assert!(page.landed(second, Loadable::Failed("later")));
        assert!(matches!(page.items, Some(Loadable::Failed("later"))));
    }
}
