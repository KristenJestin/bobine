//! A playlist's page: its titles in their order, each played or opened, "Play all", and an
//! entry taken out. Every call to the server runs off the window's thread, like the others.

use std::time::Instant;

use bobine::provider::{EntryId, ItemId, Playlist, PlaylistEntry, ProviderError, Title};
use gpui_kit::{
    AppContext, Bounds, Context, Entity, Focusable, Pixels, Point, Task, UniformListScrollHandle,
    Window,
};

use super::pages::{Loadable, Origin};
use super::{Film as Playing, Shell};
use crate::home_page::{self, PlayTarget, StartError};
use crate::input::TextInput;
use crate::motion::{self, Fade};
use crate::navigation::Screen;
use crate::playlists::{self, Picker};
use crate::series_page::{Next, Refreshes};

pub struct PlaylistPage {
    pub id: ItemId,
    pub name: String,
    pub entries: Loadable<Vec<PlaylistEntry>>,
    pub scroll: UniformListScrollHandle,
    /// A title is being opened for the player: nothing else starts.
    pub busy: bool,
    /// Each read of the playlist is numbered: one asked before a later read lands late, and is
    /// dropped.
    asked: Refreshes,
    /// The entries taken out whose answer has not come: a read that lands meanwhile, asked
    /// before the server took them out, leaves them out all the same.
    removing: Vec<EntryId>,
    /// The number of the newest read asked.
    read: u64,
    /// A removal was refused, or the newest read was dropped for a removal asked after it:
    /// the playlist is read again once no removal waits for its answer.
    owed: bool,
}

impl PlaylistPage {
    fn new(id: ItemId, name: String) -> Self {
        Self {
            id,
            name,
            entries: Loadable::Loading,
            scroll: UniformListScrollHandle::new(),
            busy: false,
            asked: Refreshes::default(),
            removing: Vec::new(),
            read: 0,
            owed: false,
        }
    }

    /// The read numbered `number` came: it shows, unless a later read was asked since; what
    /// is being taken out stays out. A failed read leaves what was drawn.
    fn landed(&mut self, number: u64, read: Result<Vec<PlaylistEntry>, &'static str>) -> bool {
        if !self.asked.is_newest(number) {
            // Only a removal came after it: no read is on its way to show what it missed.
            if number == self.read {
                self.owed = true;
            }
            return false;
        }
        self.entries = match read {
            Ok(mut entries) => {
                entries.retain(|entry| !self.removing.contains(&entry.id));
                Loadable::Ready(entries)
            }
            Err(_) if matches!(self.entries, Loadable::Ready(_)) => return false,
            Err(why) => Loadable::Failed(why),
        };
        true
    }

