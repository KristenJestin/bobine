//! The request page of a title the library does not hold (#63): what it is, after the film and
//! series pages, and its request. A film is asked for in one click; a series opens a menu of the
//! seasons not had nor asked for, all checked, under the button.

use std::cell::Cell;
use std::rc::Rc;

use bobine::requests::{Ask, Availability, Discovered, MediaKind, TitleDetail, TmdbId};
use gpui_kit::{Context, Pixels, Point, ScrollHandle, ScrollWheelEvent, Window};

use super::pages::{ChipBounds, route_wheel};
use super::{Loadable, Shell};
use crate::discover::{self, SeasonPicker};
use crate::menu::Menus;
use crate::motion::{self, Presence};
use crate::navigation::Screen;
use crate::scroll;

/// The one menu of a request page: the seasons of a series.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seasons;

pub struct RequestPage {
    pub kind: MediaKind,
    pub tmdb: TmdbId,
    /// Told apart from every other page ever opened, the closed ones included.
    generation: u64,
    pub detail: Loadable<TitleDetail>,
    /// The server's recommendations for it.
    pub similar: Vec<Discovered>,
    /// A series: the seasons its request offers.
    pub picker: Option<SeasonPicker>,
    pub menus: Menus<Seasons>,
    pub menu_fade: Presence<Seasons>,
    /// Where the request button was last drawn, so the seasons open under it.
    pub button: ChipBounds,
    /// A request on its way.
    pub busy: bool,
    /// Why the last request failed.
    pub error: Option<&'static str>,
    pub scroll: ScrollHandle,
    /// The cast and the similar titles: each scrolls sideways.
    pub rows: [ScrollHandle; 2],
    gesture: scroll::Gesture,
}

impl RequestPage {
    fn new(kind: MediaKind, tmdb: TmdbId, generation: u64) -> Self {
        Self {
            kind,
            tmdb,
            generation,
            detail: Loadable::Loading,
            similar: Vec::new(),
            picker: None,
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            button: Rc::new(Cell::new(None)),
            busy: false,
            error: None,
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            gesture: scroll::Gesture::default(),
        }
    }

    /// What was just asked for shows as asked: the title pending, a series' seasons taken.
    fn asked(&mut self, ask: &Ask) {
        let Loadable::Ready(detail) = &mut self.detail else {
            return;
        };
        if detail.title.availability == Availability::Missing {
            detail.title.availability = Availability::Pending;
        }
        if let Ask::Tv(_, seasons) = ask {
            for season in &mut detail.seasons {
                season.taken |= seasons.contains(&season.number);
            }
            self.picker = Some(SeasonPicker::new(&detail.seasons));
        }
    }

    /// Whether the page offers its request: a film nobody asked for, or a series with seasons
    /// left to ask for.
    pub fn can_request(&self, in_library: bool) -> bool {
        let Loadable::Ready(detail) = &self.detail else {
            return false;
        };
        match self.kind {
            MediaKind::Movie => discover::can_request(detail.title.availability, in_library),
            MediaKind::Tv => {
                !in_library
                    && !matches!(
                        detail.title.availability,
                        Availability::Available | Availability::Blocked
                    )
                    && self.picker.as_ref().is_some_and(|p| !p.offered.is_empty())
            }
        }
    }
}

impl Shell {
    /// The page of a title the library does not hold, to request it.
    pub fn open_request(&mut self, kind: MediaKind, tmdb: TmdbId, cx: &mut Context<Self>) {
        if self.seerr.client.is_none() {
            return;
        }
        self.forward.clear();
        let generation = self.next_generation();
        self.seerr
            .requests
            .push(RequestPage::new(kind, tmdb, generation));
        self.navigator.open(Screen::Request);
        self.want_tmdb_index(cx);
        self.load_request(generation, cx);
        cx.notify();
    }

    /// The request page of that number, if it is still open.
    fn request_numbered(&mut self, generation: u64) -> Option<&mut RequestPage> {
        self.seerr
            .requests
            .iter_mut()
            .find(|page| page.generation == generation)
    }

