//! The request server (#63): how its client is made from the Settings (the address in the
//! settings file, the key in the keyring), and what the window keeps of it. Nothing of it shows
//! while it is not configured but the Integrations section of the Settings.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bobine::provider::{TmdbIndex, Token, TokenStore};
use bobine::requests::{
    DiscoverRow, Discovered, MediaKind, MyRequest, RequestError, RequestServer, ServerCheck, TmdbId,
};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, ScrollHandle, ScrollWheelEvent, Subscription, Task, Window};

use super::pages::route_wheel;
use super::{Loadable, Shell};
use crate::discover;
use crate::input::TextInput;
use crate::navigation::Screen;
use crate::scroll;
use crate::secret_field::{self, SecretField};

/// How long the Discover search waits for the viewer to stop typing.
const SEARCH_AFTER: Duration = Duration::from_millis(300);

/// Where the keyring keeps the request server's API key, for the signed-in user.
pub const SEERR_KEY: &str = "seerr-api-key";

/// Makes a client of the request server at an address, with its key, for the Jellyfin user.
pub type OpenSeerr =
    Arc<dyn Fn(&str, Token, &str) -> Result<Arc<dyn RequestServer>, RequestError> + Send + Sync>;

/// The real client. A test never reaches a real server: it gives a fake, and this refuses.
pub fn real_seerr() -> OpenSeerr {
    #[cfg(not(test))]
    return Arc::new(|address, key, viewer| {
        let client = bobine::seerr::SeerrClient::new(address, key, viewer)?;
        Ok(Arc::new(client) as Arc<dyn RequestServer>)
    });
    #[cfg(test)]
    Arc::new(|_, _, _| Err(RequestError::Unreachable("no real server in a test".into())))
}

/// The request server, as the Settings and the pages see it.
#[derive(Default)]
pub struct Seerr {
    /// Made once the address and the key are both there: Discover shows from then on.
    pub client: Option<Arc<dyn RequestServer>>,
    /// The Settings' field of the address, and the key's.
    pub address: Option<Entity<TextInput>>,
    pub key: Option<Entity<SecretField>>,
    /// What the key field said at the last look, once it looked in the keyring: a change makes
    /// the client again.
    key_saved: Option<bool>,
    /// What the last test of the connection said: the version, or why it failed.
    pub check: Option<Loadable<String>>,
    /// Said under the fields when the requests go as the key's owner, not the viewer.
    pub owner_line: Option<String>,
    /// The number of the client being made: an older making that ends late is dropped.
    making: u64,
    _key_changed: Option<Subscription>,
    /// The library by TMDb id: what of the request server's titles is in it.
    pub index: Option<TmdbIndex>,
    pub discover: DiscoverPage,
    /// The request pages open, the last one shown.
    pub requests: Vec<super::RequestPage>,
    /// What the server recommends after a title of the library, read once per title.
    similar: HashMap<(MediaKind, TmdbId), Vec<Discovered>>,
}

/// The Discover screen: its search, its rows, and the viewer's requests.
#[derive(Default)]
pub struct DiscoverPage {
    /// Trending, popular movies, popular series, upcoming: `DiscoverRow::ALL`'s order.
    pub rows: [Option<Loadable<Vec<Discovered>>>; 4],
    pub mine: Option<Loadable<Vec<MyRequest>>>,
    pub field: Option<Entity<TextInput>>,
    /// What the results are for; empty: the rows show.
    pub query: String,
    pub results: Option<Loadable<Vec<Discovered>>>,
    /// The search last sent: an answer to an older one is dropped.
    ticket: u64,
    waiting: Option<Task<()>>,
    pub scroll: ScrollHandle,
    /// My requests, then the four rows: each scrolls sideways.
    pub row_scrolls: [ScrollHandle; 5],
    gesture: scroll::Gesture,
    _typed: Option<Subscription>,
}

impl Shell {
    pub fn seerr_client(&self) -> Option<Arc<dyn RequestServer>> {
        self.seerr.client.clone()
    }