    /// Whether the playlist must be read again now: a read is owed and every removal answered.
    fn owes_read(&self) -> bool {
        self.owed && self.removing.is_empty()
    }
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
    /// Which open of the menu this is: the reads of an earlier one never land in it.
    pub generation: u64,
    /// The playlists listed, while what each holds is read in turn.
    reading: Vec<Playlist>,
    /// The reads for the checks, one after another; dropped, they stop.
    fetch: Option<Task<()>>,
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
        series: bool,
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
            picker: Picker::new(item.clone(), series),
            anchor,
            screen: self.navigator.current(),
            open: true,
            fade,
            naming: None,
            generation: {
                self.playlist_menus += 1;
                self.playlist_menus
            },
            reading: Vec::new(),
            fetch: None,
        });
        self.read_menu_playlists(cx);
        cx.notify();
    }

    /// The checks need what each playlist holds, whole: the playlists are listed, then read
    /// one after another, never two at once.
    fn read_menu_playlists(&mut self, cx: &mut Context<Self>) {
        let (Some(server), Some(menu)) = (self.server.clone(), &mut self.playlist_menu) else {
            return;
        };
        let generation = menu.generation;
        menu.fetch = Some(cx.spawn(async move |this, cx| {
            let listing = server.clone();
            let listed = cx
                .background_executor()
                .spawn(async move { listing.playlists(Some(0)) })
                .await;
            let mut next = this
                .update(cx, |shell, cx| {
                    cx.notify();
                    shell.menu_listed(generation, listed, cx)
                })
                .ok()
                .flatten();
            while let Some((at, id)) = next {
                let reading = server.clone();
                let read = cx
                    .background_executor()
                    .spawn(async move { reading.playlist(&id) })
                    .await;
                next = this
                    .update(cx, |shell, cx| {
                        cx.notify();
                        shell.menu_read(generation, at, read, cx)
                    })
                    .ok()
                    .flatten();
            }
        }));
    }

    /// The menu of the open `generation`, while it shows.
    fn menu_of(&mut self, generation: u64) -> Option<&mut PlaylistMenu> {
        self.playlist_menu
            .as_mut()
            .filter(|menu| menu.open && menu.generation == generation)
    }

    /// The playlists of the menu opened `generation` are listed: the first to read, if any.
    pub(super) fn menu_listed(
        &mut self,
        generation: u64,
        found: Result<Vec<Playlist>, ProviderError>,
        cx: &mut Context<Self>,
    ) -> Option<(usize, ItemId)> {
        self.menu_of(generation)?;
        let found = match found {
            Ok(found) => found,
            Err(error) => {
                let said = self.failed(error, cx);
                self.menu_of(generation)?.picker.playlists = Some(Loadable::Failed(said));
                return None;
            }
        };
        let menu = self.menu_of(generation)?;
        menu.reading = found;
        Self::menu_next(menu, 0)
    }

    /// What the playlist `at` of the menu opened `generation` holds came: the next to read, if
    /// any. One the server will not read is left out.
    pub(super) fn menu_read(
        &mut self,
        generation: u64,
        at: usize,
        read: Result<Vec<PlaylistEntry>, ProviderError>,
        cx: &mut Context<Self>,
    ) -> Option<(usize, ItemId)> {
        let menu = self.menu_of(generation)?;
        match read {
            Ok(entries) => {
                menu.reading.get_mut(at)?.entries = entries;
                Self::menu_next(menu, at + 1)
            }
            Err(ProviderError::Forbidden | ProviderError::NotFound) => {
                if at < menu.reading.len() {
                    menu.reading.remove(at);
                }
                Self::menu_next(menu, at)
            }
            Err(error) => {
                let said = self.failed(error, cx);
                self.menu_of(generation)?.picker.playlists = Some(Loadable::Failed(said));
                None
            }
        }
    }

    /// The playlist `at` to read next; past the last, the menu shows them all.
    fn menu_next(menu: &mut PlaylistMenu, at: usize) -> Option<(usize, ItemId)> {
        match menu.reading.get(at) {
            Some(playlist) => Some((at, playlist.id.clone())),
            None => {
                let found = std::mem::take(&mut menu.reading);
                menu.picker.playlists = Some(Loadable::Ready(found));
                None
            }
        }
    }

    /// The menu fades out; the keys go back to the window.
    pub fn close_playlist_menu(&mut self, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.playlist_menu
            && menu.open
        {
            menu.open = false;
            menu.naming = None;
            menu.fetch = None;
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
                shell
                    .playlist_menu
                    .as_mut()
                    .filter(|menu| menu.picker.item == item)
                    .and_then(|menu| menu.picker.picked(&playlist, &answer));
                if let Err(error) = &answer {
                    shell.show_notice(playlists::refusal(error), cx);
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
                shell
                    .playlist_menu
                    .as_mut()
                    .filter(|menu| menu.picker.item == item)
                    .and_then(|menu| menu.picker.made(token, &answer));
                if let Err(error) = &answer {
                    shell.show_notice(playlists::refusal(error), cx);
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
        self.playlist_pages
            .push(PlaylistPage::new(id, name.unwrap_or_default()));
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
        let page = &mut self.playlist_pages[depth];
        let id = page.id.clone();
        let number = page.asked.ask(&mut self.refreshes_asked);
        page.read = number;
        page.owed = false;
        let asked = id.clone();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            move |server| server.playlist(&asked),
            move |shell, entries, cx| {
                let read = match entries {
                    Ok(entries) => {
                        shell
                            .marks
                            .listed(stamp, entries.iter().map(|entry| &entry.title));
                        Ok(entries)
                    }
                    Err(error) => Err(shell.failed(error, cx)),
                };
                if let Some(page) = shell.playlist_page(depth, &id) {
                    page.landed(number, read);
                    if page.owes_read() {
                        shell.load_playlist_page(depth, cx);
                    }
                }
                cx.notify();
            },
        );
    }

    /// Back on the playlist's page on top: its titles' state may have changed.
    pub(super) fn reload_playlist(&mut self, cx: &mut Context<Self>) {
        if let Some(depth) = self.playlist_pages.len().checked_sub(1) {
            self.load_playlist_page(depth, cx);
        }
    }

    /// "Play all": the first title, then each after it through the card; a title that cannot
    /// play is passed over.
    pub fn play_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let first = match self.playlist_pages.last().map(|page| &page.entries) {
            Some(Loadable::Ready(entries)) => entries.first().cloned(),
            _ => None,
        };
        if let Some(first) = first {
            self.play_entry(first, true, window, cx);
        }
    }

    /// Plays `entry` of the playlist on top. The entry after it, as the page shows them now, is
    /// kept for the card at its end. `go_on`: playing the playlist through, so an entry with
    /// nothing to play gives way to the one after it.
    pub fn play_entry(
        &mut self,
        entry: PlaylistEntry,
        go_on: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let depth = self.playlist_pages.len().wrapping_sub(1);
        let Some(page) = self.playlist_pages.last_mut() else {
            return;
        };
        if page.busy {
            return;
        }
        let Some(server) = self.server.clone() else {
            return;
        };
        let target = PlayTarget::of(&entry.title);
        let next = match &page.entries {
            Loadable::Ready(entries) => playlists::after(entries, &entry.id),
            _ => None,
        };
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
                            desktop: Some(started.desktop),
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
                    Err(StartError::NothingToPlay) => match next {
                        Some(Next::Entry(next)) if go_on => {
                            shell.tried(&origin, None, cx);
                            shell.play_entry(next, true, window, cx);
                        }
                        _ => shell.tried(&origin, Some("Nothing to play for this title."), cx),
                    },
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

    /// Takes `entry` out of the playlist on top: it leaves the page at once. If the server
    /// refuses, the page says why and reads the playlist again, as the server holds it, once
    /// every removal asked was answered: other changes may have moved the entries since, so it
    /// is not put back at its old place.
    pub fn remove_entry(&mut self, entry: EntryId, cx: &mut Context<Self>) {
        let depth = self.playlist_pages.len().wrapping_sub(1);
        let Some(page) = self.playlist_pages.last_mut() else {
            return;
        };
        let Loadable::Ready(entries) = &mut page.entries else {
            return;
        };
        if !playlists::take_out(entries, &entry) {
            return;
        }
        page.removing.push(entry.clone());
        // A read asked before is of the playlist as it was: it no longer shows.
        page.asked.ask(&mut self.refreshes_asked);
        let id = page.id.clone();
        let playlist = id.clone();
        let removed = entry.clone();
        cx.notify();
        self.ask(
            cx,
            move |server| server.remove_from_playlist(&playlist, &removed),
            move |shell, answer, cx| {
                let Some(page) = shell.playlist_page(depth, &id) else {
                    return;
                };
                page.removing.retain(|removing| removing != &entry);
                if answer.is_err() {
                    page.owed = true;
                }
                if page.owes_read() {
                    shell.load_playlist_page(depth, cx);
                }
                if let Err(error) = answer {
                    shell.failed(error.clone(), cx);
                    shell.show_notice(playlists::refusal(&error), cx);
                }
                cx.notify();
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeServer, PlaylistCall};
    use bobine::provider::MediaServer;
    use gpui_kit::{TestAppContext, WindowHandle};
    use std::sync::Arc;

    const WATCHLIST: &str = "playlist-watchlist";

    /// The page of the Watchlist, opened from the Lists, of a server first changed by `change`.
    fn on_watchlist(
        cx: &mut TestAppContext,
        change: impl FnOnce(&mut crate::fake::State),
    ) -> (FakeServer, WindowHandle<Shell>) {
        let server = FakeServer::new(12, true);
        change(&mut server.state.lock().unwrap());
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
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

    /// The next playback starts on a fake player: what it is told comes through the returned
    /// queue.
    fn arm_player(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<crate::player::PlayerEvent>>> {
        let player = crate::player::fake::FakePlayer::default();
        let queued = player.queued.clone();
        window
            .update(cx, |shell, _, _| shell.fake_player = Some(player))
            .unwrap();
        queued
    }

    /// The title playing reaches its real end: the player goes, back to the page.
    fn end_of_title(
        queued: &std::rc::Rc<std::cell::RefCell<Vec<crate::player::PlayerEvent>>>,
        cx: &mut TestAppContext,
    ) {
        use crate::player::PlayerEvent;
        queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::Duration(2820.),
            PlayerEvent::Position(2820.),
            PlayerEvent::EndOfFile,
        ]);
        cx.executor().advance_clock(super::super::PLAYER_TICK * 2);
        cx.run_until_parked();
    }

    /// What the card over the page offers.
    fn offered(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<Next> {
        window
            .read_with(cx, |shell, _| shell.up_next.episode().cloned())
            .unwrap()
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .read_with(cx, |shell, _| shell.navigator.current())
            .unwrap()
    }

    #[gpui_kit::test]
    fn a_playlist_opens_with_its_items_in_order_and_play_all_plays_them_in_sequence(
        cx: &mut TestAppContext,
    ) {
        let (server, window) = on_watchlist(cx, |_| {});
        let entries = shown(&window, cx);
        assert_eq!(entries, server.playlist(&ItemId(WATCHLIST.into())).unwrap());
        let ids: Vec<&str> = entries.iter().map(|e| e.title.id().0.as_str()).collect();
        assert_eq!(
            ids,
            ["film-1", "series-1-s1-e2", "series-2-s1-e1", "film-0"]
        );
        window
            .read_with(cx, |shell, _| {
                assert_eq!(shell.navigator.current(), Screen::Playlist);
                assert_eq!(shell.playlist_pages[0].name, "Watchlist");
            })
            .unwrap();

        let mut queued = arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(played(&server), [ItemId("film-1".into())]);
        assert_eq!(screen(&window, cx), Screen::Player);

        // Each end: the card over the page offers the title after, and plays it.
        for (at, entry) in entries.iter().enumerate().skip(1) {
            end_of_title(&queued, cx);
            assert_eq!(screen(&window, cx), Screen::Playlist);
            assert_eq!(
                offered(&window, cx),
                Some(Next::Entry(entry.clone())),
                "the card offers title {at}"
            );
            queued = arm_player(&window, cx);
            window
                .update(cx, |shell, window, cx| shell.play_up_next_now(window, cx))
                .unwrap();
            cx.run_until_parked();
            assert_eq!(played(&server).last(), Some(entry.title.id()));
        }
        end_of_title(&queued, cx);
        assert_eq!(offered(&window, cx), None, "nothing after the last");
    }

    #[gpui_kit::test]
    fn next_from_mpris_plays_the_playlist_entry_after(cx: &mut TestAppContext) {
        use crate::mpris::fake::FakeControls;
        use crate::mpris::{Command, Publisher, Status};
        use crate::player::PlayerEvent;

        let (server, window) = on_watchlist(cx, |_| {});
        let entries = shown(&window, cx);
        let controls = FakeControls::default();
        let fake = controls.clone();
        window
            .update(cx, |shell, _, _| {
                shell.mpris = Publisher::new(Box::new(fake))
            })
            .unwrap();
        let queued = arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
        queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::Duration(2820.),
            PlayerEvent::Position(100.),
        ]);
        cx.executor().advance_clock(super::super::PLAYER_TICK * 2);
        cx.run_until_parked();
        let shown = controls.last();
        assert_eq!(shown.status, Status::Playing);
        assert_eq!(
            shown.track.map(|track| track.item),
            Some(entries[0].title.id().0.clone())
        );
        assert!(shown.can_go_next, "a title follows in the playlist");

        arm_player(&window, cx);
        controls.queued.borrow_mut().push(Command::Next);
        cx.executor().advance_clock(super::super::PLAYER_TICK * 2);
        cx.run_until_parked();
        assert_eq!(played(&server).last(), Some(entries[1].title.id()));
        assert_eq!(screen(&window, cx), Screen::Player);
    }

    #[gpui_kit::test]
    fn the_card_plays_the_entry_it_shows_when_one_is_removed_during_the_countdown(
        cx: &mut TestAppContext,
    ) {
        let (server, window) = on_watchlist(cx, |_| {});
        let entries = shown(&window, cx);
        let queued = arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
        end_of_title(&queued, cx);
        assert_eq!(offered(&window, cx), Some(Next::Entry(entries[1].clone())));

        // The title that just played is taken out while the card counts down: the others move
        // up by one.
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(entries[0].id.clone(), cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(shown(&window, cx)[0], entries[1]);

        arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_up_next_now(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(played(&server).last(), Some(entries[1].title.id()));
    }

    #[gpui_kit::test]
    #[should_panic(expected = "a test started a playback without setting Shell::fake_player")]
    fn a_test_playing_without_the_fake_player_panics_before_the_real_one(cx: &mut TestAppContext) {
        let (_server, window) = on_watchlist(cx, |_| {});
        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn play_all_skips_a_title_that_cannot_play_and_goes_on_to_the_next(cx: &mut TestAppContext) {
        // The episode second in the playlist has no file left.
        let (server, window) = on_watchlist(cx, |state| {
            let episode = state.episodes.iter_mut();
            let mut episode = episode.filter(|e| e.item.id.0 == "series-1-s1-e2");
            episode.next().unwrap().versions.clear();
        });
        let entries = shown(&window, cx);
        let queued = arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
        end_of_title(&queued, cx);
        arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_up_next_now(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            played(&server),
            [ItemId("film-1".into()), entries[2].title.id().clone()],
            "the episode with nothing to play is passed over"
        );
        assert_eq!(screen(&window, cx), Screen::Player);
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert_eq!(said, None);
    }

    #[gpui_kit::test]
    fn a_title_played_alone_that_cannot_play_says_so(cx: &mut TestAppContext) {
        let (server, window) = on_watchlist(cx, |state| {
            let episode = state.episodes.iter_mut();
            let mut episode = episode.filter(|e| e.item.id.0 == "series-1-s1-e2");
            episode.next().unwrap().versions.clear();
        });
        let entries = shown(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                shell.play_entry(entries[1].clone(), false, window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert!(played(&server).is_empty());
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert_eq!(said, Some("Nothing to play for this title."));
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
    fn a_refused_removal_reads_the_playlist_again_and_says_why(cx: &mut TestAppContext) {
        let (server, window) = on_watchlist(cx, |_| {});
        let before = shown(&window, cx);
        server.state.lock().unwrap().refused_entries = vec![before[1].id.clone()];
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(before[1].id.clone(), cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(shown(&window, cx), before, "as the server holds it");
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert_eq!(said, Some("You can't change this playlist."));
        assert_eq!(server.playlist(&ItemId(WATCHLIST.into())).unwrap(), before);
    }

    #[gpui_kit::test]
    fn two_quick_removals_with_one_refused_show_what_the_server_holds(cx: &mut TestAppContext) {
        let (server, window) = on_watchlist(cx, |_| {});
        let before = shown(&window, cx);
        server.state.lock().unwrap().refused_entries = vec![before[1].id.clone()];
        // The second, then the first, before either answer comes: the refused one is no
        // longer at the place it was taken from.
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(before[1].id.clone(), cx);
                shell.remove_entry(before[0].id.clone(), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let on_server = server.playlist(&ItemId(WATCHLIST.into())).unwrap();
        assert_eq!(on_server, before[1..]);
        assert_eq!(shown(&window, cx), on_server);
    }

    #[gpui_kit::test]
    fn a_read_asked_before_a_removal_does_not_bring_it_back(cx: &mut TestAppContext) {
        let (server, window) = on_watchlist(cx, |_| {});
        let before = shown(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                // A read of the playlist as it is, then the removal, then another read.
                shell.reload_playlist(cx);
                shell.remove_entry(before[0].id.clone(), cx);
                shell.reload_playlist(cx);
            })
            .unwrap();
        cx.run_until_parked();
        let on_server = server.playlist(&ItemId(WATCHLIST.into())).unwrap();
        assert_eq!(on_server, before[1..]);
        assert_eq!(shown(&window, cx), on_server);
    }

    /// Runs the window's tasks one at a time until `reached` holds of the playlist's page.
    fn run_until(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
        reached: impl Fn(&PlaylistPage) -> bool,
    ) {
        let held = |cx: &mut TestAppContext| {
            window
                .read_with(cx, |shell, _| reached(shell.playlist_pages.last().unwrap()))
                .unwrap()
        };
        while !held(cx) {
            assert!(cx.executor().tick(), "never reached");
        }
    }

    #[gpui_kit::test]
    fn a_refusal_then_a_second_removal_before_the_reload_still_shows_what_the_server_holds(
        cx: &mut TestAppContext,
    ) {
        let (server, window) = on_watchlist(cx, |_| {});
        let before = shown(&window, cx);
        server.state.lock().unwrap().refused_entries = vec![before[1].id.clone()];
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(before[1].id.clone(), cx)
            })
            .unwrap();
        // The refusal came and asked the playlist again; that read has not landed yet.
        run_until(&window, cx, |page| page.removing.is_empty());
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(before[0].id.clone(), cx)
            })
            .unwrap();
        cx.run_until_parked();
        let on_server = server.playlist(&ItemId(WATCHLIST.into())).unwrap();
        assert_eq!(on_server, before[1..]);
        assert_eq!(shown(&window, cx), on_server, "the refused entry is back");
    }

    #[gpui_kit::test]
    fn a_reload_after_playback_then_a_removal_before_it_lands_still_shows_what_the_server_holds(
        cx: &mut TestAppContext,
    ) {
        let (server, window) = on_watchlist(cx, |_| {});
        let before = shown(&window, cx);
        let asked = |window: &WindowHandle<Shell>, cx: &mut TestAppContext| {
            window
                .read_with(cx, |shell, _| shell.playlist_pages[0].asked.newest())
                .unwrap()
        };
        let read_before = asked(&window, cx);
        // The title watched to its end, as the server holds it once "stopped" reached it.
        let watched = before[0].title.id().clone();
        for film in server.state.lock().unwrap().films.iter_mut() {
            if film.id == watched {
                film.watched = true;
            }
        }
        window
            .update(cx, |shell, _, cx| {
                shell.after_playback(std::thread::spawn(|| {}), cx)
            })
            .unwrap();
        // The reload after the playback is asked; it has not landed yet.
        run_until(&window, cx, |page| page.asked.newest() != read_before);
        window
            .update(cx, |shell, _, cx| {
                shell.remove_entry(before[1].id.clone(), cx)
            })
            .unwrap();
        cx.run_until_parked();
        let on_server = server.playlist(&ItemId(WATCHLIST.into())).unwrap();
        assert!(matches!(&on_server[0].title, Title::Film(film) if film.watched));
        assert_eq!(shown(&window, cx), on_server, "the watched mark shows");
    }

    fn page() -> PlaylistPage {
        PlaylistPage::new(ItemId(WATCHLIST.into()), "Watchlist".into())
    }

    #[test]
    fn a_read_older_than_the_last_is_dropped() {
        let entries = FakeServer::new(12, false)
            .playlist(&ItemId(WATCHLIST.into()))
            .unwrap();
        let mut counter = 0;
        let mut page = page();
        let older = page.asked.ask(&mut counter);
        let newer = page.asked.ask(&mut counter);
        assert!(page.landed(newer, Ok(entries[1..].to_vec())));
        assert!(
            !page.landed(older, Ok(entries.clone())),
            "late, and dropped"
        );
        assert!(matches!(&page.entries, Loadable::Ready(shown) if shown[..] == entries[1..]));
    }

    #[test]
    fn a_read_landing_while_an_entry_is_taken_out_leaves_it_out() {
        let entries = FakeServer::new(12, false)
            .playlist(&ItemId(WATCHLIST.into()))
            .unwrap();
        let mut counter = 0;
        let mut page = page();
        page.removing.push(entries[0].id.clone());
        let read = page.asked.ask(&mut counter);
        assert!(page.landed(read, Ok(entries.clone())));
        assert!(matches!(&page.entries, Loadable::Ready(shown) if shown[..] == entries[1..]));
        // A failed read leaves what was drawn.
        let read = page.asked.ask(&mut counter);
        assert!(!page.landed(read, Err("no")));
        assert!(matches!(&page.entries, Loadable::Ready(shown) if shown[..] == entries[1..]));
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
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
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
        let playlists = server.playlists(None).unwrap();
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
        let (server, window, film) = menu_on_a_film(cx, |state| {
            state.playlist_refusal = Some(ProviderError::Forbidden)
        });

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
                    Some("You can't change this playlist."),
                    "said in the menu"
                );
                assert_eq!(
                    shell.notice,
                    Some("You can't change this playlist."),
                    "and over the page"
                );
            })
            .unwrap();
        assert!(!held_by(&server, "Late night", &film));
        assert!(!held_by(&server, "Road trip", &film));
    }

    #[gpui_kit::test]
    fn a_change_refused_for_another_reason_says_the_generic_words(cx: &mut TestAppContext) {
        let (_server, window, _) = menu_on_a_film(cx, |state| {
            state.playlist_refusal = Some(ProviderError::Server(500))
        });
        click(&window, cx, "playlist-line Late night");
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert_eq!(said, Some(playlists::FAILED));
    }

    #[gpui_kit::test]
    fn a_series_added_from_its_page_goes_in_as_its_episodes_and_says_how_many(
        cx: &mut TestAppContext,
    ) {
        let (server, window, _) = menu_on_a_film(cx, |_| {});
        let series = ItemId(crate::fake::SERIES.into());
        window
            .update(cx, |shell, _, cx| {
                shell.close_playlist_menu(cx);
                shell.open_series(series.clone(), cx);
            })
            .unwrap();
        click(&window, cx, "series-playlist");
        click(&window, cx, "playlist-line Late night");
        let episodes: Vec<ItemId> = server
            .state
            .lock()
            .unwrap()
            .episodes
            .iter()
            .filter(|e| e.series == series)
            .map(|e| e.item.id.clone())
            .collect();
        let late = server.playlists(None).unwrap().remove(0);
        let held: Vec<&ItemId> = late.entries.iter().map(|e| e.title.id()).collect();
        assert_eq!(held[5..], episodes.iter().collect::<Vec<_>>()[..]);
        assert!(
            !playlists::holds(&late.entries, &series),
            "never the series"
        );
        let told = window.read_with(cx, |shell, _| {
            shell.playlist_menu.as_ref().unwrap().picker.told.clone()
        });
        assert_eq!(
            told.unwrap(),
            Some(format!("Added {} episodes.", episodes.len()))
        );
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        assert!(
            visual.debug_bounds("playlist-told").is_some(),
            "in the menu"
        );
    }

    #[gpui_kit::test]
    fn a_menu_opened_closed_and_reopened_ignores_the_first_reads_and_runs_one_chain(
        cx: &mut TestAppContext,
    ) {
        let (server, window, film) = menu_on_a_film(cx, |_| {});
        let all = server.playlists(None).unwrap().len();
        let (first, second) = window
            .update(cx, |shell, _, cx| {
                shell.close_playlist_menu(cx);
                server.state.lock().unwrap().record = Default::default();
                shell.open_playlist_menu(film.clone(), false, None, cx);
                let first = shell.playlist_menu.as_ref().unwrap().generation;
                shell.close_playlist_menu(cx);
                shell.open_playlist_menu(film.clone(), false, None, cx);
                (first, shell.playlist_menu.as_ref().unwrap().generation)
            })
            .unwrap();
        assert_ne!(first, second);
        cx.run_until_parked();
        {
            let state = server.state.lock().unwrap();
            assert_eq!(
                state.record.playlists_listed, 1,
                "the first chain never ran"
            );
            assert_eq!(state.record.playlist_reads.len(), all, "one read each");
        }
        let lines_now = lines(&window, cx);
        assert_eq!(lines_now.len(), all);
        // An answer of the first open landing late changes nothing.
        window
            .update(cx, |shell, _, cx| {
                shell.menu_listed(first, Ok(Vec::new()), cx);
                shell.menu_read(first, 0, Err(ProviderError::NotFound), cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(lines(&window, cx), lines_now);
        assert_eq!(
            server.state.lock().unwrap().record.playlist_reads.len(),
            all
        );
    }

    #[gpui_kit::test]
    fn a_playlist_the_server_will_not_read_is_left_out_of_the_menu(cx: &mut TestAppContext) {
        let (_server, window, _) = menu_on_a_film(cx, |state| {
            state.unreadable_playlists = vec![ItemId("playlist-late".into())]
        });
        assert_eq!(lines(&window, cx), [("Watchlist".into(), false)]);
    }

    #[gpui_kit::test]
    fn new_playlist_from_a_series_says_how_many_episodes_went_in(cx: &mut TestAppContext) {
        let (server, window, _) = menu_on_a_film(cx, |_| {});
        let series = ItemId(crate::fake::SERIES.into());
        window
            .update(cx, |shell, _, cx| {
                shell.close_playlist_menu(cx);
                shell.open_series(series.clone(), cx);
            })
            .unwrap();
        click(&window, cx, "series-playlist");
        new_playlist(&window, cx, "Road trip");
        cx.run_until_parked();
        let state = server.state.lock().unwrap();
        let episodes = state.episodes.iter().filter(|e| e.series == series).count();
        drop(state);
        let told = window.read_with(cx, |shell, _| {
            shell.playlist_menu.as_ref().unwrap().picker.told.clone()
        });
        assert_eq!(told.unwrap(), Some(format!("Added {episodes} episodes.")));
    }

    /// A playlist's title playing on a fake player, the menu of its "Add to playlist" button
    /// opened by a click.
    fn menu_in_the_player(cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let (_server, window) = on_watchlist(cx, |_| {});
        arm_player(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.play_all(window, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Player);
        // The pointer moved: the controls show, at once.
        window
            .update(cx, |shell, _, cx| {
                let playback = shell.playback.as_mut().unwrap();
                playback.session.mouse_moved(Instant::now());
                playback.controls_fade = Fade::starting_at(motion::quick(), true);
                cx.notify();
            })
            .unwrap();
        click(&window, cx, "player-playlist");
        window
    }

    #[gpui_kit::test]
    fn the_players_add_to_playlist_button_opens_the_menu_for_what_plays(cx: &mut TestAppContext) {
        let window = menu_in_the_player(cx);
        window
            .read_with(cx, |shell, _| {
                let menu = shell.playlist_menu.as_ref().expect("the menu opened");
                assert!(menu.open && menu.anchor.is_some(), "under its button");
                assert_eq!(menu.picker.item, ItemId("film-1".into()));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn escape_closes_the_menu_in_the_player_and_the_player_stays(cx: &mut TestAppContext) {
        let window = menu_in_the_player(cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        window
            .read_with(cx, |shell, _| {
                assert!(!shell.playlist_menu_is_open());
                assert_eq!(shell.navigator.current(), Screen::Player);
                assert!(shell.playback.is_some());
            })
            .unwrap();
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