    /// Reads the title and its recommendations, for the page of that number.
    fn load_request(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(page) = self.request_numbered(generation) else {
            return;
        };
        let (kind, tmdb) = (page.kind, page.tmdb);
        self.ask_seerr(
            cx,
            move |seerr| seerr.title(kind, tmdb),
            move |shell, detail, _| {
                let Some(page) = shell.request_numbered(generation) else {
                    return;
                };
                match detail {
                    Ok(detail) => {
                        page.picker =
                            (kind == MediaKind::Tv).then(|| SeasonPicker::new(&detail.seasons));
                        page.detail = Loadable::Ready(detail);
                    }
                    // A page already shown stays: only a first read says it failed.
                    Err(error) if !matches!(page.detail, Loadable::Ready(_)) => {
                        page.detail = Loadable::Failed(discover::error_line(&error));
                    }
                    Err(_) => {}
                }
            },
        );
        self.ask_seerr(
            cx,
            move |seerr| seerr.recommendations(kind, tmdb),
            move |shell, similar, _| {
                // Without them the page is whole: the row hides.
                if let (Some(page), Ok(similar)) = (shell.request_numbered(generation), similar) {
                    page.similar = similar;
                }
            },
        );
    }

    /// The request button: a film is asked for; a series opens its seasons, or closes them.
    pub fn request_title(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.seerr.requests.last_mut() else {
            return;
        };
        match page.kind {
            MediaKind::Movie => {
                let ask = Ask::Movie(page.tmdb);
                self.send_request(ask, cx);
            }
            MediaKind::Tv => {
                page.menus.toggle(Seasons);
                cx.notify();
            }
        }
    }

    /// A season of the menu checked or unchecked.
    pub fn toggle_season(&mut self, number: u32, cx: &mut Context<Self>) {
        if let Some(picker) = self
            .seerr
            .requests
            .last_mut()
            .and_then(|page| page.picker.as_mut())
        {
            picker.toggle(number);
            cx.notify();
        }
    }

    /// The menu's own button: the seasons checked are asked for.
    pub fn request_seasons(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.seerr.requests.last_mut() else {
            return;
        };
        let Some(chosen) = page.picker.as_ref().map(SeasonPicker::chosen) else {
            return;
        };
        if chosen.is_empty() {
            return;
        }
        let ask = Ask::Tv(page.tmdb, chosen);
        self.send_request(ask, cx);
    }

    /// Sends the request of the page shown; once it went through, the page reads its title
    /// again (now pending) and every row that shows it says so.
    fn send_request(&mut self, ask: Ask, cx: &mut Context<Self>) {
        let Some(page) = self.seerr.requests.last_mut() else {
            return;
        };
        if page.busy || !matches!(page.detail, Loadable::Ready(_)) {
            return;
        }
        page.busy = true;
        page.error = None;
        page.menus.close();
        let (generation, kind, tmdb) = (page.generation, page.kind, page.tmdb);
        let asked = ask.clone();
        self.ask_seerr(
            cx,
            move |seerr| seerr.request(&ask),
            move |shell, done, cx| {
                if done.is_ok() {
                    shell.mark_requested(kind, tmdb);
                }
                let Some(page) = shell.request_numbered(generation) else {
                    return;
                };
                page.busy = false;
                match done {
                    Ok(()) => {
                        // Pending at once, whatever the read that follows says: a second press
                        // has nothing left to ask for, even if that read fails.
                        page.asked(&asked);
                        shell.load_request(generation, cx);
                    }
                    Err(error) => page.error = Some(discover::error_line(&error)),
                }
            },
        );
        cx.notify();
    }