    /// The fields of the Integrations section, for the signed-in user: the address from the
    /// settings file, the key from the keyring. The client follows them.
    pub(super) fn set_up_seerr(&mut self, cx: &mut Context<Self>) {
        let address = cx.new(|cx| {
            let mut input = TextInput::new("https://seerr.example.org", false, cx);
            if let Some(kept) = &self.settings.seerr_address {
                input.set_text(kept, cx);
            }
            input
        });
        let Some(key) = self.add_secret_field("API key", SEERR_KEY, cx) else {
            return;
        };
        let changed = cx.observe(&key, |shell, key, cx| {
            let key = key.read(cx);
            if !key.is_settled() {
                return;
            }
            let saved = Some(key.is_saved());
            if saved != shell.seerr.key_saved {
                shell.seerr.key_saved = saved;
                shell.connect_seerr(cx);
            }
        });
        self.seerr = Seerr {
            address: Some(address),
            key: Some(key),
            _key_changed: Some(changed),
            ..Seerr::default()
        };
    }

    /// The address typed is kept (an empty one forgets it) and the client made again.
    pub fn save_seerr_address(&mut self, cx: &mut Context<Self>) {
        let Some(input) = self.seerr.address.clone() else {
            return;
        };
        let typed = bobine::seerr::with_scheme(input.read(cx).text());
        if typed != input.read(cx).text() {
            input.update(cx, |field, cx| field.set_text(&typed, cx));
        }
        let address = (!typed.is_empty()).then_some(typed);
        if address == self.settings.seerr_address {
            return;
        }
        self.settings.seerr_address = address;
        self.keep_settings();
        self.connect_seerr(cx);
        cx.notify();
    }

