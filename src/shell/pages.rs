//! What the screens show and how it comes from the server. Every call to the server runs on the
//! background executor; its answer comes back to the window's thread, which updates the page.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bobine::provider::{
    Collection, Connector, FilmDetail, FilmQuery, FilterChoices, ItemId, LibraryKind, MediaServer,
    MediaSource, Page, PersonHit, PlayMethod, Playback as Opened, PlaybackReport, ProviderError,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, Context, Entity, Image, NavigationDirection, Pixels, Point, RenderImage, ScrollHandle,
    ScrollWheelEvent, Subscription, Task, TouchPhase, UniformListScrollHandle, Window, point, px,
};

use super::{Film as Playing, HomePage, Shell};
use crate::chrome::Account;
use crate::fallback::Fallback;
use crate::favourite::{self, ListPage, Toggle};
use crate::film_page::{self, Choices, From, Plan};
use crate::grid::{Grid, Listed, RETRY_AFTER};
use crate::home_page::ListKind;
use crate::images::{self, Key};
use crate::input::TextInput;
use crate::library_tools::{Sort, Tools};
use crate::menu::Menus;
use crate::motion::{self, Fade, Glide, Presence};
use crate::navigation::{NavEntry, Screen};
use crate::player::libmpv::Libmpv;
use crate::poster_actions::{Action, Said};
use crate::reports::Reporting;
use crate::scroll::{self, Move, Phase, Wheel};
use crate::series_page::Next;
use crate::start::{self, Start};

/// A page gone back from, for the mouse's forward button to open again.
pub(super) enum Reopen {
    Film(ItemId),
    /// The series, and the season it was left on.
    Series(ItemId, Option<ItemId>),
    Person(PersonHit),
    List(ListKind, Option<String>),
    Collection(ItemId),
    Favorites,
    Playlist(ItemId),
}

/// How long the address must stay unchanged before the server is asked who it is.
const PROBE_AFTER: Duration = Duration::from_millis(600);

pub enum Loadable<T> {
    Loading,
    Ready(T),
    Failed(&'static str),
}

pub struct LoginForm {
    pub address: Entity<TextInput>,
    pub user: Entity<TextInput>,
    pub password: Entity<TextInput>,
    /// What the server at the address said it is, or why it did not.
    pub server: Option<Result<String, &'static str>>,
    probed: String,
    probing: Option<Task<()>>,
    pub error: Option<&'static str>,
    pub busy: bool,
    /// The busy form is waiting for the server to say it signed the user out, not for a login.
    pub signing_out: bool,
    _address_changed: Subscription,
}

/// The menus of the tools above a grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Genre,
    Decade,
    Sort,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Genre, Tool::Decade, Tool::Sort];
}

/// The films or the series of the first library of their kind of the user.
pub struct LibraryPage<T> {
    pub library: Option<Loadable<ItemId>>,
    pub grid: Grid<T>,
    pub scroll: UniformListScrollHandle,
    /// The filters and the order, kept for as long as the program runs.
    pub tools: Tools,
    /// The genres and decades to choose from, asked once the library is known.
    pub choices: Option<Loadable<FilterChoices>>,
    /// Why the list could not be had, when its first page failed.
    pub error: Option<&'static str>,
    pub menus: Menus<Tool>,
    /// The menu drawn, kept while it fades out.
    pub menu_fade: Presence<Tool>,
    /// Where each tool's chip was last drawn, so its menu opens under it.
    pub chips: [ChipBounds; 3],
    /// How much of the list shows: it comes in when a list arrives and after a jump.
    pub appear: Glide,
    /// The list whose arrival `appear` has already played.
    pub appeared: Option<u64>,
    /// The index of the title to scroll to, found by a letter of the alphabet bar.
    pub jump: Option<usize>,
    /// A letter found before the list it belongs to had arrived: the list it was found for,
    /// and the number of titles before the letter. The jump is made when the list lands.
    pub waiting_jump: Option<(u64, u32)>,
    /// The alphabet bar comes and goes with the sort by title.
    pub letters: Fade,
    /// How much the "Non vus" chip is lit.
    pub unwatched: Fade,
    /// Where the current letter is on the bar, as a position that glides from letter to letter.
    pub highlight: Glide,
}

impl<T> Default for LibraryPage<T> {
    fn default() -> Self {
        Self {
            library: None,
            grid: Grid::default(),
            scroll: UniformListScrollHandle::default(),
            tools: Tools::default(),
            choices: None,
            error: None,
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            chips: Default::default(),
            appear: Glide::new(1., motion::page()),
            appeared: None,
            jump: None,
            waiting_jump: None,
            letters: Fade::new(motion::quick()),
            unwatched: Fade::new(motion::quick()),
            highlight: Glide::new(0., motion::quick()),
        }
    }
}

/// An opacity this low is gone: the list leaving has left.
pub const GONE: f32 = 0.02;

impl<T: Listed> LibraryPage<T> {
    /// Changes the filters or the order. When they did change, the list starts over from the
    /// top: the pages of the old one are forgotten and its late answers will not be taken.
    pub fn change(&mut self, change: impl FnOnce(&mut Tools)) -> bool {
        let before = self.tools.clone();
        change(&mut self.tools);
        if self.tools == before {
            return false;
        }
        self.grid.restart();
        self.error = None;
        self.jump = None;
        self.waiting_jump = None;
        self.menus.close();
        true
    }

    /// The server counted `index` titles before a letter, for the list asked under `epoch`: the
    /// grid goes there, unless the list has changed since. A list still on its way is waited
    /// for: its size is not known yet.
    pub fn found_letter(&mut self, epoch: u64, index: u32) {
        if !self.grid.is_current(epoch) {
            return;
        }
        match self.grid.total {
            Some(total) => self.jump = Some(index.min(total.saturating_sub(1)) as usize),
            None => self.waiting_jump = Some((epoch, index)),
        }
    }

    /// Called at each draw, with the draw's instant: while a list is awaited the old one fades
    /// out; once it is gone and the new one is there, the new one starts at the top and fades in.
    pub fn follow_list(&mut self, now: Instant) {
        if self.grid.has_stale() {
            // The new list is given up: the old one comes back, with the failure and its way
            // to try again, rather than a blank.
            self.appear.set(if self.gave_up() { 1. } else { 0. }, now);
            if self.grid.total.is_none() || self.appear.value(now) > GONE {
                return;
            }
            self.grid.drop_stale();
        }
        if self.grid.total.is_some() && self.appeared != Some(self.grid.epoch()) {
            self.appeared = Some(self.grid.epoch());
            self.scroll
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(0.)));
            self.appear.jump(0.);
            self.appear.set(1., now);
        }
    }

    /// The pages to ask for the rows in view. While the old list is kept on screen the rows in
    /// view are its, not the new list's: its first page is what is wanted.
    pub fn pages_wanted(&mut self, items: Range<usize>, now: Instant) -> Vec<u32> {
        let items = if self.grid.has_stale() { 0..1 } else { items };
        self.grid.pages_to_ask(items, now)
    }

    /// The letter jump to make now: not while the old list is leaving, it would scroll that one.
    pub fn take_jump(&mut self) -> Option<usize> {
        if self.grid.has_stale() {
            return None;
        }
        self.jump.take()
    }

    /// Whether the cells of the list on screen are drawn: not once the old list has faded out,
    /// it would still take clicks, opacity does not stop them.
    pub fn cells_visible(&self, now: Instant) -> bool {
        !self.grid.has_stale() || self.appear.value(now) > GONE
    }

    /// The first page of the list was asked again until it was given up, and no list is there:
    /// the failure is shown, with a way to try again.
    pub fn gave_up(&self) -> bool {
        self.grid.total.is_none() && self.grid.gave_up(0)
    }

    /// "Retry": the first page is asked anew, as many times as the first time.
    pub fn retry(&mut self) {
        self.grid.forget_failures();
        self.error = None;
    }

    /// The count in the header: none while the new list is awaited, the old total is not its.
    pub fn header_count(&self) -> Option<u32> {
        if self.grid.has_stale() {
            return None;
        }
        self.grid.total
    }

    /// A page of the list asked under `epoch` came back.
    pub fn loaded(&mut self, epoch: u64, page: u32, items: Page<T>) {
        self.grid.loaded(epoch, page, items);
        if let Some((waiting, index)) = self.waiting_jump.take() {
            self.found_letter(waiting, index);
        }
    }
}

/// What a root screen asks for when it shows.
#[derive(Debug, PartialEq, Eq)]
pub enum Arrival {
    Home,
    /// A full list of the Home: what was started or comes next changed with the playback.
    HomeList,
    /// The Lists are asked again each time they show: a title put in one elsewhere shows.
    MyList,
    /// So are the favorites.
    Favorites,
    /// Back on a playlist's page from a title: its titles' state is asked again.
    Playlist,
    Collections,
    /// Back on a collection page from a film: its films' marks are asked again.
    Collection,
    Library(LibraryKind),
    /// A library already known is not asked again: its tools and its list stay as they were.
    Nothing,
}

pub fn arrival(screen: Screen, films_known: bool, series_known: bool) -> Arrival {
    match screen {
        Screen::Home => Arrival::Home,
        Screen::HomeList => Arrival::HomeList,
        Screen::MyList => Arrival::MyList,
        Screen::Favorites => Arrival::Favorites,
        Screen::Playlist => Arrival::Playlist,
        Screen::Collections => Arrival::Collections,
        Screen::CollectionDetail => Arrival::Collection,
        Screen::Library if !films_known => Arrival::Library(LibraryKind::Films),
        Screen::Series if !series_known => Arrival::Library(LibraryKind::Series),
        _ => Arrival::Nothing,
    }
}

/// The page shown again once the player is left, asked anew.
#[derive(Debug, PartialEq, Eq)]
pub enum Reload {
    Series(usize),
    Film(usize),
    Home,
    /// A full list of the Home.
    List,
    /// The playlist's page on top.
    Playlist,
}

/// What is asked again after a playback, on `screen`, the one back to: the Home or one of its
/// lists, else the series page on top, else the film page on top, if any.
pub fn reload_after_playback(screen: Screen, films: usize, series: usize) -> Option<Reload> {
    match screen {
        Screen::Home => return Some(Reload::Home),
        Screen::HomeList => return Some(Reload::List),
        Screen::Playlist => return Some(Reload::Playlist),
        _ => {}
    }
    if screen == Screen::SeriesDetail {
        return series.checked_sub(1).map(Reload::Series);
    }
    films.checked_sub(1).map(Reload::Film)
}

/// The page a play button was pressed on, at its depth, with what it shows.
pub enum Origin {
    Film(usize, ItemId),
    Series(usize, ItemId),
    /// A playlist's page: its play buttons, and the card of its next title.
    Playlist(usize, ItemId),
    /// The header or a card of the Home or of one of its lists.
    Home,
}

/// The wheel or the touchpad over a detail page: the page goes up and down, the row under the
/// pointer sideways, never both. Whether something moved.
pub fn route_wheel(
    page: &ScrollHandle,
    rows: &[ScrollHandle],
    gesture: &mut scroll::Gesture,
    event: &ScrollWheelEvent,
    window: &Window,
) -> bool {
    let delta = event.delta.pixel_delta(window.line_height());
    let wheel = Wheel {
        dx: delta.x.into(),
        dy: delta.y.into(),
        precise: event.delta.precise(),
        shift: event.modifiers.shift,
        phase: match event.touch_phase {
            TouchPhase::Started => Phase::Started,
            TouchPhase::Moved => Phase::Moved,
            TouchPhase::Ended | TouchPhase::Cancelled => Phase::Ended,
        },
    };
    let row = rows
        .iter()
        .position(|row| row.bounds().contains(&event.position));
    let (handle, delta) = match gesture.route(wheel, row, Instant::now()) {
        Some(Move::Page(dy)) => (page, point(px(0.), px(dy))),
        Some(Move::Row(row, dx)) => (&rows[row], point(px(dx), px(0.))),
        None => return false,
    };
    let (offset, max) = (handle.offset(), handle.max_offset());
    let to = point(
        px(scroll::moved(offset.x.into(), delta.x.into(), max.x.into())),
        px(scroll::moved(offset.y.into(), delta.y.into(), max.y.into())),
    );
    if to == offset {
        return false;
    }
    handle.set_offset(to);
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Version,
    Audio,
    Subtitle,
}

impl Chip {
    pub const ALL: [Chip; 3] = [Chip::Version, Chip::Audio, Chip::Subtitle];

    /// The choice made in this chip's menu, for a film or an episode with `versions`.
    pub fn choose(self, choices: &mut Choices, versions: &[MediaSource], value: Option<i32>) {
        match self {
            Chip::Version => choices.set_version(versions, value.unwrap_or(0) as usize),
            Chip::Audio => choices.choose_audio(versions, value),
            Chip::Subtitle => choices.choose_subtitle(versions, value),
        }
    }
}

pub struct DetailPage {
    pub id: ItemId,
    /// Told apart from every other page ever opened, the closed ones included.
    generation: u64,
    pub detail: Loadable<FilmDetail>,
    pub choices: Option<Choices>,
    /// Hidden while unknown, when the film is in none, or when the lookup failed.
    pub collection: Option<Collection>,
    pub menus: Menus<Chip>,
    /// The menu drawn, kept while it fades out.
    pub menu_fade: Presence<Chip>,
    /// Where each chip was last drawn, so its menu opens under it.
    pub chips: [Rc<Cell<Option<Bounds<Pixels>>>>; 3],
    /// "My list".
    pub favourite: Toggle,
    pub scroll: ScrollHandle,
    /// The rows that scroll sideways: the cast, the collection, similar titles.
    pub rows: [ScrollHandle; 3],
    gesture: scroll::Gesture,
    /// What went wrong with the last button.
    pub error: Option<&'static str>,
    pub busy: bool,
}