    /// A title just asked for shows pending wherever it was missing, until the rows are read
    /// again.
    fn mark_requested(&mut self, kind: MediaKind, tmdb: TmdbId) {
        let mark = |titles: &mut Vec<Discovered>| {
            for title in titles {
                if title.kind == kind
                    && title.tmdb == tmdb
                    && title.availability == Availability::Missing
                {
                    title.availability = Availability::Pending;
                }
            }
        };
        let discover = &mut self.seerr.discover;
        for row in discover.rows.iter_mut().chain([&mut discover.results]) {
            if let Some(Loadable::Ready(titles)) = row {
                mark(titles);
            }
        }
        for page in &mut self.seerr.requests {
            mark(&mut page.similar);
        }
    }

    /// The seasons menu of the page shown, and where its button was drawn.
    pub(super) fn seasons_menu(&mut self) -> Option<(&mut Menus<Seasons>, ChipBounds)> {
        if self.navigator.current() != Screen::Request {
            return None;
        }
        let page = self.seerr.requests.last_mut()?;
        Some((&mut page.menus, page.button.clone()))
    }

    /// The mouse went down outside the seasons menu: it closes, unless on its own button,
    /// whose click closes it.
    pub fn pressed_outside_seasons(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some((menus, button)) = self.seasons_menu() {
            let on = button
                .get()
                .is_some_and(|bounds| bounds.contains(&position))
                .then_some(Seasons);
            menus.pressed_outside(on);
            cx.notify();
        }
    }

    /// The wheel over a request page: the page up and down, the row under the pointer sideways.
    pub fn scroll_request_page(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.seerr.requests.last_mut() else {
            return;
        };
        if route_wheel(&page.scroll, &page.rows, &mut page.gesture, event, window) {
            // The menu stays under its button only while the page stands still.
            page.menus.close();
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use bobine::requests::{Ask, MediaKind, RequestError};
    use gpui_kit::{Modifiers, TestAppContext, WindowHandle};

    use super::super::discover::tests::{
        KEY, address_only, click, configured, configured_on, drawn,
    };
    use super::*;
    use crate::fake::{FILM, FakeServer, SERIES};
    use crate::fake_seerr::{
        ASHEN_SHORE, COPPER_COAST, FakeSeerr, HARBOUR_WATCH, NORTHERN_LIGHTS, PAPER_MOONS,
    };
    use crate::navigation::NavEntry;
    use bobine::provider::{ItemId, ProviderError};

    /// A shell on Discover, then on the request page of a title.
    fn on_request(
        seerr: &FakeSeerr,
        kind: MediaKind,
        tmdb: TmdbId,
        cx: &mut TestAppContext,
    ) -> WindowHandle<Shell> {
        let window = configured(seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.select(NavEntry::Discover, cx);
                shell.open_discovered(kind, tmdb, cx);
            })
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
    fn a_title_not_in_the_library_opens_its_request_page(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Movie, NORTHERN_LIGHTS, cx);
        assert_eq!(screen(&window, cx), Screen::Request);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("request-page").is_some());
        assert!(visual.debug_bounds("request-button").is_some());
        // The cast, and similar titles with their pills.
        assert!(visual.debug_bounds("request-cast 0").is_some());
        assert!(visual.debug_bounds("discover-card Paper Moons").is_some());
        assert!(seerr.requests().is_empty(), "opening asks for nothing");
    }