    /// Makes the client from the address and the key, or drops it when one of them is missing:
    /// Discover goes with it, and the Settings say which is missing, or why the client could not
    /// be made. The key is read from the keyring off the window's thread, and goes from there to
    /// the client only.
    fn connect_seerr(&mut self, cx: &mut Context<Self>) {
        self.seerr.making += 1;
        let making = self.seerr.making;
        self.seerr.check = None;
        self.seerr.owner_line = None;
        let key_saved = self.seerr.key_saved == Some(true);
        let address = self.settings.seerr_address.clone();
        let missing = match (&address, key_saved) {
            (Some(_), false) => Some("Enter the API key."),
            (None, true) => Some("Enter the address."),
            _ => None,
        };
        let (Some(address), true, Some(server)) = (address, key_saved, self.server.clone()) else {
            self.seerr_gone(cx);
            self.seerr.check = missing.map(Loadable::Failed);
            return;
        };
        let account = secret_field::account(&server.server().id, server.user_id(), SEERR_KEY);
        let (store, open): (Arc<dyn TokenStore>, _) =
            (self.secrets.clone(), self.open_seerr.clone());
        let viewer = server.user_id().to_string();
        let task = cx.background_executor().spawn(async move {
            let Ok(Some(key)) = store.load(&account) else {
                return Err("Enter the API key.");
            };
            let client = open(&address, key, &viewer).map_err(|e| discover::error_line(&e))?;
            // Whom the requests go as, said in the Settings when it is not the viewer.
            let check = client.check();
            Ok((client, check))
        });
        cx.spawn(async move |this, cx| {
            let made = task.await;
            this.update(cx, |shell, cx| {
                if shell.seerr.making != making {
                    return;
                }
                match made {
                    Ok((client, check)) => {
                        shell.seerr.client = Some(client);
                        shell.seerr_checked(check);
                        shell.seerr_connected(cx);
                    }
                    Err(why) => {
                        shell.seerr_gone(cx);
                        shell.seerr.check = Some(Loadable::Failed(why));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The client is there: the library is indexed by TMDb id, and a Discover showing reads.
    fn seerr_connected(&mut self, cx: &mut Context<Self>) {
        self.load_tmdb_index(cx);
        if self.navigator.current() == Screen::Discover {
            self.load_discover(cx);
        }
    }

    /// No client any more: nothing of it shows, and its screens are left.
    fn seerr_gone(&mut self, cx: &mut Context<Self>) {
        self.seerr.client = None;
        self.seerr.index = None;
        self.seerr.discover = DiscoverPage::default();
        self.seerr.requests.clear();
        self.seerr.similar.clear();
        self.navigator
            .forget(|screen| matches!(screen, Screen::Discover | Screen::Request));
        cx.notify();
    }

    fn load_tmdb_index(&mut self, cx: &mut Context<Self>) {
        self.ask(
            cx,
            |server| server.tmdb_index(),
            |shell, index, _| {
                // Without it, every title is said not in the library: nothing worse.
                if let Ok(index) = index {
                    shell.seerr.index = Some(index);
                }
            },
        );
    }

    /// Runs `work` against the request server off the window's thread, then `then` with its
    /// answer, if the same client is still there.
    pub(super) fn ask_seerr<R: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&dyn RequestServer) -> R + Send + 'static,
        then: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let Some(client) = self.seerr.client.clone() else {
            return;
        };
        let making = self.seerr.making;
        let task = cx
            .background_executor()
            .spawn(async move { work(&*client) });
        cx.spawn(async move |this, cx| {
            let answer = task.await;
            this.update(cx, |shell, cx| {
                if shell.seerr.making == making && shell.seerr.client.is_some() {
                    then(shell, answer, cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Discover shows: the rows once, the viewer's requests each time, the library again.
    pub(super) fn load_discover(&mut self, cx: &mut Context<Self>) {
        if self.seerr.client.is_none() {
            return;
        }
        if self.seerr.discover.field.is_none() {
            let field = cx.new(|cx| TextInput::new("Search movies and series on TMDb", false, cx));
            let typed = cx.observe(&field, |shell, _, cx| shell.discover_typed(cx));
            self.seerr.discover.field = Some(field);
            self.seerr.discover._typed = Some(typed);
        }
        for (at, row) in DiscoverRow::ALL.into_iter().enumerate() {
            if matches!(self.seerr.discover.rows[at], Some(Loadable::Ready(_))) {
                continue;
            }
            self.seerr.discover.rows[at] = Some(Loadable::Loading);
            self.ask_seerr(
                cx,
                move |seerr| seerr.discover(row),
                move |shell, found, _| {
                    shell.seerr.discover.rows[at] = Some(match found {
                        Ok(found) => Loadable::Ready(found),
                        Err(error) => Loadable::Failed(discover::error_line(&error)),
                    });
                },
            );
        }
        if self.seerr.discover.mine.is_none() {
            self.seerr.discover.mine = Some(Loadable::Loading);
        }
        self.ask_seerr(
            cx,
            |seerr| seerr.my_requests(),
            |shell, mine, _| {
                let page = &mut shell.seerr.discover;
                match mine {
                    Ok(mine) => page.mine = Some(Loadable::Ready(mine)),
                    // Requests already shown stay: the line says only a first failure.
                    Err(error) if !matches!(page.mine, Some(Loadable::Ready(_))) => {
                        page.mine = Some(Loadable::Failed(discover::error_line(&error)));
                    }
                    Err(_) => {}
                }
            },
        );
        self.load_tmdb_index(cx);
    }

    /// The search field changed: the search goes out once the viewer stops typing.
    fn discover_typed(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &self.seerr.discover.field else {
            return;
        };
        let query = field.read(cx).text().trim().to_string();
        let page = &mut self.seerr.discover;
        if query == page.query {
            return;
        }
        page.query = query.clone();
        page.ticket += 1;
        let ticket = page.ticket;
        if query.is_empty() {
            page.results = None;
            page.waiting = None;
            cx.notify();
            return;
        }
        page.results = Some(Loadable::Loading);
        let executor = cx.background_executor().clone();
        page.waiting = Some(cx.spawn(async move |this, cx| {
            executor.timer(SEARCH_AFTER).await;
            this.update(cx, |shell, cx| shell.search_discover(ticket, query, cx))
                .ok();
        }));
        cx.notify();
    }

    fn search_discover(&mut self, ticket: u64, query: String, cx: &mut Context<Self>) {
        if self.seerr.discover.ticket != ticket {
            return;
        }
        self.ask_seerr(
            cx,
            move |seerr| seerr.search(&query),
            move |shell, found, _| {
                let page = &mut shell.seerr.discover;
                if page.ticket != ticket {
                    return;
                }
                page.results = Some(match found {
                    Ok(found) => Loadable::Ready(found),
                    Err(error) => Loadable::Failed(discover::error_line(&error)),
                });
            },
        );
    }

    /// The library page of a title, when the library holds it.
    pub fn library_page_of(&self, kind: MediaKind, tmdb: u64) -> Option<bobine::provider::ItemId> {
        discover::in_library(self.seerr.index.as_ref(), kind, tmdb)
    }

    /// A card of Discover: a title in the library opens its own page.
    pub fn open_discovered(&mut self, kind: MediaKind, tmdb: u64, cx: &mut Context<Self>) {
        match (kind, self.library_page_of(kind, tmdb)) {
            (MediaKind::Movie, Some(film)) => self.open_film(film, cx),
            (MediaKind::Tv, Some(series)) => self.open_series(series, cx),
            (_, None) => self.open_request(kind, tmdb, cx),
        }
    }

    /// Reads, once, what the server recommends after a title of the library: a film or series
    /// page shows those the library does not hold.
    pub(super) fn load_similar(
        &mut self,
        kind: MediaKind,
        tmdb: Option<TmdbId>,
        cx: &mut Context<Self>,
    ) {
        let Some(tmdb) = tmdb else {
            return;
        };
        if self.seerr.similar.contains_key(&(kind, tmdb)) {
            return;
        }
        self.ask_seerr(
            cx,
            move |seerr| seerr.recommendations(kind, tmdb),
            move |shell, similar, _| {
                // Without them the page is whole: the row hides.
                if let Ok(similar) = similar {
                    shell.seerr.similar.insert((kind, tmdb), similar);
                }
            },
        );
    }

    /// "Similar, not in your library", after a title of the library.
    pub fn similar_not_in_library(&self, kind: MediaKind, tmdb: Option<TmdbId>) -> Vec<Discovered> {
        let Some(similar) = tmdb.and_then(|tmdb| self.seerr.similar.get(&(kind, tmdb))) else {
            return Vec::new();
        };
        discover::not_in_library(similar.clone(), self.seerr.index.as_ref())
    }

    /// The palette's "Search on Discover": Discover opens on what was typed.
    pub fn search_on_discover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = &self.search else {
            return;
        };
        let typed = state.input.read(cx).text().trim().to_string();
        self.close_search(window, cx);
        self.select(crate::navigation::NavEntry::Discover, cx);
        if let Some(field) = self.seerr.discover.field.clone() {
            field.update(cx, |field, cx| field.set_text(&typed, cx));
        }
    }

    /// The wheel over Discover: the page up and down, the row under the pointer sideways.
    pub fn scroll_discover(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let page = &mut self.seerr.discover;
        if route_wheel(
            &page.scroll,
            &page.row_scrolls,
            &mut page.gesture,
            event,
            window,
        ) {
            cx.notify();
        }
    }

    fn seerr_checked(&mut self, check: Result<ServerCheck, RequestError>) {
        match check {
            Ok(check) => {
                let (version, owner) = crate::discover::check_lines(&check);
                self.seerr.check = Some(Loadable::Ready(version));
                self.seerr.owner_line = owner;
            }
            Err(error) => {
                self.seerr.check = Some(Loadable::Failed(crate::discover::error_line(&error)));
                self.seerr.owner_line = None;
            }
        }
    }

    /// The Settings' "Test": the version, or why the connection failed.
    pub fn test_seerr(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.seerr.client.clone() else {
            // Why there is no client was said already; without a word, both are missing.
            if !matches!(self.seerr.check, Some(Loadable::Failed(_))) {
                self.seerr.check =
                    Some(Loadable::Failed("Enter the address and the API key first."));
            }
            cx.notify();
            return;
        };
        self.seerr.check = Some(Loadable::Loading);
        let making = self.seerr.making;
        let task = cx
            .background_executor()
            .spawn(async move { client.check() });
        cx.spawn(async move |this, cx| {
            let check = task.await;
            this.update(cx, |shell, cx| {
                if shell.seerr.making == making {
                    shell.seerr_checked(check);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use bobine::provider::{Connector, TokenStore};
    use bobine::requests::RequestError;
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::config::Settings;
    use crate::fake::FILM;
    use crate::fake::{FakeServer, MemoryKeyring};
    use crate::fake_seerr::FakeSeerr;
    use crate::navigation::NavEntry;
    use bobine::provider::ItemId;

    pub const ADDRESS: &str = "https://seerr.example.org";
    pub const KEY: &str = "k3y-seerr-0123456789abcdef";
    pub const ACCOUNT: &str = "setting/server-1/user-1/seerr-api-key";

    /// A shell on a kept session, its settings at `path`, with the fake request server behind
    /// whatever is configured.
    pub fn start(
        path: Option<PathBuf>,
        keyring: &Arc<MemoryKeyring>,
        seerr: &FakeSeerr,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Shell>, FakeServer) {
        let server = FakeServer::new(6, true);
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let secrets: Arc<dyn TokenStore> = keyring.clone();
        let window = cx.add_window(|_, cx| {
            Shell::new(None, Box::new(move || Ok(connector)), path, secrets, cx)
        });
        let fake = seerr.clone();
        window
            .update(cx, |shell, _, _| {
                shell.open_seerr = Arc::new(move |address, key, viewer| {
                    assert_eq!(address, ADDRESS);
                    assert_eq!(key.expose_secret(), KEY);
                    assert_eq!(viewer, crate::fake::USER_ID);
                    Ok(Arc::new(fake.clone()) as Arc<dyn RequestServer>)
                })
            })
            .unwrap();
        cx.run_until_parked();
        (window, server)
    }

    /// A shell where the request server is configured: its address in the settings, its key in
    /// the keyring.
    pub fn configured(seerr: &FakeSeerr, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("settings.toml");
        Settings {
            seerr_address: Some(ADDRESS.into()),
            ..Settings::default()
        }
        .save(&path)
        .unwrap();
        let keyring = Arc::new(MemoryKeyring::default());
        keyring.save(ACCOUNT, &Token::new(KEY.into())).unwrap();
        start(Some(path), &keyring, seerr, cx).0
    }

    pub fn drawn(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> VisualTestContext {
        let visual = VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        visual
    }

    pub fn click(visual: &mut VisualTestContext, selector: &'static str) {
        let at = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();
    }

    fn connected(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |shell, _, _| shell.seerr.client.is_some())
            .unwrap()
    }

    #[gpui_kit::test]
    fn not_configured_only_the_integrations_section_shows_with_its_empty_fields(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        let (window, _) = start(None, &Arc::new(MemoryKeyring::default()), &seerr, cx);
        assert!(!connected(&window, cx));
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.seerr.address.as_ref().unwrap().read(cx).text(), "");
                assert!(!shell.seerr.key.as_ref().unwrap().read(cx).is_saved());
                shell.open_settings(cx);
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("seerr-address").is_some());
        assert!(visual.debug_bounds("seerr-owner").is_none());
        assert_eq!(seerr.state.lock().unwrap().record.checks, 0);
    }

    #[gpui_kit::test]
    fn the_address_is_kept_in_the_settings_and_the_key_in_the_keyring_only(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let keyring = Arc::new(MemoryKeyring::default());
        let seerr = FakeSeerr::default();
        let (window, _) = start(Some(path.clone()), &keyring, &seerr, cx);
        let key = window
            .update(cx, |shell, _, cx| {
                let address = shell.seerr.address.clone().unwrap();
                address.update(cx, |field, cx| field.set_text(ADDRESS, cx));
                shell.save_seerr_address(cx);
                shell.seerr.key.clone().unwrap()
            })
            .unwrap();
        assert!(!connected(&window, cx), "no key yet");
        key.update(cx, |field, cx| {
            field.type_in(KEY, cx);
            field.save(cx);
        });
        cx.run_until_parked();
        assert!(connected(&window, cx));
        assert_eq!(keyring.secret(ACCOUNT).as_deref(), Some(KEY));
        assert_eq!(
            Settings::read(&path).seerr_address.as_deref(),
            Some(ADDRESS)
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(KEY), "{text}");
        // Clearing the key takes the client away.
        key.update(cx, |field, cx| field.clear(cx));
        cx.run_until_parked();
        assert!(!connected(&window, cx));
    }

    /// A shell whose settings hold the address, its keyring nothing.
    fn address_only(seerr: &FakeSeerr, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("settings.toml");
        Settings {
            seerr_address: Some(ADDRESS.into()),
            ..Settings::default()
        }
        .save(&path)
        .unwrap();
        start(Some(path), &Arc::new(MemoryKeyring::default()), seerr, cx).0
    }

    #[gpui_kit::test]
    fn an_address_saved_without_a_key_shows_no_pill_and_asks_for_the_key(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = address_only(&seerr, cx);
        assert!(!connected(&window, cx));
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("nav Discover").is_none());
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    shell.seerr.check,
                    Some(Loadable::Failed("Enter the API key."))
                ));
                assert_eq!(shell.seerr.owner_line, None);
            })
            .unwrap();
        assert!(visual.debug_bounds("seerr-check").is_some());
        assert!(visual.debug_bounds("seerr-owner").is_none());
        assert_eq!(seerr.state.lock().unwrap().record.checks, 0);
    }

    #[gpui_kit::test]
    fn an_address_typed_without_a_scheme_is_kept_with_https(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let keyring = Arc::new(MemoryKeyring::default());
        keyring.save(ACCOUNT, &Token::new(KEY.into())).unwrap();
        let seerr = FakeSeerr::default();
        let (window, _) = start(Some(path.clone()), &keyring, &seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                let address = shell.seerr.address.clone().unwrap();
                address.update(cx, |field, cx| field.set_text("seerr.example.org", cx));
                shell.save_seerr_address(cx);
                assert_eq!(address.read(cx).text(), ADDRESS);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(connected(&window, cx));
        assert_eq!(
            Settings::read(&path).seerr_address.as_deref(),
            Some(ADDRESS)
        );
    }

    #[gpui_kit::test]
    fn the_test_says_why_the_client_could_not_be_made(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_seerr = Arc::new(|_, _, _| Err(RequestError::InvalidAddress));
                shell
                    .seerr
                    .address
                    .clone()
                    .unwrap()
                    .update(cx, |field, cx| {
                        field.set_text("https://seerr.example.org/other", cx)
                    });
                shell.save_seerr_address(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!connected(&window, cx));
        window
            .update(cx, |shell, _, cx| shell.test_seerr(cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    shell.seerr.check,
                    Some(Loadable::Failed(
                        "The address must start with http:// or https://."
                    ))
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn emptying_the_address_takes_discover_away_and_back_lands_on_home(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_discover(&seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_settings(cx);
                let address = shell.seerr.address.clone().unwrap();
                address.update(cx, |field, cx| field.set_text("", cx));
                shell.save_seerr_address(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(!connected(&window, cx));
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("nav Discover").is_none());
        window
            .update(cx, |shell, _, cx| assert!(shell.go_back(cx)))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn a_configured_connection_is_tested_and_shows_its_version(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        assert!(connected(&window, cx));
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "seerr-test");
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    &shell.seerr.check,
                    Some(Loadable::Ready(line)) if line == "Connected to Seerr 2.7.3."
                ));
            })
            .unwrap();
        assert!(visual.debug_bounds("seerr-check").is_some());
    }

    #[gpui_kit::test]
    fn a_wrong_key_says_so(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().failure = Some(RequestError::KeyRefused);
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.test_seerr(cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    shell.seerr.check,
                    Some(Loadable::Failed("Seerr refused the API key."))
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn without_a_seerr_user_for_the_viewer_the_settings_say_requests_go_as_the_owner(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().no_viewer = true;
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("seerr-owner").is_some());
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.seerr.owner_line.as_deref(),
                    Some("No Seerr user has your Jellyfin account: requests go as Owner.")
                );
            })
            .unwrap();
    }

    fn on_discover(seerr: &FakeSeerr, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let window = configured(seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Discover, cx))
            .unwrap();
        cx.run_until_parked();
        window
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    #[gpui_kit::test]
    fn not_configured_no_pill_no_row_no_palette_line(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let (window, _) = start(None, &Arc::new(MemoryKeyring::default()), &seerr, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("nav Home").is_some());
        assert!(visual.debug_bounds("nav Discover").is_none());
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Discover, cx))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Home);
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("similar-not-in-library").is_none());
        palette_typed(&window, "winter", cx);
        let mut visual = drawn(&window, cx);
        assert!(
            window
                .update(cx, |shell, _, _| shell.search_is_open())
                .unwrap()
        );
        assert!(visual.debug_bounds("palette-discover").is_none());
        let record = &seerr.state.lock().unwrap().record;
        assert_eq!((record.checks, record.my_requests), (0, 0));
    }

    /// The Ctrl+K palette open, `query` typed in it.
    fn palette_typed(window: &WindowHandle<Shell>, query: &str, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, window, cx| {
                assert!(shell.open_search(window, cx));
                let input = shell.search.as_ref().unwrap().input.clone();
                input.update(cx, |field, cx| field.set_text(query, cx));
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn configured_the_palette_offers_to_search_on_discover(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        palette_typed(&window, "winter", cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "palette-discover");
        assert_eq!(screen(&window, cx), Screen::Discover);
        cx.executor().advance_clock(SEARCH_AFTER);
        cx.run_until_parked();
        assert_eq!(seerr.state.lock().unwrap().record.searches, ["winter"]);
        window
            .update(cx, |shell, _, _| assert!(!shell.search_is_open()))
            .unwrap();
    }

    #[gpui_kit::test]
    fn configured_the_discover_pill_shows_and_opens_discover(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "nav Discover");
        assert_eq!(screen(&window, cx), Screen::Discover);
        assert!(visual.debug_bounds("discover-page").is_some());
    }

    #[gpui_kit::test]
    fn the_rows_load(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_discover(&seerr, cx);
        window
            .update(cx, |shell, _, _| {
                for row in &shell.seerr.discover.rows {
                    assert!(matches!(row, Some(Loadable::Ready(titles)) if !titles.is_empty()));
                }
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(
            visual
                .debug_bounds("discover-card Northern Lights")
                .is_some()
        );
        // A title of the library says so; one asked for says it is pending.
        assert!(visual.debug_bounds("status In library").is_some());
        assert!(visual.debug_bounds("status Pending").is_some());
    }

    #[gpui_kit::test]
    fn my_requests_are_asked_again_each_time_discover_opens(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_discover(&seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.select(NavEntry::Home, cx);
                shell.select(NavEntry::Discover, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(seerr.state.lock().unwrap().record.my_requests, 2);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Requested").is_some());
    }

    fn type_in(window: &WindowHandle<Shell>, query: &str, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| {
                let field = shell.seerr.discover.field.clone().unwrap();
                field.update(cx, |field, cx| field.set_text(query, cx));
            })
            .unwrap();
        cx.run_until_parked();
    }

    /// Types `query`, then waits for the search to go out and come back.
    fn search(window: &WindowHandle<Shell>, query: &str, cx: &mut TestAppContext) {
        type_in(window, query, cx);
        cx.executor().advance_clock(SEARCH_AFTER);
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_search_shows_results_with_their_status(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_discover(&seerr, cx);
        type_in(&window, "w", cx);
        search(&window, "winter", cx);
        assert_eq!(
            seerr.state.lock().unwrap().record.searches,
            ["winter"],
            "one search once the typing stopped"
        );
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("discover-results").is_some());
        for card in [
            "discover-card The Last Winter",
            "discover-card Winter Light",
            "discover-card Winter's End",
        ] {
            assert!(visual.debug_bounds(card).is_some(), "{card}");
        }
        assert!(visual.debug_bounds("status In library").is_some());
        // Emptied, the rows come back.
        search(&window, "", cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("discover-results").is_none());
    }

    #[gpui_kit::test]
    fn a_title_already_in_the_library_opens_its_normal_page(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_discover(&seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_discovered(MediaKind::Movie, crate::fake::IN_LIBRARY_FILM, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.details.last().unwrap().id, ItemId(FILM.into()))
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn seerr_down_says_so_in_a_line_and_the_page_stays(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        seerr.state.lock().unwrap().failure = Some(RequestError::Unreachable("down".into()));
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Discover, cx))
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("discover-page").is_some());
        assert!(visual.debug_bounds("discover-search").is_some());
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    shell.seerr.discover.rows[0],
                    Some(Loadable::Failed("Seerr cannot be reached."))
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn clearing_the_key_takes_discover_away_and_leaves_its_screen(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_discover(&seerr, cx);
        let key = window
            .update(cx, |shell, _, _| shell.seerr.key.clone().unwrap())
            .unwrap();
        key.update(cx, |field, cx| field.clear(cx));
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Home);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("nav Discover").is_none());
    }
}
