//! A playlist's page: its titles in their order, each played or opened, "Play all", and an
//! entry taken out. Every call to the server runs off the window's thread, like the others.

use std::time::Instant;

use bobine::provider::{EntryId, ItemId, PlaylistEntry, Title};
use gpui_kit::{
    AppContext, Bounds, Context, Entity, Focusable, Pixels, Point, UniformListScrollHandle, Window,
};

use super::pages::{Loadable, Origin};
use super::{Film as Playing, Shell};
use crate::home_page::{self, PlayTarget, StartError};
use crate::input::TextInput;
use crate::motion::{self, Fade};
use crate::navigation::Screen;
use crate::playlists::{self, Picker};

pub struct PlaylistPage {
    pub id: ItemId,
    pub name: String,
    pub entries: Loadable<Vec<PlaylistEntry>>,
    pub scroll: UniformListScrollHandle,
    /// A title is being opened for the player: nothing else starts.
    pub busy: bool,
}

/// The "Add to playlist" menu, while it shows (it stays fading out after it closed).
pub struct PlaylistMenu {
    pub picker: Picker,
    /// Where the button that opened it was drawn; none when it was opened otherwise.
    pub anchor: Option<Bounds<Pixels>>,
    /// The screen it was opened on: it goes when another shows.
    screen: Screen,
    /// Closed means fading out.
    pub open: bool,
    pub fade: Fade,
    /// The name of a new playlist, once "New playlist…" was pressed.
    pub naming: Option<Entity<TextInput>>,
}

impl Shell {
    pub fn playlist_menu_is_open(&self) -> bool {
        self.playlist_menu.as_ref().is_some_and(|menu| menu.open)
    }