    #[gpui_kit::test]
    fn a_film_is_requested_and_shows_as_pending(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Movie, NORTHERN_LIGHTS, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        assert_eq!(seerr.requests(), [Ask::Movie(NORTHERN_LIGHTS)]);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Pending").is_some());
        assert!(visual.debug_bounds("request-button").is_none());
        // Discover's rows say so too, without being read again.
        window
            .update(cx, |shell, _, _| {
                let Some(Loadable::Ready(trending)) = &shell.seerr.discover.rows[0] else {
                    panic!("trending was read");
                };
                let film = trending.iter().find(|t| t.tmdb == NORTHERN_LIGHTS).unwrap();
                assert_eq!(film.availability, Availability::Pending);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_double_press_sends_one_request_even_when_the_title_cannot_be_read_again(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().titles_fail_after_a_request = true;
        let window = on_request(&seerr, MediaKind::Movie, NORTHERN_LIGHTS, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        if visual.debug_bounds("request-button").is_some() {
            click(&mut visual, "request-button");
        }
        assert_eq!(seerr.requests(), [Ask::Movie(NORTHERN_LIGHTS)]);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Pending").is_some());
    }

    #[gpui_kit::test]
    fn a_series_request_takes_its_seasons_out_of_the_menu_at_once(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().titles_fail_after_a_request = true;
        let window = on_request(&seerr, MediaKind::Tv, COPPER_COAST, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        click(&mut visual, "season 2");
        click(&mut visual, "seasons-send");
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Pending").is_some());
        click(&mut visual, "request-button");
        assert!(visual.debug_bounds("season 1").is_none());
        assert!(visual.debug_bounds("season 2").is_some());
    }

    #[gpui_kit::test]
    fn a_title_requested_already_cannot_be_requested_again(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Movie, PAPER_MOONS, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Pending").is_some());
        assert!(visual.debug_bounds("request-button").is_none());
    }

    #[gpui_kit::test]
    fn a_blocklisted_title_says_blocked_and_cannot_be_requested(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Movie, ASHEN_SHORE, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Blocked").is_some());
        assert!(visual.debug_bounds("request-button").is_none());
    }

    #[gpui_kit::test]
    fn a_refused_request_says_why_in_a_line(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().movie_quota = Some(0);
        let window = on_request(&seerr, MediaKind::Movie, NORTHERN_LIGHTS, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        assert!(seerr.requests().is_empty());
        window
            .update(cx, |shell, _, _| {
                let page = shell.seerr.requests.last().unwrap();
                assert_eq!(page.error, Some("Your request quota is reached."));
                assert!(!page.busy);
            })
            .unwrap();
        assert!(visual.debug_bounds("request-error").is_some());
    }

    #[gpui_kit::test]
    fn a_series_is_requested_with_chosen_seasons(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Tv, COPPER_COAST, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        assert!(visual.debug_bounds("seasons-menu").is_some());
        assert!(seerr.requests().is_empty(), "the button opens the seasons");
        click(&mut visual, "season 2");
        click(&mut visual, "seasons-send");
        assert_eq!(seerr.requests(), [Ask::Tv(COPPER_COAST, vec![1, 3])]);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("seasons-menu").is_none());
    }

    #[gpui_kit::test]
    fn a_series_request_leaves_out_the_seasons_already_had_or_requested(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Tv, HARBOUR_WATCH, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("status Partially available").is_some());
        click(&mut visual, "request-button");
        assert!(visual.debug_bounds("season 1").is_none());
        assert!(visual.debug_bounds("season 2").is_none());
        assert!(visual.debug_bounds("season 3").is_some());
        click(&mut visual, "seasons-send");
        assert_eq!(seerr.requests(), [Ask::Tv(HARBOUR_WATCH, vec![3])]);
    }

    #[gpui_kit::test]
    fn the_seasons_menu_closes_on_escape_and_on_an_outside_click(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Tv, COPPER_COAST, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        assert!(visual.debug_bounds("seasons-menu").is_some());
        window.update(cx, |shell, _, cx| shell.escape(cx)).unwrap();
        assert_eq!(
            screen(&window, cx),
            Screen::Request,
            "Escape closes the menu only"
        );
        let open = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, _| {
                    shell.seerr.requests.last().unwrap().menus.open().is_some()
                })
                .unwrap()
        };
        assert!(!open(cx));
        let mut visual = drawn(&window, cx);
        click(&mut visual, "request-button");
        assert!(open(cx));
        // A click on the title, far from the menu.
        let away = visual.debug_bounds("detail-title").unwrap().center();
        visual.simulate_click(away, Modifiers::none());
        visual.run_until_parked();
        assert!(!open(cx));
        assert!(seerr.requests().is_empty());
    }

    #[gpui_kit::test]
    fn the_request_page_goes_back_to_discover_and_forward_again(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_request(&seerr, MediaKind::Movie, NORTHERN_LIGHTS, cx);
        window
            .update(cx, |shell, _, cx| assert!(shell.go_back(cx)))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Discover);
        window
            .update(cx, |shell, _, cx| assert!(shell.go_forward(cx)))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Request);
        window
            .update(cx, |shell, _, _| {
                let page = shell.seerr.requests.last().unwrap();
                assert_eq!((page.kind, page.tmdb), (MediaKind::Movie, NORTHERN_LIGHTS));
                assert!(matches!(page.detail, Loadable::Ready(_)));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_library_film_shows_similar_titles_not_in_the_library(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("similar-not-in-library").is_some());
        assert!(
            visual
                .debug_bounds("discover-card Northern Lights")
                .is_some()
        );
        // The Tides is in the library: it is not offered.
        assert!(visual.debug_bounds("discover-card The Tides").is_none());
        // The row is under the hero: the page scrolls down to it first.
        window
            .update(cx, |shell, _, cx| {
                let page = shell.details.last().unwrap();
                let bottom = page.scroll.max_offset().y;
                page.scroll
                    .set_offset(gpui_kit::point(gpui_kit::px(0.), -bottom));
                cx.notify();
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "discover-card Northern Lights");
        assert_eq!(screen(&window, cx), Screen::Request);
    }

    #[gpui_kit::test]
    fn a_library_series_shows_similar_series_not_in_the_library(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(SERIES.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("similar-not-in-library").is_some());
        assert!(visual.debug_bounds("discover-card Copper Coast").is_some());
        assert!(visual.debug_bounds("discover-card Signal Lost").is_none());
    }

    /// A media server whose index by TMDb id fails until it is told otherwise.
    fn without_index() -> FakeServer {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().tmdb_index_error =
            Some(ProviderError::Unreachable("down".into()));
        server
    }

    fn index_back(server: &FakeServer) {
        server.state.lock().unwrap().tmdb_index_error = None;
    }

    #[gpui_kit::test]
    fn the_similar_row_waits_for_the_tmdb_index_and_a_failed_one_is_asked_again(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        let server = without_index();
        let window = configured_on(server.clone(), &seerr, cx);
        let film = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
                .unwrap();
            cx.run_until_parked();
        };
        film(cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("similar-not-in-library").is_none());
        index_back(&server);
        window
            .update(cx, |shell, _, cx| assert!(shell.go_back(cx)))
            .unwrap();
        film(cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("similar-not-in-library").is_some());
        assert!(visual.debug_bounds("discover-card The Tides").is_none());
    }

    #[gpui_kit::test]
    fn the_request_button_waits_for_the_tmdb_index(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let server = without_index();
        let window = configured_on(server.clone(), &seerr, cx);
        let request = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| {
                    shell.open_request(MediaKind::Movie, NORTHERN_LIGHTS, cx)
                })
                .unwrap();
            cx.run_until_parked();
        };
        request(cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("request-page").is_some());
        assert!(visual.debug_bounds("request-button").is_none());
        index_back(&server);
        window
            .update(cx, |shell, _, cx| assert!(shell.go_back(cx)))
            .unwrap();
        request(cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("request-button").is_some());
    }

    #[gpui_kit::test]
    fn similar_is_asked_when_seerr_becomes_ready_for_a_page_already_open(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = address_only(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        let key = window
            .update(cx, |shell, _, _| shell.seerr.key.clone().unwrap())
            .unwrap();
        key.update(cx, |field, cx| {
            field.type_in(KEY, cx);
            field.save(cx);
        });
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("similar-not-in-library").is_some());
    }

    #[gpui_kit::test]
    fn seerr_down_the_request_page_says_so(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        seerr.state.lock().unwrap().failure = Some(RequestError::Unreachable("down".into()));
        window
            .update(cx, |shell, _, cx| {
                shell.open_request(MediaKind::Movie, NORTHERN_LIGHTS, cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    shell.seerr.requests.last().unwrap().detail,
                    Loadable::Failed("Seerr cannot be reached.")
                ));
            })
            .unwrap();
    }
}