impl ListPage for DetailPage {
    fn item(&self) -> &ItemId {
        &self.id
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn button(&mut self) -> (&mut Toggle, &mut Option<&'static str>) {
        (&mut self.favourite, &mut self.error)
    }
}

impl DetailPage {
    pub(super) fn new(id: ItemId, generation: u64) -> Self {
        DetailPage {
            id,
            generation,
            detail: Loadable::Loading,
            choices: None,
            collection: None,
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            chips: Default::default(),
            favourite: Toggle::default(),
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            gesture: Default::default(),
            error: None,
            busy: false,
        }
    }

    /// A read that failed: only a first load shows the failure. A page already shown stays,
    /// whatever the network does to a read made behind it.
    fn load_failed(&mut self, said: &'static str) {
        if !matches!(self.detail, Loadable::Ready(_)) {
            self.detail = Loadable::Failed(said);
        }
    }

    pub fn ready(&self) -> Option<(&FilmDetail, &Choices)> {
        match (&self.detail, &self.choices) {
            (Loadable::Ready(detail), Some(choices)) => Some((detail, choices)),
            _ => None,
        }
    }
}

/// One page of the films or series of a library, from the server.
type Fetch<T> = fn(&dyn MediaServer, &ItemId, &FilmQuery) -> Result<Page<T>, ProviderError>;

/// Where a chip was last drawn, so its menu opens under it.
pub type ChipBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

/// Opens the connector (the account file and the keyring): it blocks, so it runs off the
/// window's thread, once, at the start.
pub type OpenConnector = Box<dyn FnOnce() -> Result<Arc<dyn Connector>, ProviderError> + Send>;

impl Shell {
    /// The kept session, or the login.
    pub(super) fn begin(&mut self, open: OpenConnector, cx: &mut Context<Self>) {
        let task = cx.background_executor().spawn(async move {
            let connector = open()?;
            let start = start::start(&*connector);
            Ok::<_, ProviderError>((connector, start))
        });
        cx.spawn(async move |this, cx| {
            let started = task.await;
            this.update(cx, |shell, cx| {
                shell.starting = false;
                match started {
                    Ok((connector, start)) => {
                        shell.connector = Some(connector);
                        match start {
                            Start::Home(server) => shell.signed_in(server, cx),
                            Start::Login(why) => shell.show_login(why, cx),
                        }
                    }
                    Err(error) => shell.show_login(Some(start::error_message(&error)), cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn show_login(&mut self, why: Option<&'static str>, cx: &mut Context<Self>) {
        let address = cx.new(|cx| TextInput::new("https://media.example.org", false, cx));
        let user = cx.new(|cx| TextInput::new("Username", false, cx));
        let password = cx.new(|cx| TextInput::new("Password", true, cx));
        let address_changed = cx.observe(&address, |shell, _, cx| shell.probe_soon(cx));
        self.login = Some(LoginForm {
            address,
            user,
            password,
            server: None,
            probed: String::new(),
            probing: None,
            error: why,
            busy: false,
            signing_out: false,
            _address_changed: address_changed,
        });
        self.login_focus_due = true;
        self.server = None;
        self.details.clear();
        self.series_pages.clear();
        self.up_next.cancel();
        self.people.clear();
        self.list = None;
        self.collection_pages.clear();
        self.playlist_pages.clear();
        self.playlist_menu = None;
        self.forward.clear();
        self.search = None;
        self.navigator.switch_root(Screen::Login);
        cx.notify();
    }

    /// Asks the server at the address who it is, once the user stopped typing it.
    fn probe_soon(&mut self, cx: &mut Context<Self>) {
        let (Some(form), Some(connector)) = (&mut self.login, self.connector.clone()) else {
            return;
        };
        let address = form.address.read(cx).text().trim().to_string();
        if address == form.probed {
            return;
        }
        form.probed = address.clone();
        form.server = None;
        if address.is_empty() {
            form.probing = None;
            return;
        }
        let executor = cx.background_executor().clone();
        form.probing = Some(cx.spawn(async move |this, cx| {
            executor.timer(PROBE_AFTER).await;
            let probed = executor
                .spawn(async move { connector.probe(&address) })
                .await;
            this.update(cx, |shell, cx| {
                if let Some(form) = &mut shell.login {
                    form.server = Some(
                        probed
                            .map(|info| start::server_line(&info))
                            .map_err(|error| start::error_message(&error)),
                    );
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub fn submit_login(&mut self, cx: &mut Context<Self>) {
        let Some(form) = &mut self.login else {
            return;
        };
        if form.busy {
            return;
        }
        let Some(connector) = self.connector.clone() else {
            form.error = Some("The account file cannot be written.");
            cx.notify();
            return;
        };
        let address = form.address.read(cx).text().to_string();
        let user = form.user.read(cx).text().to_string();
        // Out of the field at once: from here it lives only in the login call.
        let password = form.password.update(cx, |field, cx| field.take(cx));
        form.busy = true;
        form.error = None;
        let task = cx
            .background_executor()
            .spawn(async move { start::login(&*connector, &address, &user, password) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |shell, cx| {
                match result {
                    Ok(server) => shell.signed_in(server, cx),
                    Err(why) => {
                        if let Some(form) = &mut shell.login {
                            form.busy = false;
                            form.error = Some(why);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn signed_in(&mut self, server: Arc<dyn MediaServer>, cx: &mut Context<Self>) {
        self.server = Some(server);
        self.login = None;
        self.reset_session(cx);
        self.navigator.switch_root(Screen::Home);
        self.load_home(cx);
    }

    /// Forgets what the session before showed: the pages, the images, the menus.
    fn reset_session(&mut self, cx: &mut Context<Self>) {
        self.account_menu.close();
        self.settings_page.menus.close();
        // Their keyring entries stay: the next session of the same user finds them again.
        self.settings_page.secrets.clear();
        self.home = HomePage::default();
        self.library = LibraryPage::default();
        self.series = LibraryPage::default();
        self.collections = Default::default();
        self.my_list = Default::default();
        self.lists = Default::default();
        self.details.clear();
        self.series_pages.clear();
        self.people.clear();
        self.list = None;
        self.collection_pages.clear();
        self.forward.clear();
        self.search = None;
        self.hero_view.update(cx, |hero, cx| {
            hero.forget_logos();
            cx.notify();
        });
        // The atlas holds the old images until they are dropped from it.
        for gone in self.images.cache.drain() {
            cx.drop_image(gone, None);
        }
        self.images = images::Images::default();
    }

    pub fn toggle_account_menu(&mut self, cx: &mut Context<Self>) {
        self.account_menu.toggle(Account);
        cx.notify();
    }

    /// The mouse went down outside the open account menu, maybe on its button.
    pub fn pressed_outside_account_menu(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let on = self
            .account_button
            .get()
            .is_some_and(|bounds| bounds.contains(&position))
            .then_some(Account);
        self.account_menu.pressed_outside(on);
        cx.notify();
    }

    /// Leaves the account. A film playing stops first: its last reports need the token. The
    /// old session is out of reach from this call on, and the login shows at once, from the
    /// server's address, busy until the server was told and the token left the keyring (the HTTP
    /// timeout bounds it). A login made meanwhile would meet a late logout.
    pub fn sign_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(server), Some(connector)) = (self.server.take(), self.connector.clone()) else {
            return;
        };
        if self.playback.is_some() {
            if window.is_fullscreen() {
                window.toggle_fullscreen();
            }
            self.release_tiles(window, cx);
        }
        let reports = self.stop_playback();
        self.follow_blackout(window, cx);
        let address = connector.last_address();
        self.reset_session(cx);
        self.show_login(None, cx);
        if let Some(form) = &mut self.login {
            form.busy = true;
            form.signing_out = true;
            if let Some(address) = address {
                form.address
                    .update(cx, |field, cx| field.set_text(&address, cx));
            }
        }
        let task = cx
            .background_executor()
            .spawn(async move { start::sign_out(&*server, reports) });
        cx.spawn(async move |this, cx| {
            let why = task.await;
            this.update(cx, |shell, cx| shell.signed_out(why, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    /// The server was told and the token left the keyring: the form becomes usable, with why the
    /// local sign out was incomplete if it was. Only the form that waited for it is touched: one
    /// the user is typing in, or a session signed in meanwhile, is left alone.
    fn signed_out(&mut self, why: Option<&'static str>, cx: &mut Context<Self>) {
        if self.server.is_some() {
            return;
        }
        let Some(form) = self.login.as_mut().filter(|form| form.signing_out) else {
            return;
        };
        form.busy = false;
        form.signing_out = false;
        form.error = why;
        cx.notify();
    }

    /// What the user is told; an expired session brings the login back.
    pub(super) fn failed(&mut self, error: ProviderError, cx: &mut Context<Self>) -> &'static str {
        let said = start::error_message(&error);
        if start::brings_the_login_back(&error, self.login.is_some()) {
            self.show_login(Some(said), cx);
        }
        said
    }

    /// Runs `work` against the server off the window's thread, then `then` with its answer.
    pub(super) fn ask<R: Send + 'static>(
        &self,
        cx: &mut Context<Self>,
        work: impl FnOnce(&dyn MediaServer) -> R + Send + 'static,
        then: impl FnOnce(&mut Self, R, &mut Context<Self>) + 'static,
    ) {
        let Some(server) = self.server.clone() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { work(&*server) });
        cx.spawn(async move |this, cx| {
            let answer = task.await;
            this.update(cx, |shell, cx| {
                then(shell, answer, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A click on the pill bar.
    pub fn select(&mut self, entry: NavEntry, cx: &mut Context<Self>) {
        if !self.navigator.select(entry) {
            return;
        }
        self.forward.clear();
        self.details.clear();
        self.series_pages.clear();
        self.up_next.cancel();
        self.people.clear();
        self.list = None;
        self.collection_pages.clear();
        self.arrived(cx);
        cx.notify();
    }

    /// Escape, the back link or the mouse's back button.
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        let gone = match self.navigator.current() {
            Screen::FilmDetail => self.details.pop().map(|page| Reopen::Film(page.id)),
            Screen::SeriesDetail => self.series_pages.pop().map(|page| {
                let season = page
                    .seasons
                    .get(page.season)
                    .map(|season| season.id.clone());
                Reopen::Series(page.id, season)
            }),
            Screen::Person => self.people.pop().map(|page| {
                Reopen::Person(PersonHit {
                    id: page.id,
                    name: page.name,
                    photo: page.photo,
                })
            }),
            Screen::HomeList => self
                .list
                .take()
                .map(|list| Reopen::List(list.kind, list.library)),
            Screen::CollectionDetail => self
                .collection_pages
                .pop()
                .map(|page| Reopen::Collection(page.id)),
            Screen::Favorites => Some(Reopen::Favorites),
            Screen::Playlist => self
                .playlist_pages
                .pop()
                .map(|page| Reopen::Playlist(page.id)),
            _ => None,
        };
        self.up_next.cancel();
        if !self.navigator.back() {
            return false;
        }
        self.forward.extend(gone);
        self.arrived(cx);
        cx.notify();
        true
    }

    /// The mouse's forward button: the page last gone back from opens again, read afresh.
    pub fn go_forward(&mut self, cx: &mut Context<Self>) -> bool {
        // Opening a page forgets what was ahead: the rest is put back after.
        let mut forward = std::mem::take(&mut self.forward);
        let Some(page) = forward.pop() else {
            return false;
        };
        match page {
            Reopen::Film(id) => self.open_film(id, cx),
            Reopen::Series(id, season) => self.open_series_on(id, season, cx),
            Reopen::Person(person) => self.open_person(person, cx),
            Reopen::List(kind, library) => self.open_list(kind, library, cx),
            Reopen::Collection(id) => self.open_collection(id, cx),
            Reopen::Favorites => self.open_favorites(cx),
            Reopen::Playlist(id) => self.open_playlist(id, cx),
        }
        self.forward = forward;
        true
    }

    /// A side button of the mouse, wherever the pointer is. Back does what Escape does. Not on
    /// the login nor in the player, where a film would stop on a stray press.
    pub fn navigate(
        &mut self,
        direction: NavigationDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.navigator.current().shows_pill_bar() {
            return;
        }
        if self.search_is_open() {
            if direction == NavigationDirection::Back {
                self.close_search(window, cx);
            }
            return;
        }
        match direction {
            NavigationDirection::Back => self.escape(cx),
            NavigationDirection::Forward => {
                self.go_forward(cx);
            }
        }
    }

    /// A root screen shows: what it lists may have changed since (a film watched, resumed).
    fn arrived(&mut self, cx: &mut Context<Self>) {
        let arrival = arrival(
            self.navigator.current(),
            self.library.library.is_some(),
            self.series.library.is_some(),
        );
        match arrival {
            Arrival::Home => self.load_home(cx),
            Arrival::HomeList => self.refresh_list(cx),
            Arrival::MyList => self.load_lists(cx),
            Arrival::Favorites => self.load_my_list(cx),
            Arrival::Playlist => self.reload_playlist(cx),
            Arrival::Collections => self.load_collections(cx),
            Arrival::Collection => self.reload_collection(cx),
            Arrival::Library(kind) => self.load_library(kind, cx),
            Arrival::Nothing => {}
        }
    }

    /// The first library of `kind` of the user, then its first page.
    fn load_library(&mut self, kind: LibraryKind, cx: &mut Context<Self>) {
        *self.library_of(kind) = Some(Loadable::Loading);
        self.ask(
            cx,
            move |server| {
                let libraries = server.libraries()?;
                Ok(libraries
                    .into_iter()
                    .find(|library| library.kind == kind)
                    .map(|library| library.id))
            },
            move |shell, found: Result<Option<ItemId>, ProviderError>, cx| {
                let loaded = match found {
                    Ok(Some(library)) => Loadable::Ready(library),
                    Ok(None) if kind == LibraryKind::Series => {
                        Loadable::Failed("No series library on this server.")
                    }
                    Ok(None) => Loadable::Failed("No film library on this server."),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                let library = match &loaded {
                    Loadable::Ready(library) => Some(library.clone()),
                    _ => None,
                };
                *shell.library_of(kind) = Some(loaded);
                match kind {
                    LibraryKind::Series => shell.ask_series(0..1, cx),
                    _ => shell.ask_films(0..1, cx),
                }
                if let Some(library) = library {
                    shell.load_choices(kind, library, cx);
                }
            },
        );
    }

    fn library_of(&mut self, kind: LibraryKind) -> &mut Option<Loadable<ItemId>> {
        match kind {
            LibraryKind::Series => &mut self.series.library,
            _ => &mut self.library.library,
        }
    }

    /// The genres and decades the tools above the grid offer, once the library is known.
    fn load_choices(&mut self, kind: LibraryKind, library: ItemId, cx: &mut Context<Self>) {
        fn set(shell: &mut Shell, kind: LibraryKind, loaded: Loadable<FilterChoices>) {
            match kind {
                LibraryKind::Series => shell.series.choices = Some(loaded),
                _ => shell.library.choices = Some(loaded),
            }
        }
        set(self, kind, Loadable::Loading);
        self.ask(
            cx,
            move |server| server.filter_choices(&library),
            move |shell, found, cx| {
                let loaded = match found {
                    Ok(choices) => Loadable::Ready(choices),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                set(shell, kind, loaded);
            },
        );
    }

    /// The pages of the films at these indexes, when not asked yet.
    pub fn ask_films(&mut self, films: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.ask_pages(
            films,
            |shell| &mut shell.library,
            |s, l, q| s.films(l, q),
            cx,
        );
    }

    /// The pages of the series at these indexes, when not asked yet.
    pub fn ask_series(&mut self, series: std::ops::Range<usize>, cx: &mut Context<Self>) {
        self.ask_pages(
            series,
            |shell| &mut shell.series,
            |s, l, q| s.series(l, q),
            cx,
        );
    }

    fn ask_pages<T: Listed + Said + Send + 'static>(
        &mut self,
        items: std::ops::Range<usize>,
        page_of: fn(&mut Shell) -> &mut LibraryPage<T>,
        fetch: Fetch<T>,
        cx: &mut Context<Self>,
    ) {
        let stamp = self.marks.stamp();
        let shown = page_of(self);
        let Some(Loadable::Ready(library)) = &shown.library else {
            return;
        };
        let library = library.clone();
        let (tools, epoch) = (shown.tools.clone(), shown.grid.epoch());
        for page in shown.pages_wanted(items.clone(), Instant::now()) {
            let (library, tools) = (library.clone(), tools.clone());
            self.ask(
                cx,
                move |server| fetch(server, &library, &tools.query(page)),
                move |shell, items, cx| match items {
                    Ok(items) => {
                        shell.marks.listed(stamp, &items.items);
                        page_of(shell).loaded(epoch, page, items);
                    }
                    Err(error) => {
                        let shown = page_of(shell);
                        shown.grid.failed(epoch, page, Instant::now());
                        if shown.grid.total.is_none() && shown.grid.is_current(epoch) {
                            let said = shell.failed(error, cx);
                            page_of(shell).error = Some(said);
                            // An idle window does not redraw by itself: the retry needs a wake.
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(RETRY_AFTER).await;
                                this.update(cx, |_, cx| cx.notify()).ok();
                            })
                            .detach();
                        }
                    }
                },
            );
        }
    }

    /// Changes the filters or the order of the grid shown; when they did change, its list is
    /// asked again from the first page.
    fn change_tools(&mut self, change: impl FnOnce(&mut Tools), cx: &mut Context<Self>) {
        let screen = self.navigator.current();
        let changed = match screen {
            Screen::Library => self.library.change(change),
            Screen::Series => self.series.change(change),
            _ => false,
        };
        if changed && screen == Screen::Series {
            self.ask_series(0..1, cx);
        } else if changed {
            self.ask_films(0..1, cx);
        }
        cx.notify();
    }

    /// "Retry", when the first page of the list was given up: it is asked again.
    pub fn retry_list(&mut self, cx: &mut Context<Self>) {
        match self.navigator.current() {
            Screen::Library => {
                self.library.retry();
                self.ask_films(0..1, cx);
            }
            Screen::Series => {
                self.series.retry();
                self.ask_series(0..1, cx);
            }
            _ => return,
        }
        cx.notify();
    }

    /// The "Non vus" chip.
    pub fn toggle_unwatched(&mut self, cx: &mut Context<Self>) {
        self.change_tools(
            |tools| tools.filters.unwatched = !tools.filters.unwatched,
            cx,
        );
    }

    /// A choice in the menu of a tool: the index of a genre, the first year of a decade, the
    /// index of a sort; none for "all".
    pub fn pick(&mut self, tool: Tool, value: Option<i32>, cx: &mut Context<Self>) {
        let genre = |choices: &Option<Loadable<FilterChoices>>| match (choices, value) {
            (Some(Loadable::Ready(choices)), Some(index)) => {
                choices.genres.get(index as usize).cloned()
            }
            _ => None,
        };
        let genre = match self.navigator.current() {
            Screen::Library => genre(&self.library.choices),
            Screen::Series => genre(&self.series.choices),
            _ => return,
        };
        self.change_tools(
            |tools| match tool {
                Tool::Genre => tools.filters.genre = genre,
                Tool::Decade => tools.filters.decade = value,
                Tool::Sort => {
                    tools.sort = value
                        .and_then(|index| Sort::ALL.get(index as usize))
                        .copied()
                        .unwrap_or_default()
                }
            },
            cx,
        );
        // Choosing what is already chosen leaves the grid alone but still closes the menu.
        if let Some((menus, _)) = self.tool_menus() {
            menus.close();
        }
    }

    /// A genre of a page's facts: the library of `entry`, filtered on that genre, the same state
    /// as choosing it in the genre menu. It opens over the page, which Back returns to.
    pub fn open_genre(&mut self, entry: NavEntry, genre: String, cx: &mut Context<Self>) {
        let Some(screen) = entry.target() else {
            return;
        };
        self.forward.clear();
        self.navigator.open(screen);
        self.arrived(cx);
        self.change_tools(|tools| tools.filters.genre = Some(genre), cx);
    }

    /// The menus of the tools above the grid shown, and where their chips were drawn.
    fn tool_menus(&mut self) -> Option<(&mut Menus<Tool>, &[ChipBounds; 3])> {
        match self.navigator.current() {
            Screen::Library => Some((&mut self.library.menus, &self.library.chips)),
            Screen::Series => Some((&mut self.series.menus, &self.series.chips)),
            _ => None,
        }
    }

    pub fn toggle_tool_menu(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if let Some((menus, _)) = self.tool_menus() {
            menus.toggle(tool);
            cx.notify();
        }
    }

    /// The mouse went down outside the open menu, maybe on one of the chips.
    pub fn pressed_outside_tool_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((menus, chips)) = self.tool_menus() {
            let on = Tool::ALL.into_iter().find(|tool| {
                chips[*tool as usize]
                    .get()
                    .is_some_and(|bounds| bounds.contains(&position))
            });
            menus.pressed_outside(on);
            cx.notify();
        }
    }

    /// A letter of the alphabet bar: asks the server where its titles start, then the grid goes
    /// there and loads the page it lands on. `#` starts the list.
    pub fn jump_to_letter(&mut self, letter: char, cx: &mut Context<Self>) {
        match self.navigator.current() {
            Screen::Library => self.jump_in(
                |shell| &mut shell.library,
                |s, l, q| s.films(l, q),
                letter,
                cx,
            ),
            Screen::Series => self.jump_in(
                |shell| &mut shell.series,
                |s, l, q| s.series(l, q),
                letter,
                cx,
            ),
            _ => {}
        }
        cx.notify();
    }

    fn jump_in<T: Listed + Send + 'static>(
        &mut self,
        page_of: fn(&mut Shell) -> &mut LibraryPage<T>,
        fetch: Fetch<T>,
        letter: char,
        cx: &mut Context<Self>,
    ) {
        let page = page_of(self);
        let Some(Loadable::Ready(library)) = &page.library else {
            return;
        };
        if !page.tools.letters_shown() {
            return;
        }
        let epoch = page.grid.epoch();
        let Some(query) = page.tools.letter_query(letter) else {
            page.found_letter(epoch, 0);
            return;
        };
        let library = library.clone();
        self.ask(
            cx,
            move |server| fetch(server, &library, &query),
            move |shell, found, _| {
                // A failure leaves the grid where it is: the next click asks again.
                if let Ok(found) = found {
                    page_of(shell).found_letter(epoch, found.total);
                }
            },
        );
    }

    pub fn open_film(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.forward.clear();
        let generation = self.next_generation();
        self.details.push(DetailPage::new(id, generation));
        self.navigator.open(Screen::FilmDetail);
        self.load_detail(self.details.len() - 1, true, cx);
        cx.notify();
    }

    /// The page at `depth` of the detail pages, if it still shows `id`.
    pub(super) fn detail_page(&mut self, depth: usize, id: &ItemId) -> Option<&mut DetailPage> {
        self.details.get_mut(depth).filter(|page| &page.id == id)
    }

    /// The film page of that number at `depth`, if it is still open there: a read answers to
    /// the page that asked it, not to another that shows the same title.
    pub(super) fn detail_numbered(
        &mut self,
        depth: usize,
        generation: u64,
    ) -> Option<&mut DetailPage> {
        favourite::numbered(&mut self.details, depth, generation)
    }

    /// Asks the film, then (once the page shows) its collection: that lookup is slow.
    pub(super) fn load_detail(
        &mut self,
        depth: usize,
        with_collection: bool,
        cx: &mut Context<Self>,
    ) {
        let id = self.details[depth].id.clone();
        let asked = id.clone();
        let generation = self.details[depth].generation;
        let read_at = self.details[depth].favourite.changes();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            move |server| server.film(&asked),
            move |shell, detail, cx| {
                let detail = match detail {
                    Ok(detail) => detail,
                    Err(error) => {
                        let said = shell.failed(error, cx);
                        if let Some(page) = shell.detail_numbered(depth, generation) {
                            page.load_failed(said);
                        }
                        return;
                    }
                };
                shell.library.grid.update(&detail.film);
                let film = &detail.film;
                shell
                    .marks
                    .heard(stamp, &film.id, Action::MyList, detail.favourite);
                shell
                    .marks
                    .heard(stamp, &film.id, Action::Watched, film.watched);
                shell.marks.listed(stamp, &detail.similar);
                let Some(page) = shell.detail_numbered(depth, generation) else {
                    return;
                };
                // Coming back from the player keeps what the chips chose.
                if page
                    .choices
                    .as_ref()
                    .is_none_or(|c| c.source(&detail.versions).is_none())
                {
                    page.choices = Some(Choices::new(&detail.versions));
                }
                page.favourite.sync(detail.favourite, read_at);
                page.detail = Loadable::Ready(detail);
                if with_collection {
                    shell.load_collection(depth, id, cx);
                }
            },
        );
    }

    fn load_collection(&mut self, depth: usize, id: ItemId, cx: &mut Context<Self>) {
        let asked = id.clone();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            move |server| server.collection_of(&asked),
            move |shell, collection, _| {
                let collection = collection.ok().flatten();
                if let Some(collection) = &collection {
                    shell.marks.listed(stamp, &collection.others);
                }
                if let Some(page) = shell.detail_page(depth, &id) {
                    // A failed lookup hides the row: the page is whole without it.
                    page.collection = collection;
                }
            },
        );
    }

    pub fn choose(&mut self, chip: Chip, value: Option<i32>, cx: &mut Context<Self>) {
        if self.navigator.current() == Screen::SeriesDetail {
            self.choose_for_episode(chip, value);
            cx.notify();
            return;
        }
        let Some(page) = self.details.last_mut() else {
            return;
        };
        page.menus.close();
        let (Loadable::Ready(detail), Some(choices)) = (&page.detail, &mut page.choices) else {
            return;
        };
        chip.choose(choices, &detail.versions, value);
        cx.notify();
    }

    /// The wheel or the touchpad over the film page.
    pub fn scroll_film_page(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.details.last_mut() else {
            return;
        };
        if route_wheel(&page.scroll, &page.rows, &mut page.gesture, event, window) {
            // A menu stays under its chip only while the page stands still.
            page.menus.close();
            cx.notify();
        }
    }

    /// The chip menus of the detail page shown, and where its chips were drawn.
    fn chip_menus(&mut self) -> Option<(&mut Menus<Chip>, &[ChipBounds; 3])> {
        match self.navigator.current() {
            Screen::FilmDetail => self.details.last_mut().map(|p| (&mut p.menus, &p.chips)),
            Screen::SeriesDetail => self
                .series_pages
                .last_mut()
                .map(|p| (&mut p.menus, &p.chips)),
            _ => None,
        }
    }

    pub fn toggle_menu(&mut self, chip: Chip, cx: &mut Context<Self>) {
        if let Some((menus, _)) = self.chip_menus() {
            menus.toggle(chip);
            cx.notify();
        }
    }

    /// The mouse went down outside the open menu, maybe on a chip.
    pub fn pressed_outside_menu(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((menus, chips)) = self.chip_menus() {
            let on = Chip::ALL.into_iter().find(|chip| {
                chips[*chip as usize]
                    .get()
                    .is_some_and(|bounds| bounds.contains(&position))
            });
            menus.pressed_outside(on);
            cx.notify();
        }
    }

    /// Escape cancels the next episode's card, else closes the open menu, else goes back.
    pub fn escape(&mut self, cx: &mut Context<Self>) {
        if self.playlist_menu_is_open() {
            self.close_playlist_menu(cx);
            return;
        }
        if self.cancel_up_next(cx) {
            return;
        }
        if self.account_menu.escape() || self.settings_page.menus.escape() {
            cx.notify();
            return;
        }
        if let Some((menus, _)) = self.chip_menus()
            && menus.escape()
        {
            cx.notify();
            return;
        }
        if let Some((menus, _)) = self.tool_menus()
            && menus.escape()
        {
            cx.notify();
            return;
        }
        self.go_back(cx);
    }

    pub fn toggle_watched(&mut self, cx: &mut Context<Self>) {
        let depth = self.details.len().wrapping_sub(1);
        let Some(page) = self.details.last_mut() else {
            return;
        };
        let Loadable::Ready(detail) = &page.detail else {
            return;
        };
        if page.busy {
            return;
        }
        page.busy = true;
        let film = detail.film.clone();
        let id = page.id.clone();
        self.ask(
            cx,
            move |server| film_page::toggle_watched(server, &film),
            move |shell, film, cx| {
                let film = film.map_err(|error| shell.failed(error, cx));
                if let Ok(film) = &film {
                    shell.library.grid.update(film);
                    shell.marks.follow(&film.id, Action::Watched, film.watched);
                }
                let Some(page) = shell.detail_page(depth, &id) else {
                    return;
                };
                page.busy = false;
                match film {
                    Ok(film) => {
                        if let Loadable::Ready(detail) = &mut page.detail {
                            detail.film = film;
                        }
                    }
                    Err(said) => page.error = Some(said),
                }
            },
        );
    }

    /// "▶ Resume" or "↺ From the start": the server opens a playback, then the player starts.
    pub fn play_film(&mut self, from: From, window: &mut Window, cx: &mut Context<Self>) {
        let depth = self.details.len().wrapping_sub(1);
        let Some(page) = self.details.last_mut() else {
            return;
        };
        let Some((detail, choices)) = page.ready() else {
            return;
        };
        if page.busy {
            return;
        }
        let Some(plan) = film_page::plan(&detail.film, &detail.versions, choices, from) else {
            page.error = Some("This film has no version to play.");
            cx.notify();
            return;
        };
        let Some(server) = self.server.clone() else {
            return;
        };
        let mut parts: Vec<String> = detail.film.year.iter().map(i32::to_string).collect();
        parts.extend(
            choices
                .source(&detail.versions)
                .map(film_page::version_label),
        );
        parts.push("direct play".into());
        let playing = Playing {
            title: plan.title.clone(),
            subtitle: Some(parts.join(" · ")),
            list: Some(plan.film.clone()),
        };
        let origin = Origin::Film(depth, page.id.clone());
        page.busy = true;
        page.error = None;
        let (film, source) = (plan.film.clone(), plan.source.clone());
        let task = cx
            .background_executor()
            .spawn(async move { server.play(&film, &source) });
        cx.spawn_in(window, async move |this, cx| {
            let opened = task.await;
            this.update_in(cx, |shell, window, cx| {
                shell.start_playback(origin, plan, opened, playing, None, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The page a play button was pressed on: no longer busy, and why it did not play.
    pub(super) fn tried(
        &mut self,
        origin: &Origin,
        error: Option<&'static str>,
        cx: &mut Context<Self>,
    ) {
        // The header and the posters play from any page: the window says what went wrong where
        // it is, for a moment, in the pill of its other messages.
        let (busy, shown) = match origin {
            Origin::Film(depth, id) => match self.detail_page(*depth, id) {
                Some(page) => (&mut page.busy, &mut page.error),
                None => return,
            },
            Origin::Series(depth, id) => match self.series_pages.get_mut(*depth) {
                Some(page) if &page.id == id => (&mut page.busy, &mut page.error),
                _ => return,
            },
            // A playlist's page has no line of its own: the window says it, as for the Home.
            Origin::Playlist(depth, id) => {
                if let Some(page) = self.playlist_page(*depth, id) {
                    page.busy = false;
                }
                if let Some(said) = error {
                    self.show_notice(said, cx);
                }
                return;
            }
            Origin::Home => {
                self.home.busy = false;
                if let Some(said) = error {
                    self.show_notice(said, cx);
                }
                return;
            }
        };
        *busy = false;
        if error.is_some() {
            *shown = error;
        }
    }

    /// The server opened the playback: the player starts. `next`: the episode after, for the
    /// card at the end.
    #[allow(
        clippy::too_many_arguments,
        reason = "one playback's parts, put together once"
    )]
    pub(super) fn start_playback(
        &mut self,
        origin: Origin,
        plan: Plan,
        opened: Result<Opened, ProviderError>,
        playing: Playing,
        next: Option<Next>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        #[cfg(test)]
        self.planned.push((plan.film.clone(), plan.start));
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => {
                let said = self.failed(error, cx);
                self.tried(&origin, Some(said), cx);
                return;
            }
        };
        self.tried(&origin, None, cx);
        // A test plays on a fake player, through the rest of this path: what the end offers.
        #[cfg(test)]
        if let Some(player) = self.fake_player.take() {
            let display = Box::new(crate::video::fake::FakeDisplay::default());
            let session = crate::player::session::Session::new(
                Box::new(player),
                display,
                playing.title,
                playing.subtitle,
            );
            self.begin_playback(session, None, window, cx);
            if let Some(playback) = &mut self.playback {
                playback.playing = Some(opened.film.clone());
            }
            self.playback_started(&opened.film, next, cx);
            return;
        }
        let options = self.mpv_options.clone();
        let Some(server) = self.server.clone() else {
            return;
        };
        let mpv = match Libmpv::new(&options) {
            Ok(mpv) => Arc::new(mpv),
            Err(error) => {
                eprintln!("bobine: the player cannot start: {error}");
                self.tried(&origin, Some("The player cannot start."), cx);
                return;
            }
        };
        let reporting = Reporting::start(
            server.clone(),
            PlaybackReport {
                session: opened.session.clone(),
                film: opened.film.clone(),
                source: opened.source.clone(),
                position: plan.start,
                paused: false,
                audio_track: plan.audio_index,
                subtitle_track: plan.subtitle_index,
                method: PlayMethod::DirectPlay,
            },
        );
        let externals = plan.externals(&opened);
        let media = plan.media(&opened, &externals);
        let fallback = Fallback::new(server.clone(), plan.clone(), &opened);
        let list = playing.list.clone();
        match self.play(mpv, media, playing, Some(reporting), window, cx) {
            Ok(()) => {
                self.arm_playback(fallback, window, cx);
                if let Some(playback) = &mut self.playback {
                    playback.playing = Some(opened.film.clone());
                }
                self.load_trickplay(opened.film.clone(), opened.source.clone(), cx);
                self.playback_started(&opened.film, next, cx);
                if let Some(item) = list {
                    self.follow_list(item, cx);
                }
            }
            Err(error) => {
                // The words of the video surface or of mpv's error code: no address in them.
                eprintln!("bobine: the film does not play: {error}");
                self.tried(&origin, Some("Cannot play."), cx);
            }
        }
    }

    /// The playback that just began may fall back to the server's conversion and have its quality
    /// chosen; it starts at the default quality of the settings, as the viewer would have chosen
    /// it in the player's menu.
    fn arm_playback(&mut self, fallback: Fallback, window: &mut Window, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        playback.fallback = Some(fallback);
        playback.session.offer_quality();
        let request = playback
            .session
            .choose_quality(self.settings.default_quality.for_player());
        self.player_request(request, window, cx);
    }

    /// Back from the player: once "stopped" reached the server, the film page shows where the
    /// viewer is now.
    pub(super) fn after_playback(&mut self, reports: JoinHandle<()>, cx: &mut Context<Self>) {
        let depth = match reload_after_playback(
            self.navigator.current(),
            self.details.len(),
            self.series_pages.len(),
        ) {
            Some(Reload::Series(depth)) => {
                self.load_series(depth, false, Some(reports), cx);
                return;
            }
            Some(Reload::Film(depth)) => depth,
            Some(Reload::Home) => {
                self.reload_home_after(reports, cx);
                return;
            }
            Some(Reload::List) => {
                self.ask(
                    cx,
                    move |_| {
                        let _ = reports.join();
                    },
                    |shell, (), cx| shell.refresh_list(cx),
                );
                return;
            }
            Some(Reload::Playlist) => {
                self.ask(
                    cx,
                    move |_| {
                        let _ = reports.join();
                    },
                    |shell, (), cx| shell.reload_playlist(cx),
                );
                return;
            }
            None => return,
        };
        let id = self.details[depth].id.clone();
        let asked = id.clone();
        let generation = self.details[depth].generation;
        let read_at = self.details[depth].favourite.changes();
        self.ask(
            cx,
            move |server| {
                // The worker ends once the last report is sent.
                let _ = reports.join();
                server.film(&asked)
            },
            move |shell, detail, _| {
                let Ok(detail) = detail else {
                    return;
                };
                shell.library.grid.update(&detail.film);
                // Continue watching is asked again when Home shows.
                if let Some(page) = shell.detail_numbered(depth, generation)
                    && let Loadable::Ready(shown) = &mut page.detail
                {
                    page.favourite.sync(detail.favourite, read_at);
                    *shown = detail;
                }
            },
        );
    }

    /// The image at `width`, once it is here; asked of the server the first time it is drawn.
    pub fn image(
        &mut self,
        image: Option<&bobine::provider::ImageRef>,
        width: u32,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let image = image?;
        let key = Key::new(image, width);
        if let Some(found) = self.images.cache.get(&key) {
            return Some(found.clone());
        }
        self.images.want(&key, image, Instant::now());
        self.fetch_images(cx);
        // A failed image waiting for its retry is drawn here, with nothing else to end a fetch.
        self.wake_for_image_retries(cx);
        None
    }

    /// The image at `width` if it is here, read without writing anything: for a draw that must
    /// not touch the shell. Its asking is `ask_hero_images`'s, outside the draw.
    pub fn peek_image(
        &self,
        image: Option<&bobine::provider::ImageRef>,
        width: u32,
    ) -> Option<Arc<RenderImage>> {
        self.images
            .cache
            .peek(&Key::new(image?, width))
            .map(Arc::clone)
    }

    /// A failed image still on screen is asked again once its wait is over: the window is woken
    /// then, since an idle window does not redraw by itself.
    fn wake_for_image_retries(&mut self, cx: &mut Context<Self>) {
        let Some(at) = self.images.next_retry() else {
            return;
        };
        if self
            .image_wake
            .as_ref()
            .is_some_and(|(planned, _)| *planned <= at && *planned > Instant::now())
        {
            return;
        }
        let wait = at.saturating_duration_since(Instant::now());
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            this.update(cx, |shell, cx| {
                shell.image_wake = None;
                cx.notify();
            })
            .ok();
        });
        self.image_wake = Some((at, task));
    }

    /// Starts the fetches that may, up to the cap; each one that ends starts the next.
    fn fetch_images(&mut self, cx: &mut Context<Self>) {
        while let Some((key, image)) = self.images.next_fetch() {
            let svg = cx.svg_renderer();
            let width = key.width();
            self.ask(
                cx,
                move |server| {
                    let bytes = server.image(&image, width).ok()?;
                    let format = images::format(&bytes)?;
                    Arc::new(Image::from_bytes(format, bytes))
                        .to_image_data(svg)
                        .ok()
                },
                move |shell, decoded, cx| {
                    shell
                        .images
                        .fetched(&key, decoded.is_some(), Instant::now());
                    if let Some(decoded) = decoded {
                        let size = decoded.size(0);
                        let bytes = (size.width.0 * size.height.0 * 4).max(0) as usize;
                        for gone in shell.images.cache.insert(key, decoded, bytes) {
                            cx.drop_image(gone, None);
                        }
                    }
                    shell.notify_hero(cx);
                    shell.fetch_images(cx);
                    shell.wake_for_image_retries(cx);
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobine::provider::Film;

    #[test]
    fn the_home_and_its_lists_reload_after_a_playback() {
        assert_eq!(
            reload_after_playback(Screen::Home, 0, 0),
            Some(Reload::Home)
        );
        assert_eq!(
            reload_after_playback(Screen::HomeList, 0, 0),
            Some(Reload::List)
        );
        assert_eq!(
            reload_after_playback(Screen::Playlist, 1, 1),
            Some(Reload::Playlist)
        );
    }

    #[test]
    fn the_series_page_reloads_after_a_playback() {
        assert_eq!(
            reload_after_playback(Screen::SeriesDetail, 1, 2),
            Some(Reload::Series(1))
        );
        assert_eq!(
            reload_after_playback(Screen::FilmDetail, 2, 1),
            Some(Reload::Film(1))
        );
        assert_eq!(reload_after_playback(Screen::Player, 0, 0), None);
    }

    use crate::fake::{FILMS_LIBRARY, FakeServer};
    use bobine::provider::MediaServer;

    fn shown_film() -> DetailPage {
        let server = FakeServer::new(3, false);
        let id = ItemId(crate::fake::FILM.into());
        let mut page = DetailPage::new(id.clone(), 1);
        page.detail = Loadable::Ready(server.film(&id).unwrap());
        page
    }

    #[test]
    fn a_read_that_fails_on_a_film_page_that_is_showing_keeps_the_page() {
        let mut page = shown_film();
        page.load_failed("Offline");
        assert!(matches!(page.detail, Loadable::Ready(_)));
    }

    #[test]
    fn a_first_load_that_fails_shows_the_failure() {
        let mut page = DetailPage::new(ItemId(crate::fake::FILM.into()), 1);
        page.load_failed("Offline");
        assert!(matches!(page.detail, Loadable::Failed("Offline")));
    }

    /// A films page whose first page of a 1284-film library has arrived.
    fn films_page() -> LibraryPage<Film> {
        let server = FakeServer::new(1284, true);
        let mut page = LibraryPage::<Film>::default();
        page.grid.pages_to_ask(0..8, Instant::now());
        let first = server
            .films(&ItemId(FILMS_LIBRARY.into()), &page.tools.query(0))
            .unwrap();
        page.grid.loaded(page.grid.epoch(), 0, first);
        page
    }

    #[test]
    fn changing_a_filter_starts_the_list_over_from_the_top() {
        let mut page = films_page();
        page.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-900.)));
        page.jump = Some(40);
        assert!(page.change(|tools| tools.filters.genre = Some("Drame".into())));
        assert_eq!(page.tools.filters.genre.as_deref(), Some("Drame"));
        assert_eq!(page.grid.total, None);
        assert_eq!(page.jump, None);
    }

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    fn opacity(page: &LibraryPage<Film>, now: Instant) -> f32 {
        page.appear.value(now)
    }

    /// A films page showing its list, scrolled down, whose filter has just changed.
    fn films_page_changed(start: Instant) -> LibraryPage<Film> {
        let mut page = films_page();
        page.follow_list(start);
        page.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-900.)));
        let later = at(start, 1000);
        assert_eq!(opacity(&page, later), 1.);
        page.change(|tools| tools.filters.unwatched = true);
        page
    }

    #[test]
    fn the_old_list_fades_out_where_it_is_while_the_new_one_is_awaited() {
        let t = Instant::now();
        let mut page = films_page_changed(t);
        let t = at(t, 1000);
        page.follow_list(t);
        assert!(page.grid.has_stale());
        let halfway = opacity(&page, at(t, 60));
        assert!(0. < halfway && halfway < 1., "{halfway}");
        // Gone, and still waiting: the old list is kept under, invisible, not the blank.
        page.follow_list(at(t, 400));
        assert_eq!(opacity(&page, at(t, 400)), 0.);
        assert!(page.grid.has_stale());
        // It does not jump to the top before the new list is there.
        assert_eq!(page.scroll.0.borrow().base_handle.offset().y, px(-900.));
    }

    #[test]
    fn a_new_list_comes_in_once_the_old_one_is_gone() {
        let t = Instant::now();
        let mut page = films_page_changed(t);
        let t = at(t, 1000);
        page.follow_list(t);
        // The answer is quick: the old list is still fading out.
        page.loaded(page.grid.epoch(), 0, answer(&page, 0));
        page.follow_list(at(t, 10));
        assert!(
            page.grid.has_stale(),
            "swapped while the old list still showed"
        );
        assert_eq!(page.scroll.0.borrow().base_handle.offset().y, px(-900.));
        // Faded out: the new list takes its place, at the top, and fades in from nothing.
        page.follow_list(at(t, 400));
        assert!(!page.grid.has_stale());
        assert_eq!(page.scroll.0.borrow().base_handle.offset().y, px(0.));
        assert_eq!(opacity(&page, at(t, 400)), 0.);
        let coming = opacity(&page, at(t, 500));
        assert!(0. < coming && coming < 1., "{coming}");
        assert_eq!(opacity(&page, at(t, 900)), 1.);
        assert!(page.appear.moving(at(t, 500)));
    }

    #[test]
    fn a_new_list_that_arrives_late_comes_in_after_the_old_one_is_gone() {
        let t = Instant::now();
        let mut page = films_page_changed(t);
        let t = at(t, 1000);
        page.follow_list(t);
        page.follow_list(at(t, 500));
        page.loaded(page.grid.epoch(), 0, answer(&page, 0));
        page.follow_list(at(t, 600));
        assert!(!page.grid.has_stale());
        assert_eq!(opacity(&page, at(t, 600)), 0.);
        assert!(opacity(&page, at(t, 700)) > 0.);
    }

    #[test]
    fn a_new_list_given_up_during_the_fade_brings_the_old_one_back_with_its_error() {
        let t = Instant::now();
        let mut page = films_page_changed(t);
        let t = at(t, 1000);
        page.follow_list(t);
        page.follow_list(at(t, 400));
        assert_eq!(opacity(&page, at(t, 400)), 0.);
        // The first page of the new list fails until it is given up.
        for _ in 0..=crate::grid::MAX_RETRIES {
            page.grid.failed(page.grid.epoch(), 0, at(t, 400));
        }
        page.error = Some("Offline");
        assert!(page.gave_up());
        page.follow_list(at(t, 500));
        assert!(page.grid.has_stale());
        assert!(page.cells_visible(at(t, 600)));
        assert_eq!(opacity(&page, at(t, 1000)), 1.);
        // "Retry": the list goes away again while the new one is awaited.
        page.retry();
        page.follow_list(at(t, 1100));
        assert_eq!(opacity(&page, at(t, 1500)), 0.);
    }

    #[test]
    fn a_first_list_comes_in_from_nothing() {
        let t = Instant::now();
        let mut page = films_page();
        page.follow_list(t);
        assert!(!page.grid.has_stale());
        assert_eq!(opacity(&page, t), 0.);
        assert_eq!(opacity(&page, at(t, 1000)), 1.);
    }

    #[test]
    fn changing_the_order_starts_the_list_over_too() {
        let mut page = films_page();
        assert!(page.change(|tools| tools.sort = crate::library_tools::Sort::Title));
        assert_eq!(page.grid.total, None);
    }

    #[test]
    fn choosing_what_is_already_chosen_changes_nothing() {
        let mut page = films_page();
        assert!(!page.change(|tools| tools.filters.unwatched = false));
        assert_eq!(page.grid.total, Some(1284));
    }

    #[test]
    fn changing_the_tools_closes_the_open_menu() {
        let mut page = films_page();
        page.menus.toggle(Tool::Genre);
        page.change(|tools| tools.filters.unwatched = true);
        assert_eq!(page.menus.open(), None);
    }

    #[test]
    fn the_tools_and_the_list_stay_when_leaving_the_screen_and_coming_back() {
        let mut films = films_page();
        films.library = Some(Loadable::Ready(ItemId(FILMS_LIBRARY.into())));
        let mut series = LibraryPage::<bobine::provider::Series>::default();
        let mut nav = crate::navigation::Navigator::new();
        let known = |films: &LibraryPage<Film>, series: &LibraryPage<_>| {
            (films.library.is_some(), series.library.is_some())
        };

        // The tools are changed, the new list arrives and is scrolled.
        films.change(|tools| {
            tools.filters.genre = Some("Drame".into());
            tools.sort = Sort::Title;
        });
        let server = FakeServer::new(1284, true);
        let first = server
            .films(&ItemId(FILMS_LIBRARY.into()), &films.tools.query(0))
            .unwrap();
        films.grid.pages_to_ask(0..8, Instant::now());
        films.grid.loaded(films.grid.epoch(), 0, first);
        let (total, title) = (
            films.grid.total,
            films.grid.film(0).map(|f| f.title.clone()),
        );
        films
            .scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-900.)));
        let tools = films.tools.clone();

        // Away to the series, which load for the first time, then back to the films.
        nav.select(NavEntry::Series);
        let (f, s) = known(&films, &series);
        assert_eq!(
            arrival(nav.current(), f, s),
            Arrival::Library(LibraryKind::Series)
        );
        series.library = Some(Loadable::Loading);
        nav.select(NavEntry::Films);
        let (f, s) = known(&films, &series);
        assert_eq!(arrival(nav.current(), f, s), Arrival::Nothing);

        assert_eq!(films.tools, tools);
        assert!(films.tools.narrowed());
        assert_eq!(films.grid.total, total);
        assert_eq!(films.grid.film(0).map(|f| f.title.clone()), title);
        assert_eq!(films.scroll.0.borrow().base_handle.offset().y, px(-900.));
        // The other library's tools are its own.
        assert!(!series.tools.narrowed());
    }

    #[test]
    fn my_list_is_asked_again_each_time_it_shows() {
        assert_eq!(arrival(Screen::MyList, true, true), Arrival::MyList);
        assert_eq!(arrival(Screen::Home, true, true), Arrival::Home);
        assert_eq!(arrival(Screen::HomeList, true, true), Arrival::HomeList);
        assert_eq!(
            arrival(Screen::Collections, true, true),
            Arrival::Collections
        );
        assert_eq!(
            arrival(Screen::CollectionDetail, true, true),
            Arrival::Collection
        );
        assert_eq!(arrival(Screen::Library, true, true), Arrival::Nothing);
    }

    #[test]
    fn a_library_not_asked_yet_is_asked_when_its_screen_shows() {
        assert_eq!(
            arrival(Screen::Library, false, true),
            Arrival::Library(LibraryKind::Films)
        );
        assert_eq!(arrival(Screen::Library, true, false), Arrival::Nothing);
        assert_eq!(arrival(Screen::Home, true, true), Arrival::Home);
    }

    #[test]
    fn a_letter_found_takes_the_grid_to_its_title() {
        let mut page = films_page();
        page.found_letter(page.grid.epoch(), 412);
        assert_eq!(page.jump, Some(412));
    }

    #[test]
    fn a_letter_found_past_the_last_title_goes_to_the_last_one() {
        // "Z" with no title under it counts every title before it.
        let mut page = films_page();
        page.found_letter(page.grid.epoch(), 1284);
        assert_eq!(page.jump, Some(1283));
    }

    /// A page of a 1284-film library as the server would answer it for `page`'s tools.
    fn answer(page: &LibraryPage<Film>, number: u32) -> Page<Film> {
        FakeServer::new(1284, true)
            .films(&ItemId(FILMS_LIBRARY.into()), &page.tools.query(number))
            .unwrap()
    }

    #[test]
    fn a_letter_found_before_the_list_arrives_waits_for_it() {
        let mut page = films_page();
        page.change(|tools| tools.sort = Sort::Title);
        let epoch = page.grid.epoch();
        // The list is on its way: its size is not known yet.
        page.found_letter(epoch, 412);
        assert_eq!(page.jump, None);
        page.loaded(epoch, 0, answer(&page, 0));
        assert_eq!(page.jump, Some(412));
        assert_eq!(page.waiting_jump, None);
    }

    #[test]
    fn a_waiting_jump_past_the_last_title_goes_to_the_last_one() {
        let mut page = films_page();
        page.change(|tools| tools.sort = Sort::Title);
        let epoch = page.grid.epoch();
        page.found_letter(epoch, 5000);
        page.loaded(epoch, 0, answer(&page, 0));
        assert_eq!(page.jump, Some(1283));
    }

    #[test]
    fn a_waiting_jump_is_lost_when_the_list_changes_again() {
        let mut page = films_page();
        page.change(|tools| tools.sort = Sort::Title);
        let asked = page.grid.epoch();
        page.found_letter(asked, 412);
        page.change(|tools| tools.filters.unwatched = true);
        assert_eq!(page.waiting_jump, None);
        // The answer for the list before it does not land, and takes no jump with it.
        page.loaded(asked, 0, answer(&page, 0));
        page.loaded(page.grid.epoch(), 0, answer(&page, 0));
        assert_eq!(page.jump, None);
    }

    #[test]
    fn a_letter_found_for_a_list_since_changed_goes_nowhere() {
        let mut page = films_page();
        let asked = page.grid.epoch();
        page.change(|tools| tools.filters.unwatched = true);
        page.found_letter(asked, 412);
        assert_eq!(page.jump, None);
    }

    /// The page changed while it shows its list, the fade-out under way at `t`.
    fn fading(start: Instant) -> (LibraryPage<Film>, Instant) {
        let mut page = films_page_changed(start);
        let t = at(start, 1000);
        page.follow_list(t);
        (page, t)
    }

    #[test]
    fn a_first_page_that_fails_during_the_fade_is_asked_again_and_the_list_lands() {
        let (mut page, t) = fading(Instant::now());
        // The rows in view are the old list's: the new list's first page is what is wanted.
        assert_eq!(page.pages_wanted(0..40, t), [0]);
        let epoch = page.grid.epoch();
        page.grid.failed(epoch, 0, t);
        assert_eq!(page.pages_wanted(0..40, at(t, 1000)), Vec::<u32>::new());
        // The old list is gone by now, and the retry still comes.
        page.follow_list(at(t, 400));
        assert!(page.grid.has_stale());
        let retry = at(t, 1000) + crate::grid::RETRY_AFTER;
        assert_eq!(page.pages_wanted(0..40, retry), [0]);
        page.loaded(epoch, 0, answer(&page, 0));
        page.follow_list(retry);
        assert!(!page.grid.has_stale());
        assert_eq!(page.grid.total, Some(1027));
    }

    /// The first page fails every time it is asked, up to the last try, from `t`.
    fn give_up(page: &mut LibraryPage<Film>, t: Instant) -> Instant {
        let epoch = page.grid.epoch();
        let mut now = t;
        for _ in 0..=crate::grid::MAX_RETRIES {
            assert_eq!(page.pages_wanted(0..40, now), [0]);
            page.grid.failed(epoch, 0, now);
            now += crate::grid::RETRY_AFTER;
        }
        now
    }

    #[test]
    fn a_first_page_given_up_during_the_fade_shows_the_failure_and_can_be_tried_again() {
        let (mut page, t) = fading(Instant::now());
        assert!(!page.gave_up());
        let now = give_up(&mut page, t);
        page.follow_list(now);
        assert!(page.gave_up());
        assert_eq!(page.pages_wanted(0..40, now), Vec::<u32>::new());
        page.retry();
        assert!(!page.gave_up());
        assert_eq!(page.pages_wanted(0..40, now), [0]);
        // The server is back: the list lands in the old one's place.
        page.loaded(page.grid.epoch(), 0, answer(&page, 0));
        page.follow_list(now);
        assert!(!page.grid.has_stale());
        assert_eq!(page.grid.total, Some(1027));
    }

    #[test]
    fn a_first_page_that_still_has_tries_left_is_not_given_up() {
        let (mut page, t) = fading(Instant::now());
        let epoch = page.grid.epoch();
        for _ in 0..crate::grid::MAX_RETRIES {
            page.grid.failed(epoch, 0, t);
        }
        assert!(!page.gave_up());
    }

    #[test]
    fn the_rows_of_the_old_list_are_not_asked_for_the_new_one() {
        let (mut page, t) = fading(Instant::now());
        assert_eq!(page.pages_wanted(400..440, t), [0]);
        assert_eq!(page.pages_wanted(400..440, t), Vec::<u32>::new());
    }

    #[test]
    fn a_letter_found_during_the_fade_out_waits_for_the_swap() {
        let (mut page, t) = fading(Instant::now());
        let epoch = page.grid.epoch();
        page.loaded(epoch, 0, answer(&page, 0));
        page.found_letter(epoch, 412);
        // The old list is still leaving: the jump is kept, not taken and dropped.
        assert_eq!(page.take_jump(), None);
        page.follow_list(at(t, 400));
        assert!(!page.grid.has_stale());
        assert_eq!(page.take_jump(), Some(412));
    }

    #[test]
    fn the_old_list_has_no_cells_to_click_once_it_has_faded_out() {
        let (mut page, t) = fading(Instant::now());
        assert!(page.cells_visible(t));
        assert!(page.cells_visible(at(t, 60)));
        page.follow_list(at(t, 400));
        assert!(page.grid.has_stale());
        assert!(!page.cells_visible(at(t, 400)));
    }

    #[test]
    fn a_list_coming_in_has_its_cells() {
        let t = Instant::now();
        let mut page = films_page();
        page.follow_list(t);
        assert_eq!(opacity(&page, t), 0.);
        assert!(page.cells_visible(t));
    }

    #[test]
    fn the_header_has_no_count_while_the_new_list_is_awaited() {
        let (mut page, t) = fading(Instant::now());
        assert_eq!(page.header_count(), None);
        // Even when the new list has landed: the old one is still on screen.
        page.loaded(page.grid.epoch(), 0, answer(&page, 0));
        assert_eq!(page.header_count(), None);
        page.follow_list(at(t, 400));
        assert_eq!(page.header_count(), Some(1027));
    }

    #[test]
    fn the_header_has_the_count_when_the_list_stands() {
        assert_eq!(films_page().header_count(), Some(1284));
    }

    /// A shell started on a kept session, the fake server behind it.
    fn signed_in_window(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (FakeServer, gpui_kit::WindowHandle<Shell>) {
        let server = FakeServer::new(6, true);
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
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.navigator.current(), Screen::Home);
                assert!(shell.server.is_some());
            })
            .unwrap();
        (server, window)
    }

    /// A film playing on the shell, its "started" report sent, its "stopped" one slow to arrive.
    fn play_a_film(
        server: &FakeServer,
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::player::PlayerEvent;
        use crate::player::fake::FakePlayer;
        use crate::player::session::Session;
        use crate::reports::Reporting;
        use crate::video::fake::FakeDisplay;
        use bobine::provider::{MediaSourceId, PlaySessionId};

        server.state.lock().unwrap().stop_delay = Duration::from_millis(150);
        window
            .update(cx, |shell, window, cx| {
                let player = FakePlayer::default();
                player.queued.borrow_mut().push(PlayerEvent::Loaded);
                let mut session = Session::new(
                    Box::new(player),
                    Box::new(FakeDisplay::default()),
                    "Le Dernier Hiver".into(),
                    None,
                );
                let mut reporting = Reporting::start(
                    Arc::new(server.clone()),
                    PlaybackReport {
                        session: PlaySessionId("session-1".into()),
                        film: ItemId(crate::fake::FILM.into()),
                        source: MediaSourceId("source-4k".into()),
                        position: Duration::ZERO,
                        paused: false,
                        audio_track: None,
                        subtitle_track: None,
                        method: PlayMethod::DirectPlay,
                    },
                );
                let ended = session.update(Instant::now());
                reporting.tick(Instant::now(), &session, ended);
                shell.begin_playback(session, Some(reporting), window, cx);
                assert!(shell.playback.is_some());
            })
            .unwrap();
    }

    fn sign_out(window: &gpui_kit::WindowHandle<Shell>, cx: &mut gpui_kit::TestAppContext) {
        window
            .update(cx, |shell, window, cx| shell.sign_out(window, cx))
            .unwrap();
    }

    /// Signing out: the old session is out of reach at once, and the login waits, busy, for the
    /// server to answer.
    #[gpui_kit::test]
    fn signing_out_shows_the_login_at_once_with_the_address_in_its_field(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        sign_out(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.navigator.current(), Screen::Login);
                assert!(shell.server.is_none(), "the old session is still reachable");
                let form = shell.login.as_ref().expect("the login must show at once");
                assert_eq!(form.address.read(cx).text(), crate::fake::SERVER_ADDRESS);
                assert!(form.busy);
                assert!(form.signing_out);
                assert!(shell.home.featured().is_empty());
                assert_eq!(shell.images.cache.len(), 0);
                assert!(shell.details.is_empty() && shell.series_pages.is_empty());
                assert_eq!(shell.hero_view.read(cx).logos_len(), 0);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let form = shell.login.as_ref().unwrap();
                assert!(!form.busy && !form.signing_out, "the form must be usable");
                assert_eq!(form.error, None);
            })
            .unwrap();
        assert_eq!(server.state.lock().unwrap().record.logouts.len(), 1);
    }

    #[gpui_kit::test]
    fn two_sign_out_clicks_send_one_logout(cx: &mut gpui_kit::TestAppContext) {
        let (server, window) = signed_in_window(cx);
        sign_out(&window, cx);
        sign_out(&window, cx);
        cx.run_until_parked();
        sign_out(&window, cx);
        cx.run_until_parked();
        assert_eq!(server.state.lock().unwrap().record.logouts.len(), 1);
    }

    #[gpui_kit::test]
    fn a_playback_present_when_signing_out_is_stopped_before_the_logout(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        play_a_film(&server, &window, cx);
        sign_out(&window, cx);
        window
            .update(cx, |shell, _, _| {
                assert!(shell.playback.is_none(), "the film is still playing");
            })
            .unwrap();
        cx.run_until_parked();
        let state = server.state.lock().unwrap();
        assert!(matches!(
            state.record.reports.last(),
            Some(crate::fake::Report::Stopped(_))
        ));
        assert_eq!(state.record.logouts, [state.record.reports.len()]);
    }

    /// A film playing on the shell, its "My list" button following `film`.
    fn playing_with_list(
        server: &FakeServer,
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) {
        play_a_film(server, window, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.follow_list(ItemId(crate::fake::FILM.into()), cx);
            })
            .unwrap();
        cx.run_until_parked();
    }

    fn player_list_on(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> bool {
        window
            .update(cx, |shell, _, _| {
                let list = shell.playback.as_ref().unwrap().list.as_ref();
                list.expect("the player has a My list button").toggle.on()
            })
            .unwrap()
    }

    fn press_player_list(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) {
        window
            .update(cx, |shell, _, cx| shell.toggle_favourite(cx))
            .unwrap();
    }

    #[gpui_kit::test]
    fn the_favourite_button_adds_and_removes(cx: &mut gpui_kit::TestAppContext) {
        let (server, window) = signed_in_window(cx);
        let film = ItemId(crate::fake::FILM.into());
        playing_with_list(&server, &window, cx);
        assert!(!player_list_on(&window, cx));
        press_player_list(&window, cx);
        assert!(player_list_on(&window, cx), "shown at once");
        cx.run_until_parked();
        assert!(server.state.lock().unwrap().favourites.contains(&film));
        press_player_list(&window, cx);
        cx.run_until_parked();
        assert!(!player_list_on(&window, cx));
        assert!(!server.state.lock().unwrap().favourites.contains(&film));
    }

    #[gpui_kit::test]
    fn the_favourite_button_shows_a_title_already_in_the_list(cx: &mut gpui_kit::TestAppContext) {
        let (server, window) = signed_in_window(cx);
        let film = ItemId(crate::fake::FILM.into());
        server.state.lock().unwrap().favourites.push(film);
        playing_with_list(&server, &window, cx);
        assert!(player_list_on(&window, cx));
    }

    #[gpui_kit::test]
    fn a_failed_favourite_press_in_the_player_goes_back_and_says_so(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        playing_with_list(&server, &window, cx);
        server.state.lock().unwrap().favourite_fails = true;
        press_player_list(&window, cx);
        assert!(player_list_on(&window, cx));
        cx.run_until_parked();
        assert!(!player_list_on(&window, cx));
        window
            .update(cx, |shell, _, _| {
                let list = shell.playback.as_ref().unwrap().list.as_ref().unwrap();
                assert_eq!(list.error, Some(crate::favourite::FAILED));
            })
            .unwrap();
    }

    /// A film the server opened, playing on the shell with the default quality of the settings
    /// set to `default`; the transcodes the server was asked for come back.
    fn play_with_default_quality(
        default: crate::config::Quality,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (FakeServer, Vec<bobine::provider::TranscodeRequest>) {
        use crate::fallback::Fallback;
        use crate::player::PlayerEvent;
        use crate::player::fake::FakePlayer;
        use crate::player::session::Session;
        use crate::reports::Reporting;
        use crate::video::fake::FakeDisplay;

        let (server, window) = signed_in_window(cx);
        let id = ItemId(crate::fake::FILM.into());
        let detail = server.film(&id).unwrap();
        let choices = crate::film_page::Choices::new(&detail.versions);
        let plan =
            crate::film_page::plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let opened = server.play(&plan.film, &plan.source).unwrap();
        let player = FakePlayer::default();
        player
            .queued
            .borrow_mut()
            .extend([PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        window
            .update(cx, |shell, window, cx| {
                shell.settings.default_quality = default;
                let session = Session::new(
                    Box::new(player),
                    Box::new(FakeDisplay::default()),
                    "Le Dernier Hiver".into(),
                    None,
                );
                let reporting = Reporting::start(
                    Arc::new(server.clone()),
                    PlaybackReport {
                        session: opened.session.clone(),
                        film: opened.film.clone(),
                        source: opened.source.clone(),
                        position: plan.start,
                        paused: false,
                        audio_track: None,
                        subtitle_track: None,
                        method: PlayMethod::DirectPlay,
                    },
                );
                shell.begin_playback(session, Some(reporting), window, cx);
                let fallback = Fallback::new(Arc::new(server.clone()), plan.clone(), &opened);
                shell.arm_playback(fallback, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let transcodes = server.state.lock().unwrap().record.transcodes.clone();
        (server, transcodes)
    }

    #[gpui_kit::test]
    fn a_default_quality_cap_is_the_cap_a_film_starts_at(cx: &mut gpui_kit::TestAppContext) {
        let (_, transcodes) = play_with_default_quality(crate::config::Quality::Cap(10), cx);
        assert_eq!(transcodes.len(), 1);
        assert_eq!(transcodes[0].max_bitrate, Some(10_000_000));
    }

    #[gpui_kit::test]
    fn the_original_default_quality_leaves_the_film_playing_as_it_is(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_, transcodes) = play_with_default_quality(crate::config::Quality::Original, cx);
        assert!(transcodes.is_empty());
    }

    /// A quality chosen in the player: the film plays on until the server gives the conversion,
    /// which starts where the film was; the direct playback is told stopped there.
    #[gpui_kit::test]
    fn a_quality_cap_restarts_the_playback_at_the_same_position(cx: &mut gpui_kit::TestAppContext) {
        use crate::fake::Report;
        use crate::fallback::Fallback;
        use crate::player::PlayerEvent;
        use crate::player::fake::{Call, FakePlayer};
        use crate::player::session::Session;
        use crate::player::settings::Quality;
        use crate::reports::Reporting;
        use crate::video::fake::FakeDisplay;

        let (server, window) = signed_in_window(cx);
        let id = ItemId(crate::fake::FILM.into());
        let detail = server.film(&id).unwrap();
        let choices = crate::film_page::Choices::new(&detail.versions);
        let plan =
            crate::film_page::plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let opened = server.play(&plan.film, &plan.source).unwrap();
        let player = FakePlayer::default();
        let (calls, queued) = (player.calls.clone(), player.queued.clone());
        player.queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Position(4500.),
        ]);
        let cap = Quality::Cap(4_000_000);
        window
            .update(cx, |shell, window, cx| {
                let mut session = Session::new(
                    Box::new(player),
                    Box::new(FakeDisplay::default()),
                    "Le Dernier Hiver".into(),
                    None,
                );
                session.offer_quality();
                let mut reporting = Reporting::start(
                    Arc::new(server.clone()),
                    PlaybackReport {
                        session: opened.session.clone(),
                        film: opened.film.clone(),
                        source: opened.source.clone(),
                        position: plan.start,
                        paused: false,
                        audio_track: None,
                        subtitle_track: None,
                        method: PlayMethod::DirectPlay,
                    },
                );
                let ended = session.update(Instant::now());
                reporting.tick(Instant::now(), &session, ended);
                shell.begin_playback(session, Some(reporting), window, cx);
                let playback = shell.playback.as_mut().unwrap();
                playback.fallback = Some(Fallback::new(
                    Arc::new(server.clone()),
                    plan.clone(),
                    &opened,
                ));
                let request = playback.session.choose_quality(cap);
                shell.player_request(request, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let transcodes = server.state.lock().unwrap().record.transcodes.clone();
        assert_eq!(transcodes.len(), 1);
        assert_eq!(transcodes[0].start, Duration::from_secs(4500));
        assert_eq!(transcodes[0].max_bitrate, Some(4_000_000));
        let converted = server.state.lock().unwrap().converted.clone();
        let loads: Vec<(String, f64)> = calls
            .borrow()
            .iter()
            .filter_map(|call| match call {
                Call::Load { url, start, .. } => Some((url.clone(), *start)),
                _ => None,
            })
            .collect();
        assert_eq!(loads, [(converted, 4500.)]);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.playback.as_ref().unwrap().session.quality(), cap);
            })
            .unwrap();
        let stopped = (0..200).find_map(|_| {
            let found =
                server.state.lock().unwrap().record.reports.iter().find_map(
                    |report| match report {
                        Report::Stopped(report) => Some(report.clone()),
                        _ => None,
                    },
                );
            if found.is_none() {
                std::thread::sleep(Duration::from_millis(10));
            }
            found
        });
        let stopped = stopped.expect("the direct playback is told stopped");
        assert_eq!(stopped.method, PlayMethod::DirectPlay);
        assert_eq!(stopped.position, Duration::from_secs(4500));
        // The conversion opens: it is told started.
        queued.borrow_mut().push(PlayerEvent::Loaded);
        window
            .update(cx, |shell, window, cx| shell.tick(window, cx))
            .unwrap();
        let converted_started = |report: &Report| matches!(report, Report::Started(r) if r.method == PlayMethod::Transcode);
        let reports = (0..200)
            .map(|_| {
                let reports = server.state.lock().unwrap().record.reports.clone();
                if !reports.iter().any(converted_started) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                reports
            })
            .find(|reports| reports.iter().any(converted_started))
            .expect("the conversion is told started");
        let stopped = reports.iter().position(
            |report| matches!(report, Report::Stopped(r) if r.method == PlayMethod::DirectPlay),
        );
        let started = reports.iter().position(converted_started);
        assert!(
            stopped.is_some() && stopped < started,
            "the old stream stops before the new one starts: {reports:?}"
        );
    }

    /// A double click on the skip button is two presses of the button, never a double click on
    /// the picture.
    #[gpui_kit::test]
    fn a_double_click_on_skip_skips_once_and_keeps_the_window_as_it_is(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use crate::player::fake::{Call, FakePlayer};
        use crate::player::session::Session;
        use crate::player::{PlayerEvent, Seek};
        use crate::video::fake::FakeDisplay;
        use bobine::provider::{Segment, SegmentKind};
        use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseUpEvent};

        let (_server, window) = signed_in_window(cx);
        let player = FakePlayer::default();
        let calls = player.calls.clone();
        player.queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Duration(6000.),
            PlayerEvent::Position(10.),
        ]);
        window
            .update(cx, |shell, window, cx| {
                let mut session = Session::new(
                    Box::new(player),
                    Box::new(FakeDisplay::default()),
                    "Le Dernier Hiver".into(),
                    None,
                );
                session.set_segments(vec![Segment {
                    kind: SegmentKind::Intro,
                    start: Duration::ZERO,
                    end: Duration::from_secs(90),
                }]);
                session.update(Instant::now());
                assert!(session.skip_button().is_some());
                shell.begin_playback(session, None, window, cx);
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        let at = visual
            .debug_bounds("player-skip")
            .expect("the skip button")
            .center();
        for click_count in [1, 2] {
            let (button, modifiers) = (MouseButton::Left, Modifiers::none());
            visual.simulate_event(MouseDownEvent {
                position: at,
                modifiers,
                button,
                click_count,
                first_mouse: false,
            });
            visual.simulate_event(MouseUpEvent {
                position: at,
                modifiers,
                button,
                click_count,
            });
        }
        let seeks = calls
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Seek(Seek::To(_))))
            .count();
        assert_eq!(seeks, 1);
        let fullscreen = window
            .update(cx, |_, window, _| window.is_fullscreen())
            .unwrap();
        assert!(!fullscreen);
    }

    /// The server answered "play" just before the sign out: the film must not start behind the
    /// login.
    #[gpui_kit::test]
    fn a_film_opened_during_the_sign_out_never_starts(cx: &mut gpui_kit::TestAppContext) {
        let (server, window) = signed_in_window(cx);
        let id = ItemId(crate::fake::FILM.into());
        let detail = server.film(&id).unwrap();
        let choices = crate::film_page::Choices::new(&detail.versions);
        let plan =
            crate::film_page::plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let opened = server.play(&plan.film, &plan.source);
        sign_out(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                let playing = super::super::Film {
                    title: plan.title.clone(),
                    subtitle: None,
                    list: None,
                };
                shell.start_playback(Origin::Home, plan, opened, playing, None, window, cx);
                assert!(shell.playback.is_none());
                assert_eq!(shell.navigator.current(), Screen::Login);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_sign_out_that_could_not_empty_the_keyring_says_so_once_the_form_is_usable(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        server.state.lock().unwrap().logout_error = Some(ProviderError::Keyring(String::new()));
        sign_out(&window, cx);
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let form = shell.login.as_ref().unwrap();
                assert!(!form.busy);
                assert_eq!(
                    form.error,
                    Some("The saved token could not be removed from the system keyring.")
                );
            })
            .unwrap();
    }

    /// The answer of a sign out comes after the form was built again, and something is typed in
    /// it: it stays as it is.
    #[gpui_kit::test]
    fn a_late_sign_out_answer_never_wipes_the_form_being_typed_in(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        server.state.lock().unwrap().logout_error = Some(ProviderError::Storage(String::new()));
        sign_out(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.show_login(None, cx);
                let form = shell.login.as_ref().unwrap();
                form.user
                    .update(cx, |field, cx| field.set_text("camille", cx));
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let form = shell.login.as_ref().unwrap();
                assert_eq!(form.user.read(cx).text(), "camille");
                assert_eq!(form.error, None);
            })
            .unwrap();
    }

    /// The user signed in again before the answer came: that session stays.
    #[gpui_kit::test]
    fn a_late_sign_out_answer_never_kicks_out_a_session_signed_in_meanwhile(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        sign_out(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.signed_in(Arc::new(server.clone()), cx);
                shell.signed_out(Some("The account file cannot be written."), cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.server.is_some());
                assert!(shell.login.is_none());
                assert_eq!(shell.navigator.current(), Screen::Home);
            })
            .unwrap();
    }

    /// The window as last drawn: where things are, and what the mouse reaches.
    fn drawn(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> gpui_kit::VisualTestContext {
        let visual = gpui_kit::VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        visual
    }

    fn current(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    #[gpui_kit::test]
    fn the_pill_bar_and_ctrl_k_are_present_on_a_film_page_a_series_page_a_person_page_and_a_see_all_list(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_server, window) = signed_in_window(cx);
        let pages: [fn(&mut Shell, &mut Context<Shell>); 4] = [
            |shell, cx| shell.open_film(ItemId(crate::fake::FILM.into()), cx),
            |shell, cx| shell.open_series(ItemId(crate::fake::SERIES.into()), cx),
            |shell, cx| shell.open_person(crate::fake::people()[0].clone(), cx),
            |shell, cx| shell.open_list(ListKind::Resume, None, cx),
        ];
        for open in pages {
            window
                .update(cx, |shell, _, cx| {
                    shell.select(NavEntry::Home, cx);
                    open(shell, cx);
                })
                .unwrap();
            let mut visual = drawn(&window, cx);
            let screen = current(&window, cx);
            assert!(visual.debug_bounds("pill-bar").is_some(), "{screen:?}");
            visual.simulate_keystrokes("ctrl-k");
            let opened = window
                .update(cx, |shell, _, _| shell.search_is_open())
                .unwrap();
            assert!(opened, "Ctrl+K on {screen:?}");
            visual.simulate_keystrokes("escape");
            assert_eq!(current(&window, cx), screen);
        }
    }

    #[gpui_kit::test]
    fn the_mouse_back_button_goes_back_and_the_forward_button_goes_forward(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{Modifiers, MouseButton, NavigationDirection};

        let (_server, window) = signed_in_window(cx);
        let film = ItemId(crate::fake::FILM.into());
        window
            .update(cx, |shell, _, cx| shell.open_film(film.clone(), cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        // Over the bar, which lets no click through to the page: the side buttons still work.
        let at = visual.debug_bounds("pill-bar").expect("the bar").center();
        let mut press = |direction, cx: &mut gpui_kit::TestAppContext| {
            let button = MouseButton::Navigate(direction);
            visual.simulate_mouse_down(at, button, Modifiers::none());
            visual.simulate_mouse_up(at, button, Modifiers::none());
            drawn(&window, cx);
        };
        press(NavigationDirection::Back, cx);
        assert_eq!(current(&window, cx), Screen::Home);
        press(NavigationDirection::Forward, cx);
        assert_eq!(current(&window, cx), Screen::FilmDetail);
        let shown = window
            .update(cx, |shell, _, _| {
                shell
                    .details
                    .iter()
                    .map(|page| page.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap();
        assert_eq!(shown, [film]);
        // Nothing further to go forward to.
        press(NavigationDirection::Forward, cx);
        assert_eq!(current(&window, cx), Screen::FilmDetail);
        // A page opened after going back leaves nothing to go forward to.
        press(NavigationDirection::Back, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(crate::fake::SERIES.into()), cx)
            })
            .unwrap();
        press(NavigationDirection::Forward, cx);
        assert_eq!(current(&window, cx), Screen::SeriesDetail);
        press(NavigationDirection::Back, cx);
        assert_eq!(current(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn a_see_all_list_uses_the_library_grid_layout(cx: &mut gpui_kit::TestAppContext) {
        let (_server, window) = signed_in_window(cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Films, cx))
            .unwrap();
        // Where things are down the page and how big: the page itself may still be sliding in.
        let layout = |visual: &mut gpui_kit::VisualTestContext| {
            ["grid-heading", "grid-rows"].map(|part| {
                let bounds = visual.debug_bounds(part).expect(part);
                (bounds.origin.y, bounds.size)
            })
        };
        let library = layout(&mut drawn(&window, cx));
        let films = ListKind::Latest(
            ItemId(crate::fake::FILMS_LIBRARY.into()),
            LibraryKind::Films,
        );
        window
            .update(cx, |shell, _, cx| {
                shell.select(NavEntry::Home, cx);
                shell.open_list(films, Some("Films".into()), cx);
            })
            .unwrap();
        assert_eq!(layout(&mut drawn(&window, cx)), library);
    }

    #[gpui_kit::test]
    fn an_empty_list_shows_its_reason_and_a_row_of_suggestions(cx: &mut gpui_kit::TestAppContext) {
        let (server, window) = signed_in_window(cx);
        // Everything started is finished: the list behind the Home's row is empty by now.
        {
            let mut state = server.state.lock().unwrap();
            state.films.iter_mut().for_each(|film| film.resume = None);
            let episodes = state.episodes.iter_mut();
            episodes.for_each(|episode| episode.item.resume = None);
        }
        window
            .update(cx, |shell, _, cx| {
                shell.open_list(ListKind::Resume, None, cx)
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("empty-reason").is_some());
        assert!(visual.debug_bounds("suggestions").is_some());
        // What the Home still holds that was started is the row to watch instead; the list that
        // is empty is never offered back.
        assert!(visual.debug_bounds("suggested: Recently added").is_some());
        assert!(
            visual
                .debug_bounds("suggested: Continue watching")
                .is_none()
        );
    }

    #[gpui_kit::test]
    fn empty_favorites_and_an_empty_collections_page_show_their_reason_and_the_suggestion_row(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window) = signed_in_window(cx);
        server.state.lock().unwrap().favourites.clear();
        window
            .update(cx, |shell, _, cx| {
                shell.select(NavEntry::MyList, cx);
                shell.open_favorites(cx);
            })
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("grid-heading").is_some());
        assert!(visual.debug_bounds("empty-reason").is_some());
        assert!(
            visual
                .debug_bounds("suggested: Continue watching")
                .is_some()
        );

        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Collections, cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                shell.collections.cards = Some(Loadable::Ready(Vec::new()));
                cx.notify();
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("grid-heading").is_some());
        assert!(visual.debug_bounds("empty-reason").is_some());
        assert!(
            visual
                .debug_bounds("suggested: Continue watching")
                .is_some()
        );
    }

    /// A side button pressed at `at`, then released.
    fn side_press(
        window: &gpui_kit::WindowHandle<Shell>,
        direction: gpui_kit::NavigationDirection,
        at: Point<Pixels>,
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let button = gpui_kit::MouseButton::Navigate(direction);
        let mut visual = drawn(window, cx);
        visual.simulate_mouse_down(at, button, gpui_kit::Modifiers::none());
        visual.simulate_mouse_up(at, button, gpui_kit::Modifiers::none());
        drawn(window, cx);
    }

    #[gpui_kit::test]
    fn the_mouse_back_button_cancels_the_next_episode_card_and_stays(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::NavigationDirection::Back;

        let (_server, window) = signed_in_window(cx);
        let id = ItemId(crate::fake::SERIES.into());
        window
            .update(cx, |shell, _, cx| shell.open_series(id.clone(), cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let episode = crate::fake::episodes().remove(0);
                shell
                    .up_next
                    .start(Next::Episode(episode), (0, id), Instant::now());
            })
            .unwrap();
        let at = drawn(&window, cx)
            .debug_bounds("pill-bar")
            .unwrap()
            .center();
        side_press(&window, Back, at, cx);
        window
            .update(cx, |shell, _, _| {
                assert!(shell.up_next.episode().is_none(), "the card is still there");
            })
            .unwrap();
        assert_eq!(current(&window, cx), Screen::SeriesDetail);
        // Pressed again with nothing left to close, it goes back.
        side_press(&window, Back, at, cx);
        assert_eq!(current(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn the_mouse_back_button_closes_the_account_menu_and_stays(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::NavigationDirection::Back;

        let (_server, window) = signed_in_window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_film(ItemId(crate::fake::FILM.into()), cx);
                shell.toggle_account_menu(cx);
            })
            .unwrap();
        let at = drawn(&window, cx)
            .debug_bounds("pill-bar")
            .unwrap()
            .center();
        side_press(&window, Back, at, cx);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.account_menu.open(), None, "the menu is still open");
            })
            .unwrap();
        assert_eq!(current(&window, cx), Screen::FilmDetail);
        side_press(&window, Back, at, cx);
        assert_eq!(current(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn forward_reopens_a_series_on_the_season_it_was_left_on(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::NavigationDirection::{Back, Forward};

        let (_server, window) = signed_in_window(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(crate::fake::SERIES.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
        // The page opens on the season of the next episode: the first is another.
        let season = |cx: &mut gpui_kit::TestAppContext| {
            window
                .update(cx, |shell, _, _| {
                    shell.series_pages.last().map(|p| p.season)
                })
                .unwrap()
        };
        assert_ne!(season(cx), Some(0));
        window
            .update(cx, |shell, _, cx| shell.select_season(0, cx))
            .unwrap();
        let at = drawn(&window, cx)
            .debug_bounds("pill-bar")
            .unwrap()
            .center();
        side_press(&window, Back, at, cx);
        side_press(&window, Forward, at, cx);
        cx.run_until_parked();
        assert_eq!(current(&window, cx), Screen::SeriesDetail);
        assert_eq!(season(cx), Some(0));
    }

    /// Gone back from a film page once: the forward button has a page to open.
    fn with_a_page_ahead(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) {
        window
            .update(cx, |shell, _, cx| {
                shell.open_film(ItemId(crate::fake::FILM.into()), cx);
                shell.go_back(cx);
                assert_eq!(shell.forward.len(), 1);
            })
            .unwrap();
    }

    fn pages_ahead(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> usize {
        window
            .update(cx, |shell, _, _| shell.forward.len())
            .unwrap()
    }

    #[gpui_kit::test]
    fn the_side_buttons_are_ignored_in_the_player_and_on_the_login(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::NavigationDirection::{Back, Forward};

        let (server, window) = signed_in_window(cx);
        play_a_film(&server, &window, cx);
        for direction in [Back, Forward] {
            window
                .update(cx, |shell, window, cx| {
                    shell.navigate(direction, window, cx)
                })
                .unwrap();
            window
                .update(cx, |shell, _, _| {
                    assert_eq!(shell.navigator.current(), Screen::Player);
                    assert!(shell.playback.is_some(), "a stray press stopped the film");
                })
                .unwrap();
        }
        sign_out(&window, cx);
        cx.run_until_parked();
        for direction in [Back, Forward] {
            window
                .update(cx, |shell, window, cx| {
                    shell.navigate(direction, window, cx)
                })
                .unwrap();
            assert_eq!(current(&window, cx), Screen::Login);
        }
    }

    #[gpui_kit::test]
    fn with_the_palette_open_back_closes_it_and_forward_does_nothing(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::NavigationDirection::{Back, Forward};

        let (_server, window) = signed_in_window(cx);
        with_a_page_ahead(&window, cx);
        window
            .update(cx, |shell, window, cx| shell.open_search(window, cx))
            .unwrap();
        window
            .update(cx, |shell, window, cx| shell.navigate(Forward, window, cx))
            .unwrap();
        window
            .update(cx, |shell, _, _| {
                assert!(shell.search_is_open());
                assert_eq!(shell.navigator.current(), Screen::Home);
            })
            .unwrap();
        assert_eq!(pages_ahead(&window, cx), 1);
        window
            .update(cx, |shell, window, cx| shell.navigate(Back, window, cx))
            .unwrap();
        window
            .update(cx, |shell, _, _| assert!(!shell.search_is_open()))
            .unwrap();
        assert_eq!(current(&window, cx), Screen::Home);
        assert_eq!(pages_ahead(&window, cx), 1);
    }

    #[gpui_kit::test]
    fn the_forward_stack_is_cleared_by_a_bar_entry(cx: &mut gpui_kit::TestAppContext) {
        let (_server, window) = signed_in_window(cx);
        with_a_page_ahead(&window, cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Films, cx))
            .unwrap();
        assert_eq!(pages_ahead(&window, cx), 0);
    }

    #[gpui_kit::test]
    fn the_forward_stack_is_cleared_by_starting_a_playback(cx: &mut gpui_kit::TestAppContext) {
        let (server, window) = signed_in_window(cx);
        with_a_page_ahead(&window, cx);
        play_a_film(&server, &window, cx);
        assert_eq!(pages_ahead(&window, cx), 0);
    }

    #[gpui_kit::test]
    fn the_forward_stack_is_cleared_by_signing_out(cx: &mut gpui_kit::TestAppContext) {
        let (_server, window) = signed_in_window(cx);
        with_a_page_ahead(&window, cx);
        sign_out(&window, cx);
        assert_eq!(pages_ahead(&window, cx), 0);
    }

    /// The window of a page opened by `open`, drawn in a window tall enough for all of it.
    fn drawn_page(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut gpui_kit::TestAppContext,
        open: impl FnOnce(&mut Shell, &mut Context<Shell>),
    ) -> gpui_kit::VisualTestContext {
        window.update(cx, |shell, _, cx| open(shell, cx)).unwrap();
        let visual = drawn(window, cx);
        visual.simulate_resize(gpui_kit::size(px(1600.), px(3000.)));
        visual.run_until_parked();
        visual
    }

    fn click_on(visual: &mut gpui_kit::VisualTestContext, what: &'static str) {
        let at = visual
            .debug_bounds(what)
            .unwrap_or_else(|| panic!("{what} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
        visual.simulate_click(at, gpui_kit::Modifiers::none());
        visual.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_click_on_a_cast_member_opens_their_page(cx: &mut gpui_kit::TestAppContext) {
        let pages: [fn(&mut Shell, &mut Context<Shell>); 2] = [
            |shell, cx| shell.open_film(ItemId(crate::fake::FILM.into()), cx),
            |shell, cx| shell.open_series(ItemId(crate::fake::SERIES.into()), cx),
        ];
        for open in pages {
            let (server, window) = signed_in_window(cx);
            let mut visual = drawn_page(&window, cx, open);
            let screen = current(&window, cx);
            click_on(&mut visual, "cast-member 1");
            assert_eq!(current(&window, cx), Screen::Person, "from {screen:?}");
            let cast = match screen {
                Screen::FilmDetail => server.film(&ItemId(crate::fake::FILM.into())).unwrap().cast,
                _ => {
                    server
                        .series_detail(&ItemId(crate::fake::SERIES.into()))
                        .unwrap()
                        .cast
                }
            };
            let opened = window
                .update(cx, |shell, _, _| {
                    let page = shell.people.last().expect("the person page");
                    (page.id.clone(), page.name.clone())
                })
                .unwrap();
            assert_eq!(opened, (cast[1].id.clone(), cast[1].name.clone()));
        }
    }

    /// The comma between two genres is drawn beside the link, not in it: the highlight and the
    /// click stop at the word.
    #[gpui_kit::test]
    fn the_comma_after_a_genre_is_outside_its_link(cx: &mut gpui_kit::TestAppContext) {
        let (_server, window) = signed_in_window(cx);
        let mut visual = drawn_page(&window, cx, |shell, cx| {
            shell.open_film(ItemId(crate::fake::FILM.into()), cx)
        });
        // The film's genres are "Drame, Aventure".
        let link = visual.debug_bounds("genre Drame").expect("the first genre");
        let comma = visual
            .debug_bounds("genre-separator Drame")
            .expect("the comma after the first genre");
        assert!(comma.left() >= link.right(), "{comma:?} inside {link:?}");
        assert!(
            visual.debug_bounds("genre-separator Aventure").is_none(),
            "a comma after the last genre"
        );
    }

    #[gpui_kit::test]
    fn a_click_on_a_genre_of_a_film_opens_the_films_library_filtered_on_it(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_server, window) = signed_in_window(cx);
        let mut visual = drawn_page(&window, cx, |shell, cx| {
            shell.open_film(ItemId(crate::fake::FILM.into()), cx)
        });
        // The film's genres are "Drame, Aventure".
        click_on(&mut visual, "genre Aventure");
        assert_eq!(current(&window, cx), Screen::Library);
        let (films, series) = window
            .update(cx, |shell, _, _| {
                (
                    shell.library.tools.filters.genre.clone(),
                    shell.series.tools.filters.genre.clone(),
                )
            })
            .unwrap();
        assert_eq!((films.as_deref(), series), (Some("Aventure"), None));
    }

    /// Escape and the mouse's back button return to the page the genre was clicked on, not to
    /// the Home; the library's pill stays the lit one while it shows.
    #[gpui_kit::test]
    fn back_from_a_genre_returns_to_the_film_page_it_came_from(cx: &mut gpui_kit::TestAppContext) {
        let (_server, window) = signed_in_window(cx);
        assert_eq!(current(&window, cx), Screen::Home);
        let mut visual = drawn_page(&window, cx, |shell, cx| {
            shell.open_film(ItemId(crate::fake::FILM.into()), cx)
        });
        click_on(&mut visual, "genre Aventure");
        assert_eq!(current(&window, cx), Screen::Library);
        let section = window
            .update(cx, |shell, _, _| shell.navigator.section())
            .unwrap();
        assert_eq!(section, Screen::Library, "the pill of the library is lit");

        window.update(cx, |shell, _, cx| shell.escape(cx)).unwrap();
        assert_eq!(current(&window, cx), Screen::FilmDetail);
        let film = window
            .update(cx, |shell, _, _| {
                shell.details.last().map(|page| page.id.clone())
            })
            .unwrap();
        assert_eq!(film, Some(ItemId(crate::fake::FILM.into())));
    }

    #[gpui_kit::test]
    fn a_click_on_a_genre_of_a_series_opens_the_series_library_filtered_on_it(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_server, window) = signed_in_window(cx);
        let mut visual = drawn_page(&window, cx, |shell, cx| {
            shell.open_series(ItemId(crate::fake::SERIES.into()), cx)
        });
        // The series' genres are "Science-fiction, Drame".
        click_on(&mut visual, "genre Drame");
        assert_eq!(current(&window, cx), Screen::Series);
        let (films, series) = window
            .update(cx, |shell, _, _| {
                (
                    shell.library.tools.filters.genre.clone(),
                    shell.series.tools.filters.genre.clone(),
                )
            })
            .unwrap();
        assert_eq!((films, series.as_deref()), (None, Some("Drame")));
    }

    /// The library was seen before: its list is asked again under the genre, as the genre menu
    /// does, not kept as it was.
    #[gpui_kit::test]
    fn a_genre_clicked_over_a_library_already_loaded_filters_its_list(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_server, window) = signed_in_window(cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Films, cx))
            .unwrap();
        cx.run_until_parked();
        let before = window
            .update(cx, |shell, _, _| shell.library.grid.epoch())
            .unwrap();
        let mut visual = drawn_page(&window, cx, |shell, cx| {
            shell.open_film(ItemId(crate::fake::FILM.into()), cx)
        });
        click_on(&mut visual, "genre Aventure");
        let (genre, epoch) = window
            .update(cx, |shell, _, _| {
                (
                    shell.library.tools.filters.genre.clone(),
                    shell.library.grid.epoch(),
                )
            })
            .unwrap();
        assert_eq!(genre.as_deref(), Some("Aventure"));
        assert_ne!(epoch, before, "the list was not asked again");
    }

    /// The series page of the tests, on its second season, where the series is begun: episode
    /// 4 (the row at index 3) is stopped at 30 min. Playbacks are refused once asked, so that
    /// no player starts.
    fn series_page_of_tests(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        FakeServer,
        gpui_kit::WindowHandle<Shell>,
        gpui_kit::VisualTestContext,
    ) {
        let (server, window) = signed_in_window(cx);
        server.state.lock().unwrap().play_error =
            Some(ProviderError::Unreachable("refused".into()));
        let visual = drawn_page(&window, cx, |shell, cx| {
            shell.open_series(ItemId(crate::fake::SERIES.into()), cx)
        });
        (server, window, visual)
    }

    fn episode_still(index: usize) -> gpui_kit::ElementId {
        gpui_kit::ElementId::from(("episode-still", index))
    }

    fn pointer_over(
        visual: &mut gpui_kit::VisualTestContext,
        what: &str,
        id: &gpui_kit::ElementId,
    ) {
        let selector: &'static str = Box::leak(format!("{what} {id}").into_boxed_str());
        let at = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
        visual.run_until_parked();
    }

    #[gpui_kit::test]
    fn hovering_an_episode_shows_the_play_button_over_its_picture(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (_server, _window, mut visual) = series_page_of_tests(cx);
        let id = episode_still(3);
        let play: &'static str = Box::leak(format!("card-play {id}").into_boxed_str());
        assert!(
            visual.debug_bounds(play).is_none(),
            "drawn before the pointer"
        );
        pointer_over(&mut visual, "card-hover", &id);
        let play = visual.debug_bounds(play).expect("the play button");
        let still = visual
            .debug_bounds(Box::leak(format!("card-hover {id}").into_boxed_str()))
            .expect("the still");
        assert!(
            still.contains(&play.center()),
            "the button is over the picture"
        );
    }

    #[gpui_kit::test]
    fn a_click_on_the_play_button_of_an_episode_plays_it_from_where_it_stopped(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, window, mut visual) = series_page_of_tests(cx);
        let id = episode_still(3);
        pointer_over(&mut visual, "card-hover", &id);
        let play: &'static str = Box::leak(format!("card-play {id}").into_boxed_str());
        click_on(&mut visual, play);
        let asked = server.state.lock().unwrap().record.plays.clone();
        let stopped = {
            let state = server.state.lock().unwrap();
            let wanted = ItemId("series-0-s2-e4".into());
            let found = state.episodes.iter().find(|e| e.item.id == wanted);
            found.expect("episode 4 of season 2").clone()
        };
        assert!(stopped.item.resume.is_some(), "the episode was started");
        assert_eq!(
            asked
                .iter()
                .map(|(film, _)| film.clone())
                .collect::<Vec<_>>(),
            [stopped.item.id],
            "once, and the episode that was clicked"
        );
        assert_eq!(current(&window, cx), Screen::SeriesDetail);
        let planned = window
            .update(cx, |shell, _, _| shell.planned.clone())
            .unwrap();
        assert_eq!(
            planned,
            [(
                ItemId("series-0-s2-e4".into()),
                std::time::Duration::from_secs(30 * 60)
            )],
            "the player starts where the episode stopped"
        );
    }

    #[gpui_kit::test]
    fn a_click_anywhere_on_an_episode_row_plays_it(cx: &mut gpui_kit::TestAppContext) {
        let (server, _window, mut visual) = series_page_of_tests(cx);
        // The title of the row, away from the picture and from the check.
        let still = visual
            .debug_bounds(Box::leak(
                format!("card-hover {}", episode_still(3)).into_boxed_str(),
            ))
            .expect("the still");
        let at = gpui_kit::point(still.right() + gpui_kit::px(120.), still.center().y);
        visual.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
        visual.simulate_click(at, gpui_kit::Modifiers::none());
        visual.run_until_parked();
        let asked = server.state.lock().unwrap().record.plays.clone();
        assert_eq!(asked.len(), 1, "{asked:?}");
    }

    #[gpui_kit::test]
    fn a_click_on_the_picture_of_an_episode_beside_the_button_plays_it_once(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (server, _window, mut visual) = series_page_of_tests(cx);
        let id = episode_still(3);
        pointer_over(&mut visual, "card-hover", &id);
        let still = visual
            .debug_bounds(Box::leak(format!("card-hover {id}").into_boxed_str()))
            .expect("the still");
        let at = gpui_kit::point(
            still.left() + gpui_kit::px(10.),
            still.top() + gpui_kit::px(10.),
        );
        visual.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
        visual.simulate_click(at, gpui_kit::Modifiers::none());
        visual.run_until_parked();
        let asked = server.state.lock().unwrap().record.plays.clone();
        assert_eq!(asked.len(), 1, "{asked:?}");
    }
}
