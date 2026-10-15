//! What the screens show and how it comes from the server. Every call to the server runs on the
//! background executor; its answer comes back to the window's thread, which updates the page.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bobine::provider::{
    Collection, Connector, Episode, Film, FilmDetail, FilmQuery, ItemId, LibraryKind, MediaServer,
    Page, Playback as Opened, PlaybackReport, ProviderError,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, Context, Entity, Image, Pixels, Point, RenderImage, ScrollHandle, ScrollWheelEvent,
    Subscription, Task, TouchPhase, UniformListScrollHandle, Window, point, px,
};

use super::{Film as Playing, Shell};
use crate::film_page::{self, Choices, From, Plan};
use crate::grid::{self, Grid, Listed};
use crate::images::{self, Key};
use crate::input::TextInput;
use crate::menu::Menus;
use crate::motion::{self, Presence};
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

#[derive(Default)]
pub struct HomePage {
    pub resume: Option<Loadable<Vec<Film>>>,
}

/// The films or the series of the first library of their kind of the user.
pub struct LibraryPage<T> {
    pub library: Option<Loadable<ItemId>>,
    pub grid: Grid<T>,
    pub scroll: UniformListScrollHandle,
}

impl<T> Default for LibraryPage<T> {
    fn default() -> Self {
        Self {
            library: None,
            grid: Grid::default(),
            scroll: UniformListScrollHandle::default(),
        }
    }
}

/// The page a play button was pressed on, at its depth, with what it shows.
pub enum Origin {
    Film(usize, ItemId),
    Series(usize, ItemId),
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
}

pub struct DetailPage {
    pub id: ItemId,
    pub detail: Loadable<FilmDetail>,
    pub choices: Option<Choices>,
    /// Hidden while unknown, when the film is in none, or when the lookup failed.
    pub collection: Option<Collection>,
    pub menus: Menus<Chip>,
    /// The menu drawn, kept while it fades out.
    pub menu_fade: Presence<Chip>,
    /// Where each chip was last drawn, so its menu opens under it.
    pub chips: [Rc<Cell<Option<Bounds<Pixels>>>>; 3],
    pub scroll: ScrollHandle,
    /// The rows that scroll sideways: the cast, the collection, similar titles.
    pub rows: [ScrollHandle; 3],
    gesture: scroll::Gesture,
    /// What went wrong with the last button.
    pub error: Option<&'static str>,
    pub busy: bool,
}

impl DetailPage {
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
        self.details.clear();
        self.series_pages.clear();
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
        self.arrived(cx);
        cx.notify();
    }

    /// Escape or the back link.
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        match self.navigator.current() {
            Screen::FilmDetail => drop(self.details.pop()),
            Screen::SeriesDetail => drop(self.series_pages.pop()),
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
        match self.navigator.current() {
            Screen::Home => self.load_home(cx),
            Screen::Library if self.library.library.is_none() => {
                self.load_library(LibraryKind::Films, cx)
            }
            Screen::Series if self.series.library.is_none() => {
                self.load_library(LibraryKind::Series, cx)
            }
            _ => {}
        }
    }

    pub fn load_home(&mut self, cx: &mut Context<Self>) {
        if self.home.resume.is_none() {
            self.home.resume = Some(Loadable::Loading);
        }
        self.ask(
            cx,
            |server| server.continue_watching(),
            |shell, films, cx| {
                shell.home.resume = Some(match films {
                    Ok(films) => Loadable::Ready(films),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                });
            },
        );
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
                *shell.library_of(kind) = Some(loaded);
                match kind {
                    LibraryKind::Series => shell.ask_series(0..1, cx),
                    _ => shell.ask_films(0..1, cx),
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
        for page in shown.grid.pages_to_ask(items.clone(), Instant::now()) {
            let library = library.clone();
            self.ask(
                cx,
                move |server| fetch(server, &library, &grid::query(page)),
                move |shell, items, cx| match items {
                    Ok(items) => page_of(shell).grid.loaded(page, items),
                    Err(error) => {
                        page_of(shell).grid.failed(page, Instant::now());
                        if page_of(shell).grid.total.is_none() {
                            let said = shell.failed(error, cx);
                            page_of(shell).library = Some(Loadable::Failed(said));
                        }
                    }
                },
            );
        }
    }

    pub fn open_film(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.details.push(DetailPage {
            id,
            detail: Loadable::Loading,
            choices: None,
            collection: None,
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            chips: Default::default(),
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            gesture: Default::default(),
            error: None,
            busy: false,
        });
        self.navigator.open(Screen::FilmDetail);
        self.load_detail(self.details.len() - 1, true, cx);
        cx.notify();
    }

    /// The page at `depth` of the detail pages, if it still shows `id`.
    fn detail_page(&mut self, depth: usize, id: &ItemId) -> Option<&mut DetailPage> {
        self.details.get_mut(depth).filter(|page| &page.id == id)
    }

    /// Asks the film, then (once the page shows) its collection: that lookup is slow.
    fn load_detail(&mut self, depth: usize, with_collection: bool, cx: &mut Context<Self>) {
        let id = self.details[depth].id.clone();
        let asked = id.clone();
        self.ask(
            cx,
            move |server| server.film(&asked),
            move |shell, detail, cx| {
                let detail = match detail {
                    Ok(detail) => detail,
                    Err(error) => {
                        let said = shell.failed(error, cx);
                        if let Some(page) = shell.detail_page(depth, &id) {
                            page.detail = Loadable::Failed(said);
                        }
                        return;
                    }
                };
                shell.library.grid.update(&detail.film);
                let Some(page) = shell.detail_page(depth, &id) else {
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
        match chip {
            Chip::Version => choices.set_version(&detail.versions, value.unwrap_or(0) as usize),
            Chip::Audio => choices.audio = value,
            Chip::Subtitle => choices.subtitle = value,
        }
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
    fn tried(&mut self, origin: &Origin, error: Option<&'static str>) {
        let (busy, shown) = match origin {
            Origin::Film(depth, id) => match self.detail_page(*depth, id) {
                Some(page) => (&mut page.busy, &mut page.error),
                None => return,
            },
            Origin::Series(depth, id) => match self.series_pages.get_mut(*depth) {
                Some(page) if &page.id == id => (&mut page.busy, &mut page.error),
                _ => return,
            },
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
            server,
            PlaybackReport {
                session: opened.session.clone(),
                film: opened.film.clone(),
                source: opened.source.clone(),
                position: plan.start,
                paused: false,
                audio_track: plan.audio_index,
                subtitle_track: plan.subtitle_index,
            },
        );
        let externals = plan.externals(&opened);
        let media = plan.media(&opened, &externals);
        match self.play(mpv, media, playing, Some(reporting), window, cx) {
            Ok(()) => {
                if let Some(playback) = &mut self.playback {
                    playback.next = next;
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
        if self.navigator.current() == Screen::SeriesDetail {
            if let Some(depth) = self.series_pages.len().checked_sub(1) {
                self.load_series(depth, false, Some(reports), cx);
            }
            return;
        }
        let Some(depth) = self.details.len().checked_sub(1) else {
            return;
        };
        let id = self.details[depth].id.clone();
        let asked = id.clone();
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
                if let Some(page) = shell.detail_page(depth, &id)
                    && let Loadable::Ready(shown) = &mut page.detail
                {
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
                    shell.fetch_images(cx);
                    shell.wake_for_image_retries(cx);
                },
            );
        }
    }
}