    /// "Add to playlist" for `item`, from the button drawn at `anchor`: the menu opens under it
    /// and the playlists are asked; pressed again, it closes.
    pub fn open_playlist_menu(
        &mut self,
        item: ItemId,
        anchor: Option<Bounds<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        if self
            .playlist_menu
            .as_ref()
            .is_some_and(|menu| menu.open && menu.picker.item == item)
        {
            self.close_playlist_menu(cx);
            return;
        }
        let mut fade = Fade::new(motion::quick());
        fade.set(true, Instant::now());
        self.playlist_menu = Some(PlaylistMenu {
            picker: Picker::new(item.clone()),
            anchor,
            screen: self.navigator.current(),
            open: true,
            fade,
            naming: None,
        });
        self.ask(
            cx,
            |server| server.playlists(),
            move |shell, found, cx| {
                let found = match found {
                    Ok(found) => Loadable::Ready(found),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                if let Some(menu) = &mut shell.playlist_menu
                    && menu.picker.item == item
                {
                    menu.picker.playlists = Some(found);
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    /// The menu fades out; the keys go back to the window.
    pub fn close_playlist_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.playlist_menu
            && menu.open
        {
            menu.open = false;
            menu.naming = None;
            menu.fade.set(false, Instant::now());
            cx.notify();
        }
    }

    /// The mouse went down outside the menu: it closes, unless it is on the button that opened
    /// it, whose click closes it.
    pub fn pressed_outside_playlist_menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let on_button = self
            .playlist_menu
            .as_ref()
            .and_then(|menu| menu.anchor)
            .is_some_and(|button| button.contains(&at));
        if !on_button {
            self.close_playlist_menu(cx);
        }
    }

    /// What the menu still is at a draw: gone once faded out, or once another screen shows.
    pub(super) fn follow_playlist_menu(&mut self, window: &mut Window) {
        let screen = self.navigator.current();
        let now = Instant::now();
        let Some(menu) = &mut self.playlist_menu else {
            return;
        };
        if menu.screen != screen {
            self.playlist_menu = None;
            return;
        }
        if menu.fade.moving(now) {
            window.request_animation_frame();
        } else if !menu.open {
            self.playlist_menu = None;
        }
    }

    /// A press on a playlist of the menu: checked at once, asked of the server.
    pub fn pick_playlist(&mut self, playlist: ItemId, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.playlist_menu else {
            return;
        };
        let Some(asked) = menu.picker.pick(&playlist) else {
            return;
        };
        let item = menu.picker.item.clone();
        let added = item.clone();
        cx.notify();
        self.ask(
            cx,
            move |server| server.add_to_playlist(&asked, &added),
            move |shell, answer, cx| {
                if let Err(error) = &answer {
                    shell.failed(error.clone(), cx);
                }
                let said = shell
                    .playlist_menu
                    .as_mut()
                    .filter(|menu| menu.picker.item == item)
                    .and_then(|menu| menu.picker.picked(&playlist, &answer));
                if answer.is_err() {
                    shell.show_notice(said.unwrap_or(playlists::FAILED), cx);
                }
                cx.notify();
            },
        );
    }

    /// "New playlist…": a field for its name, which takes the keys.
    pub fn start_naming(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.playlist_menu else {
            return;
        };
        let input = cx.new(|cx| TextInput::new("Playlist name", false, cx));
        window.focus(&input.focus_handle(cx), cx);
        menu.naming = Some(input);
        cx.notify();
    }

    /// Enter in the name's field: the playlist is made with the title in it, its line checked
    /// at once.
    pub fn make_playlist(&mut self, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.playlist_menu else {
            return;
        };
        let Some(input) = menu.naming.take() else {
            return;
        };
        let name = input.read(cx).text().to_string();
        let Some((token, name)) = menu.picker.make(&name) else {
            menu.naming = Some(input);
            return;
        };
        let item = menu.picker.item.clone();
        let added = item.clone();
        cx.notify();
        self.ask(
            cx,
            move |server| server.create_playlist(&name, &added),
            move |shell, answer, cx| {
                if let Err(error) = &answer {
                    shell.failed(error.clone(), cx);
                }
                let said = shell
                    .playlist_menu
                    .as_mut()
                    .filter(|menu| menu.picker.item == item)
                    .and_then(|menu| menu.picker.made(token, &answer));
                if answer.is_err() {
                    shell.show_notice(said.unwrap_or(playlists::FAILED), cx);
                }
                cx.notify();
            },
        );
    }

    /// A key while the menu is open: Escape closes it, Enter makes the playlist named; the
    /// others are the name field's, never the page's or the player's.
    pub(super) fn playlist_menu_key(&mut self, key: &str, cx: &mut Context<Self>) {
        match key {
            "escape" => self.close_playlist_menu(cx),
            "enter" => self.make_playlist(cx),
            _ => {}
        }
    }

    /// A playlist's card of the Lists.
    pub fn open_playlist(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.forward.clear();
        let name = match &self.lists.playlists {
            Some(Loadable::Ready(found)) => found
                .iter()
                .find(|playlist| playlist.id == id)
                .map(|playlist| playlist.name.clone()),
            _ => None,
        };
        self.playlist_pages.push(PlaylistPage {
            id,
            name: name.unwrap_or_default(),
            entries: Loadable::Loading,
            scroll: UniformListScrollHandle::new(),
            busy: false,
        });
        self.navigator.open(Screen::Playlist);
        self.load_playlist_page(self.playlist_pages.len() - 1, cx);
        cx.notify();
    }

    /// The playlist's page at `depth`, if it still shows `id`.
    pub(super) fn playlist_page(&mut self, depth: usize, id: &ItemId) -> Option<&mut PlaylistPage> {
        self.playlist_pages
            .get_mut(depth)
            .filter(|page| &page.id == id)
    }

    /// What the playlist at `depth` holds, asked anew; what was drawn stays if it fails.
    pub(super) fn load_playlist_page(&mut self, depth: usize, cx: &mut Context<Self>) {
        let id = self.playlist_pages[depth].id.clone();
        let asked = id.clone();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            move |server| server.playlist(&asked),
            move |shell, entries, cx| {
                let entries = match entries {
                    Ok(entries) => {
                        shell
                            .marks
                            .listed(stamp, entries.iter().map(|entry| &entry.title));
                        Loadable::Ready(entries)
                    }
                    Err(error) => {
                        let said = shell.failed(error, cx);
                        match shell.playlist_page(depth, &id) {
                            Some(page) if matches!(page.entries, Loadable::Ready(_)) => return,
                            _ => Loadable::Failed(said),
                        }
                    }
                };
                if let Some(page) = shell.playlist_page(depth, &id) {
                    page.entries = entries;
                }
            },
        );
    }

    /// Back on the playlist's page on top: its titles' state may have changed.
    pub(super) fn reload_playlist(&mut self, cx: &mut Context<Self>) {
        if let Some(depth) = self.playlist_pages.len().checked_sub(1) {
            self.load_playlist_page(depth, cx);
        }
    }

    /// "Play all": the first title, then each after it through the card.
    pub fn play_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.play_entry(0, window, cx);
    }

    /// Plays the title at `at` of the playlist on top: a film or an episode itself, a series its
    /// next episode. The title after it is kept for the card at its end.
    pub fn play_entry(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let depth = self.playlist_pages.len().wrapping_sub(1);
        let Some(page) = self.playlist_pages.last_mut() else {
            return;
        };
        let Loadable::Ready(entries) = &page.entries else {
            return;
        };
        if page.busy {
            return;
        }
        let Some(entry) = entries.get(at) else {
            return;
        };
        let Some(server) = self.server.clone() else {
            return;
        };
        let target = PlayTarget::of(&entry.title);
        let next = playlists::after(entries, at);
        let origin = Origin::Playlist(depth, page.id.clone());
        page.busy = true;
        self.up_next.cancel();
        self.up_next_ticker = None;
        let task = cx
            .background_executor()
            .spawn(async move { home_page::start(&*server, &target) });
        cx.spawn_in(window, async move |this, cx| {
            let started = task.await;
            this.update_in(cx, |shell, window, cx| {
                match started {
                    Ok(started) => {
                        let playing = Playing {
                            title: started.title,
                            subtitle: Some(started.subtitle),
                            list: Some(started.list),
                        };
                        shell.start_playback(
                            origin,
                            started.plan,
                            started.opened,
                            playing,
                            next,
                            window,
                            cx,
                        );
                    }
                    Err(StartError::Server(error)) => {
                        let said = shell.failed(error, cx);
                        shell.tried(&origin, Some(said), cx);
                    }
                    Err(StartError::NothingToPlay) => {
                        shell.tried(&origin, Some("Nothing to play for this title."), cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// A click on a title of the playlist: its page; an episode's series, on its season.
    pub fn open_entry(&mut self, title: Title, cx: &mut Context<Self>) {
        match title {
            Title::Film(film) => self.open_film(film.id, cx),
            Title::Series(series) => self.open_series(series.id, cx),
            Title::Episode(episode) => {
                self.open_series_on(episode.series, Some(episode.season), cx)
            }
        }
    }

    /// Takes `entry` out of the playlist on top: it leaves the page at once, and comes back to
    /// its place, with a word, if the server refuses.
    pub fn remove_entry(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let depth = self.playlist_pages.len().wrapping_sub(1);
        let Some(page) = self.playlist_pages.last_mut() else {
            return;
        };
        let Loadable::Ready(entries) = &mut page.entries else {
            return;
        };
        let Some((at, gone)) = playlists::take_out(entries, &entry) else {
            return;
        };
        let id = page.id.clone();
        let playlist = id.clone();
        cx.notify();
        self.ask(
            cx,
            move |server| server.remove_from_playlist(&playlist, &entry),
            move |shell, answer, cx| {
                let Err(error) = answer else {
                    return;
                };
                shell.failed(error, cx);
                if let Some(page) = shell.playlist_page(depth, &id)
                    && let Loadable::Ready(entries) = &mut page.entries
                {
                    playlists::put_back(entries, at, gone);
                }
                shell.show_notice(playlists::FAILED, cx);
                cx.notify();
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeServer, PlaylistCall};
    use bobine::provider::{MediaServer, ProviderError};
    use gpui_kit::{TestAppContext, WindowHandle};
    use std::sync::Arc;

    const WATCHLIST: &str = "playlist-watchlist";

    /// The page of the Watchlist, opened from the Lists, of a server first changed by `change`.
    fn on_watchlist(
        cx: &mut TestAppContext,
        change: impl FnOnce(&mut crate::fake::State),
    ) -> (FakeServer, WindowHandle<Shell>) {
        let server = FakeServer::new(12, true);
        // No player starts in a test: what was asked to play is what it would have played.
        server.state.lock().unwrap().play_error = Some(ProviderError::Unreachable("no".into()));
        change(&mut server.state.lock().unwrap());
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server.clone());
        let window =
            cx.add_window(|_, cx| Shell::new(None, Box::new(move || Ok(connector)), None, cx));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                shell.select(crate::navigation::NavEntry::MyList, cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                shell.open_playlist(ItemId(WATCHLIST.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
        (server, window)
    }

    fn shown(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<PlaylistEntry> {
        window
            .read_with(cx, |shell, _| {
                match &shell.playlist_pages.last().unwrap().entries {
                    Loadable::Ready(entries) => entries.clone(),
                    _ => panic!("the playlist is not shown"),
                }
            })
            .unwrap()
    }

    fn played(server: &FakeServer) -> Vec<ItemId> {
        let state = server.state.lock().unwrap();
        state
            .record
            .plays
            .iter()
            .map(|(id, _)| id.clone())
            .collect()
    }

    #[gpui_kit::test]
    fn a_playlist_opens_with_its_items_in_order_and_play_all_plays_them_in_sequence(
        cx: &mut TestAppContext,
    ) {
        let (server, window) = on_watchlist(cx, |_| {});
        let entries = shown(&window, cx);
        assert_eq!(entries, server.playlist(&ItemId(WATCHLIST.into())).unwrap());
        let ids: Vec<&str> = entries.iter().map(|e| e.title.id().0.as_str()).collect();
        assert_eq!(ids, ["film-1", "series-1-s1-e2", "series-2", "film-0"]);
        window
            .read_with(cx, |shell, _| {
                assert_eq!(shell.navigator.current(), Screen::Playlist);
                assert_eq!(shell.playlist_pages[0].name, "Watchlist");
            })
            .unwrap();

        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(played(&server), [ItemId("film-1".into())]);

        // Each end: the card over the page offers the title after, and plays it.
        let series_next = crate::series_page::next_to_play(&server, &ItemId("series-2".into()))
            .unwrap()
            .expect("the series has a next episode");
        for (at, expected) in [
            (1, ItemId("series-1-s1-e2".into())),
            (2, series_next.item.id),
            (3, ItemId("film-0".into())),
        ] {
            window
                .update(cx, |shell, window, cx| {
                    let next = playlists::after(&entries, at - 1).expect("a title after");
                    shell.start_up_next(next, window, cx);
                    assert!(shell.up_next.episode().is_some(), "the card counts down");
                    shell.play_up_next_now(window, cx);
                })
                .unwrap();
            cx.run_until_parked();
            assert_eq!(played(&server).last(), Some(&expected), "title {at}");
        }
        assert_eq!(
            playlists::after(&entries, 3),
            None,
            "nothing after the last"
        );
    }

    #[gpui_kit::test]
    fn an_item_is_removed_from_a_playlist(cx: &mut TestAppContext) {
        let (server, window) = on_watchlist(cx, |_| {});
        let before = shown(&window, cx);
        let entry = before[1].id.clone();
        window
            .update(cx, |shell, _, cx| shell.remove_entry(entry.clone(), cx))
            .unwrap();
        // Gone at once, before the server answers.
        assert!(!shown(&window, cx).iter().any(|e| e.id == entry));
        cx.run_until_parked();
        assert_eq!(shown(&window, cx).len(), before.len() - 1);
        assert_eq!(
            server.state.lock().unwrap().record.playlist_calls,
            [PlaylistCall::Remove(
                ItemId(WATCHLIST.into()),
                entry.clone()
            )]
        );
        let on_server = server.playlist(&ItemId(WATCHLIST.into())).unwrap();
        assert!(!on_server.iter().any(|e| e.id == entry));
    }

    #[gpui_kit::test]
    fn a_refused_removal_is_put_back_and_explained_to_the_viewer(cx: &mut TestAppContext) {
        let (server, window) = on_watchlist(cx, |state| state.playlist_fails = true);
        let before = shown(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(before[1].id.clone(), cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(shown(&window, cx), before, "back at its place");
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert_eq!(said, Some(playlists::FAILED));
        assert_eq!(server.playlist(&ItemId(WATCHLIST.into())).unwrap(), before);
    }

    /// The page of a film in no playlist, its "Add to playlist" menu opened by a click on its
    /// button, the playlists come.
    fn menu_on_a_film(
        cx: &mut TestAppContext,
        change: impl FnOnce(&mut crate::fake::State),
    ) -> (FakeServer, WindowHandle<Shell>, ItemId) {
        let server = FakeServer::new(12, true);
        change(&mut server.state.lock().unwrap());
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server.clone());
        let window =
            cx.add_window(|_, cx| Shell::new(None, Box::new(move || Ok(connector)), None, cx));
        cx.run_until_parked();
        let film = ItemId("film-6".into());
        window
            .update(cx, |shell, _, cx| shell.open_film(film.clone(), cx))
            .unwrap();
        click(&window, cx, "film-playlist");
        window
            .read_with(cx, |shell, _| {
                let menu = shell.playlist_menu.as_ref().expect("the menu opened");
                assert!(menu.open && menu.anchor.is_some(), "under its button");
                assert_eq!(menu.picker.item, film);
            })
            .unwrap();
        (server, window, film)
    }

    fn click(window: &WindowHandle<Shell>, cx: &mut TestAppContext, what: &'static str) {
        let mut visual = gpui_kit::VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        let at = visual
            .debug_bounds(what)
            .unwrap_or_else(|| panic!("no {what}"))
            .center();
        visual.simulate_click(at, gpui_kit::Modifiers::none());
        visual.run_until_parked();
    }

    fn lines(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<(String, bool)> {
        window
            .read_with(cx, |shell, _| {
                let menu = shell.playlist_menu.as_ref().unwrap();
                let lines = menu.picker.lines().into_iter();
                lines.map(|line| (line.name, line.checked)).collect()
            })
            .unwrap()
    }

    /// "New playlist…", `name` typed in its field, and Enter.
    fn new_playlist(window: &WindowHandle<Shell>, cx: &mut TestAppContext, name: &str) {
        click(window, cx, "playlist-new");
        window
            .update(cx, |shell, _, cx| {
                let menu = shell.playlist_menu.as_ref().unwrap();
                let input = menu.naming.clone().expect("the name field shows");
                input.update(cx, |input, cx| input.set_text(name, cx));
            })
            .unwrap();
        let mut visual = gpui_kit::VisualTestContext::from_window((*window).into(), cx);
        visual.simulate_keystrokes("enter");
    }

    fn held_by(server: &FakeServer, name: &str, item: &ItemId) -> bool {
        let playlists = server.playlists().unwrap();
        let found = playlists.iter().find(|p| p.name == name);
        found.is_some_and(|p| playlists::holds(&p.entries, item))
    }

    #[gpui_kit::test]
    fn an_item_is_added_to_an_existing_playlist_and_to_a_new_one(cx: &mut TestAppContext) {
        let (server, window, film) = menu_on_a_film(cx, |_| {});
        assert_eq!(
            lines(&window, cx),
            [("Late night".into(), false), ("Watchlist".into(), false)]
        );

        click(&window, cx, "playlist-line Watchlist");
        assert_eq!(lines(&window, cx)[1], ("Watchlist".into(), true));
        assert!(held_by(&server, "Watchlist", &film));

        new_playlist(&window, cx, "Road trip");
        assert_eq!(lines(&window, cx)[2], ("Road trip".into(), true));
        cx.run_until_parked();
        assert!(held_by(&server, "Road trip", &film));
        assert_eq!(
            server.state.lock().unwrap().record.playlist_calls,
            [
                PlaylistCall::Add(ItemId(WATCHLIST.into()), film.clone()),
                PlaylistCall::Create("Road trip".into(), film),
            ]
        );
        // The menu stays open for another; Escape closes it, and the page stays.
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("escape");
        window
            .read_with(cx, |shell, _| {
                assert!(!shell.playlist_menu_is_open());
                assert_eq!(shell.navigator.current(), Screen::FilmDetail);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_refused_change_is_put_back_and_explained_to_the_viewer(cx: &mut TestAppContext) {
        let (server, window, film) = menu_on_a_film(cx, |state| state.playlist_fails = true);

        click(&window, cx, "playlist-line Late night");
        assert_eq!(
            lines(&window, cx)[0],
            ("Late night".into(), false),
            "put back"
        );
        new_playlist(&window, cx, "Road trip");
        cx.run_until_parked();
        assert_eq!(lines(&window, cx).len(), 2, "the refused playlist is gone");
        window
            .read_with(cx, |shell, _| {
                let menu = shell.playlist_menu.as_ref().unwrap();
                assert_eq!(
                    menu.picker.error,
                    Some(playlists::FAILED),
                    "said in the menu"
                );
                assert_eq!(shell.notice, Some(playlists::FAILED), "and over the page");
            })
            .unwrap();
        assert!(!held_by(&server, "Late night", &film));
        assert!(!held_by(&server, "Road trip", &film));
    }

    #[gpui_kit::test]
    fn a_click_outside_the_menu_closes_it(cx: &mut TestAppContext) {
        let (_server, window, _) = menu_on_a_film(cx, |_| {});
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_click(
            gpui_kit::point(gpui_kit::px(5.), gpui_kit::px(400.)),
            gpui_kit::Modifiers::none(),
        );
        visual.run_until_parked();
        let open = window.read_with(cx, |shell, _| shell.playlist_menu_is_open());
        assert!(!open.unwrap());
    }

    #[gpui_kit::test]
    fn the_series_page_opens_add_to_playlist_for_its_series(cx: &mut TestAppContext) {
        let (_server, window, _) = menu_on_a_film(cx, |_| {});
        let series = ItemId(crate::fake::SERIES.into());
        window
            .update(cx, |shell, _, cx| {
                shell.close_playlist_menu(cx);
                shell.open_series(series.clone(), cx);
            })
            .unwrap();
        click(&window, cx, "series-playlist");
        let item = window.read_with(cx, |shell, _| {
            let menu = shell.playlist_menu.as_ref().expect("the menu opened");
            menu.picker.item.clone()
        });
        assert_eq!(item.unwrap(), series);
    }
}
