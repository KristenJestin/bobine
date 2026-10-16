//! What the screens show and how it comes from the server. Every call to the server runs on the
//! background executor; its answer comes back to the window's thread, which updates the page.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bobine::provider::{
    Collection, Connector, Episode, FilmDetail, FilmQuery, FilterChoices, ItemId, LibraryKind,
    MediaServer, MediaSource, Page, PlayMethod, Playback as Opened, PlaybackReport, ProviderError,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, Context, Entity, Image, Pixels, Point, RenderImage, ScrollHandle, ScrollWheelEvent,
    Subscription, Task, TouchPhase, UniformListScrollHandle, Window, point, px,
};

use super::{Film as Playing, HomePage, Shell};
use crate::fallback::Fallback;
use crate::favourite::{self, ListPage, Toggle};
use crate::film_page::{self, Choices, From, Plan};
use crate::grid::{Grid, Listed, RETRY_AFTER};
use crate::images::{self, Key};
use crate::input::TextInput;
use crate::library_tools::{Sort, Tools};
use crate::menu::Menus;
use crate::motion::{self, Fade, Glide, Presence};
use crate::navigation::{NavEntry, Screen};
use crate::player::libmpv::Libmpv;
use crate::reports::Reporting;
use crate::scroll::{self, Move, Phase, Wheel};
use crate::start::{self, Start};

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
            self.appear.set(0., now);
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

    /// "Réessayer": the first page is asked anew, as many times as the first time.
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
    /// The user's list is asked again each time it shows: a title put in it elsewhere shows.
    MyList,
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
}

/// What is asked again after a playback, on `screen`, the one back to: the Home or one of its
/// lists, else the series page on top, else the film page on top, if any.
pub fn reload_after_playback(screen: Screen, films: usize, series: usize) -> Option<Reload> {
    match screen {
        Screen::Home => return Some(Reload::Home),
        Screen::HomeList => return Some(Reload::List),
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
            Chip::Audio => choices.audio = value,
            Chip::Subtitle => choices.subtitle = value,
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
    /// "Ma liste".
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
        let address = cx.new(|cx| TextInput::new("https://media.exemple.org", false, cx));
        let user = cx.new(|cx| TextInput::new("Nom d'utilisateur", false, cx));
        let password = cx.new(|cx| TextInput::new("Mot de passe", true, cx));
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
            form.error = Some("Le fichier du compte ne peut pas être écrit.");
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
        self.home = HomePage::default();
        self.library = LibraryPage::default();
        self.series = LibraryPage::default();
        self.collections = Default::default();
        self.my_list = Default::default();
        self.details.clear();
        self.series_pages.clear();
        self.people.clear();
        self.list = None;
        self.collection_pages.clear();
        self.search = None;
        // The atlas holds the old images until they are dropped from it.
        for gone in self.images.cache.drain() {
            cx.drop_image(gone, None);
        }
        self.images = images::Images::default();
        self.navigator.switch_root(Screen::Home);
        self.load_home(cx);
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
        self.details.clear();
        self.series_pages.clear();
        self.up_next.cancel();
        self.people.clear();
        self.list = None;
        self.collection_pages.clear();
        self.arrived(cx);
        cx.notify();
    }

    /// Escape or the back link.
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        match self.navigator.current() {
            Screen::FilmDetail => drop(self.details.pop()),
            Screen::SeriesDetail => drop(self.series_pages.pop()),
            Screen::Person => drop(self.people.pop()),
            Screen::HomeList => self.list = None,
            Screen::CollectionDetail => drop(self.collection_pages.pop()),
            _ => {}
        }
        self.up_next.cancel();
        if !self.navigator.back() {
            return false;
        }
        self.arrived(cx);
        cx.notify();
        true
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
            Arrival::MyList => self.load_my_list(cx),
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
                        Loadable::Failed("Aucune bibliothèque de séries sur ce serveur.")
                    }
                    Ok(None) => Loadable::Failed("Aucune bibliothèque de films sur ce serveur."),
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

    fn ask_pages<T: Listed + Send + 'static>(
        &mut self,
        items: std::ops::Range<usize>,
        page_of: fn(&mut Shell) -> &mut LibraryPage<T>,
        fetch: Fetch<T>,
        cx: &mut Context<Self>,
    ) {
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
                    Ok(items) => page_of(shell).loaded(epoch, page, items),
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

    /// "Réessayer", when the first page of the list was given up: it is asked again.
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
        self.ask(
            cx,
            move |server| server.collection_of(&asked),
            move |shell, collection, _| {
                if let Some(page) = shell.detail_page(depth, &id) {
                    // A failed lookup hides the row: the page is whole without it.
                    page.collection = collection.ok().flatten();
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
        if self.cancel_up_next(cx) {
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

    /// "▶ Reprendre" or "↺ Depuis le début": the server opens a playback, then the player starts.
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
            page.error = Some("Ce film n'a aucune version à lire.");
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
        parts.push("lecture directe".into());
        let playing = Playing {
            title: plan.title.clone(),
            subtitle: Some(parts.join(" · ")),
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
    pub(super) fn tried(&mut self, origin: &Origin, error: Option<&'static str>) {
        let (busy, shown) = match origin {
            Origin::Film(depth, id) => match self.detail_page(*depth, id) {
                Some(page) => (&mut page.busy, &mut page.error),
                None => return,
            },
            Origin::Series(depth, id) => match self.series_pages.get_mut(*depth) {
                Some(page) if &page.id == id => (&mut page.busy, &mut page.error),
                _ => return,
            },
            Origin::Home => (&mut self.home.busy, &mut self.home.error),
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
        next: Option<Episode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opened = match opened {
            Ok(opened) => opened,
            Err(error) => {
                let said = self.failed(error, cx);
                self.tried(&origin, Some(said));
                return;
            }
        };
        self.tried(&origin, None);
        let options = self.mpv_options.clone();
        let Some(server) = self.server.clone() else {
            return;
        };
        let mpv = match Libmpv::new(&options) {
            Ok(mpv) => Arc::new(mpv),
            Err(error) => {
                eprintln!("bobine: the player cannot start: {error}");
                self.tried(&origin, Some("Le lecteur ne peut pas démarrer."));
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
        let fallback = Fallback::new(server.clone(), plan.clone());
        match self.play(mpv, media, playing, Some(reporting), window, cx) {
            Ok(()) => {
                if let Some(playback) = &mut self.playback {
                    playback.next = next;
                    playback.fallback = Some(fallback);
                }
            }
            Err(error) => {
                // The words of the video surface or of mpv's error code: no address in them.
                eprintln!("bobine: the film does not play: {error}");
                self.tried(&origin, Some("Lecture impossible."));
            }
        }
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
                // Continue Watching is asked again when Home shows.
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
        page.load_failed("Hors ligne");
        assert!(matches!(page.detail, Loadable::Ready(_)));
    }

    #[test]
    fn a_first_load_that_fails_shows_the_failure() {
        let mut page = DetailPage::new(ItemId(crate::fake::FILM.into()), 1);
        page.load_failed("Hors ligne");
        assert!(matches!(page.detail, Loadable::Failed("Hors ligne")));
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
    fn ma_liste_is_asked_again_each_time_it_shows() {
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
}
