//! The Home: what its header and rows hold, how it comes from the server, what its buttons
//! start, and the full list behind "See all". Like every page, each call to the server runs
//! off the window's thread, and an answer that comes after a newer ask is dropped.

use std::ops::Range;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bobine::provider::{
    Episode, Feature, FeatureKind, ItemId, Library, LibraryKind, ProviderError, Title,
};
use gpui_kit::{Context, ScrollHandle, ScrollWheelEvent, Task, UniformListScrollHandle, Window};

use super::pages::{Loadable, Origin, route_wheel};
use super::{Film as Playing, Shell};
use crate::grid::{Grid, Listed};
use crate::hero::{self, Carousel};
use crate::home_page::{self, Asks, ListKind, PlayTarget, StartError};
use crate::navigation::Screen;
use crate::scroll;

/// The header's titles: how many of them take turns.
const FEATURED: u32 = 5;

/// How often the header's bar and its change of title are looked at: the bar grows by a pixel
/// or so between two looks.
const HERO_TICK: Duration = Duration::from_millis(33);

impl Listed for Title {
    fn id(&self) -> &ItemId {
        Title::id(self)
    }

    fn title(&self) -> &str {
        match self {
            Title::Film(film) => &film.title,
            Title::Series(series) => &series.title,
            Title::Episode(episode) => &episode.series_title,
        }
    }
}

/// What a library gained lately: one row of the Home.
pub struct LatestRow {
    pub library: Library,
    pub titles: Loadable<Rc<[Title]>>,
}

pub struct HomePage {
    pub features: Option<Loadable<Rc<[Feature]>>>,
    pub carousel: Carousel,
    /// The pointer is over the header: the time stops.
    pub hovered: bool,
    pub resume: Option<Loadable<Rc<[Title]>>>,
    pub next_up: Option<Loadable<Vec<Episode>>>,
    /// What "Next up" shows, worked out when `resume` or `next_up` changes rather than
    /// at every frame.
    pub next_row: Rc<[Title]>,
    /// One row for each library of films or of series, in the server's order.
    pub latest: Option<Loadable<Vec<LatestRow>>>,
    pub scroll: ScrollHandle,
    /// The rows that scroll sideways, in the order they are drawn.
    pub rows: Vec<ScrollHandle>,
    gesture: scroll::Gesture,
    /// A play button was pressed and the server has not answered: nothing else starts.
    pub busy: bool,
    /// How many times the page was asked: an answer to an older ask is not shown.
    asked: Asks,
}

impl Default for HomePage {
    fn default() -> Self {
        Self {
            features: None,
            carousel: Carousel::new(0, Instant::now()),
            hovered: false,
            resume: None,
            next_up: None,
            next_row: Rc::from([]),
            latest: None,
            scroll: ScrollHandle::new(),
            rows: Vec::new(),
            gesture: scroll::Gesture::default(),
            busy: false,
            asked: Asks::default(),
        }
    }
}

impl HomePage {
    /// The titles of the header, if there are some to show.
    pub fn featured(&self) -> &[Feature] {
        match &self.features {
            Some(Loadable::Ready(features)) => features,
            _ => &[],
        }
    }

    /// The same titles, shared: the header draws them while it also asks the shell for pictures.
    pub fn shared_features(&self) -> Option<Rc<[Feature]>> {
        match &self.features {
            Some(Loadable::Ready(features)) => Some(Rc::clone(features)),
            _ => None,
        }
    }

    /// A new load of the page starts: the number its answers must carry, and what is not there
    /// yet shows as awaited. What is drawn stays drawn until the new answer comes.
    pub fn begin_load(&mut self) -> u64 {
        // The pointer may have been over the header when the Home was left, and its leaving
        // never heard: the header says again whether it is over it.
        self.hovered = false;
        let asked = self.asked.next();
        self.features.get_or_insert(Loadable::Loading);
        self.resume.get_or_insert(Loadable::Loading);
        self.next_up.get_or_insert(Loadable::Loading);
        self.latest.get_or_insert(Loadable::Loading);
        asked
    }

    /// Whether the answers to the load `asked` are the newest: an older one comes late and is
    /// dropped.
    pub fn is_latest(&self, asked: u64) -> bool {
        self.asked.is_latest(asked)
    }

    /// The header's titles came: the one shown stays shown when it is still among them.
    pub fn features_landed(
        &mut self,
        asked: u64,
        features: Loadable<Rc<[Feature]>>,
        now: Instant,
    ) -> bool {
        if !self.is_latest(asked) {
            return false;
        }
        let shown = self
            .featured()
            .get(self.carousel.current())
            .map(|feature| feature.id.clone());
        let list: &[Feature] = match &features {
            Loadable::Ready(features) => features,
            _ => &[],
        };
        let kept = hero::kept(shown.as_ref(), list);
        self.carousel.retarget(list.len(), kept, now);
        self.features = Some(features);
        true
    }

    pub fn resume_landed(&mut self, asked: u64, started: Loadable<Rc<[Title]>>) -> bool {
        if !self.is_latest(asked) {
            return false;
        }
        self.resume = Some(started);
        self.work_out_next_row();
        true
    }

    pub fn next_up_landed(&mut self, asked: u64, next: Loadable<Vec<Episode>>) -> bool {
        if !self.is_latest(asked) {
            return false;
        }
        self.next_up = Some(next);
        self.work_out_next_row();
        true
    }

    /// The libraries came: one row for each of films or of series, the rows already drawn
    /// keeping their titles until the new ones come. Returns the libraries to ask the latest of.
    pub fn libraries_landed(&mut self, asked: u64, libraries: Vec<Library>) -> Vec<ItemId> {
        if !self.is_latest(asked) {
            return Vec::new();
        }
        let libraries: Vec<Library> = libraries
            .into_iter()
            .filter(|library| library.kind != LibraryKind::Other)
            .collect();
        let mut rows: Vec<LatestRow> = Vec::new();
        for library in &libraries {
            let before = match &mut self.latest {
                Some(Loadable::Ready(rows)) => rows
                    .iter_mut()
                    .find(|row| row.library.id == library.id)
                    .map(|row| std::mem::replace(&mut row.titles, Loadable::Loading)),
                _ => None,
            };
            rows.push(LatestRow {
                library: library.clone(),
                titles: before.unwrap_or(Loadable::Loading),
            });
        }
        self.latest = Some(Loadable::Ready(rows));
        libraries.into_iter().map(|library| library.id).collect()
    }

    pub fn libraries_failed(&mut self, asked: u64, why: &'static str) {
        if !self.is_latest(asked) {
            return;
        }
        self.latest = Some(Loadable::Failed(why));
    }

    /// The latest titles of one library came.
    pub fn latest_landed(
        &mut self,
        asked: u64,
        library: &ItemId,
        titles: Loadable<Rc<[Title]>>,
    ) -> bool {
        if !self.is_latest(asked) {
            return false;
        }
        if let Some(Loadable::Ready(rows)) = &mut self.latest
            && let Some(row) = rows.iter_mut().find(|row| &row.library.id == library)
        {
            row.titles = titles;
        }
        true
    }

    /// The row under "Continue watching" worked out again: what is started is not asked as next.
    fn work_out_next_row(&mut self) {
        self.next_row = match &self.next_up {
            Some(Loadable::Ready(next)) => {
                let started: &[Title] = match &self.resume {
                    Some(Loadable::Ready(started)) => started,
                    _ => &[],
                };
                home_page::next_up_row(next.clone(), started)
                    .into_iter()
                    .map(Title::Episode)
                    .collect()
            }
            _ => Rc::from([]),
        };
    }

    /// The rows under the header need as many handles to scroll sideways as they are drawn.
    pub fn rows_for(&mut self, count: usize) -> Vec<ScrollHandle> {
        self.rows.resize_with(count, ScrollHandle::new);
        self.rows.clone()
    }
}

/// A full list: all of "Continue watching", of "Next up", or of what a library gained.
pub struct ListPage {
    pub kind: ListKind,
    /// The library a list of the latest belongs to.
    pub library: Option<String>,
    pub grid: Grid<Title>,
    pub scroll: UniformListScrollHandle,
    pub error: Option<&'static str>,
    /// The row offered instead when the list holds nothing.
    pub suggestions: ScrollHandle,
}

impl Shell {
    /// Asks everything the Home shows. What is drawn stays drawn until the new answer comes.
    pub fn load_home(&mut self, cx: &mut Context<Self>) {
        let asked = self.home.begin_load();
        let stamp = self.marks.stamp();
        // What the library holds may have changed: its index by TMDb id too.
        self.load_tmdb_index(cx);

        self.ask(
            cx,
            |server| server.featured(FEATURED),
            move |shell, features, cx| {
                if !shell.home.is_latest(asked) {
                    return;
                }
                let features = match features {
                    Ok(features) => Loadable::Ready(Rc::from(features)),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                shell
                    .home
                    .features_landed(asked, features, cx.background_executor().now());
            },
        );
        self.ask(
            cx,
            |server| server.continue_watching(),
            move |shell, started, cx| {
                if !shell.home.is_latest(asked) {
                    return;
                }
                let started = match started {
                    Ok(started) => {
                        shell.marks.listed(stamp, &started);
                        Loadable::Ready(Rc::from(started))
                    }
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                shell.home.resume_landed(asked, started);
            },
        );
        self.ask(
            cx,
            |server| server.next_episodes(home_page::ROW),
            move |shell, next, cx| {
                if !shell.home.is_latest(asked) {
                    return;
                }
                let next = match next {
                    Ok(next) => Loadable::Ready(next),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                shell.home.next_up_landed(asked, next);
            },
        );
        self.ask(
            cx,
            |server| server.libraries(),
            move |shell, libraries, cx| {
                if !shell.home.is_latest(asked) {
                    return;
                }
                match libraries {
                    Ok(libraries) => shell.load_latest(libraries, (asked, stamp), cx),
                    Err(error) => {
                        let why = shell.failed(error, cx);
                        shell.home.libraries_failed(asked, why);
                    }
                }
            },
        );
    }

    /// What the header shows changed: it is drawn again, alone.
    pub fn notify_hero(&self, cx: &mut Context<Self>) {
        self.hero_view.update(cx, |_, cx| cx.notify());
    }

    /// One row for each library of films or of series.
    fn load_latest(
        &mut self,
        libraries: Vec<Library>,
        (asked, stamp): (u64, u64),
        cx: &mut Context<Self>,
    ) {
        for id in self.home.libraries_landed(asked, libraries) {
            self.ask(
                cx,
                {
                    let id = id.clone();
                    move |server| server.latest(&id, home_page::ROW)
                },
                move |shell, titles, cx| {
                    if !shell.home.is_latest(asked) {
                        return;
                    }
                    let titles = match titles {
                        Ok(titles) => {
                            // A series of the latest may stand for a new season or a lone
                            // episode of it, which say nothing of the series: films only.
                            let films = titles.iter().filter(|t| matches!(t, Title::Film(_)));
                            shell.marks.listed(stamp, films);
                            Loadable::Ready(Rc::from(titles))
                        }
                        Err(error) => Loadable::Failed(shell.failed(error, cx)),
                    };
                    shell.home.latest_landed(asked, &id, titles);
                },
            );
        }
    }

    /// The header changes title by itself while the Home shows, its time not stopped: a look
    /// every few milliseconds then, none the rest of the time. Asked by the Home each time it
    /// is drawn; a ticker already running, or one title only, leaves it as it is.
    pub fn start_hero_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let on_home = self.navigator.current() == Screen::Home;
        if self.hero_ticker.is_some() || !hero::ticks(on_home, self.home.carousel.len()) {
            return;
        }
        // In the window's own context: the pointer's place is the window's to say.
        let ticker: Task<()> = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(HERO_TICK).await;
                let going = this.update_in(cx, |shell, window, cx| {
                    shell.tick_hero(cx.background_executor().now(), window, cx)
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        });
        self.hero_ticker = Some(ticker);
    }

    /// Whether the ticker goes on: not when the Home is left, and not when there is one title.
    fn tick_hero(&mut self, now: Instant, window: &Window, cx: &mut Context<Self>) -> bool {
        let on_home = self.navigator.current() == Screen::Home;
        let going = hero::ticks(on_home, self.home.carousel.len());
        self.home.carousel.pause(self.header_holds(window), now);
        if !going {
            self.hero_ticker = None;
            return false;
        }
        // The window redraws while the bar grows; nothing while the pointer holds it.
        let moved = self.home.carousel.tick(now);
        if moved || !self.home.carousel.paused() {
            self.ask_hero_images(cx);
            self.notify_hero(cx);
        }
        true
    }

    /// Whether the header's time is stopped now.
    fn header_holds(&self, window: &Window) -> bool {
        // The nested compositor of the harness has no pointer to enter the window: the events it
        // sends stand for it.
        #[cfg(feature = "harness")]
        let inside = {
            let _ = window;
            true
        };
        #[cfg(not(feature = "harness"))]
        let inside = window.is_window_hovered();
        hero::holds(
            self.navigator.current() == Screen::Home,
            self.home.hovered,
            inside,
        )
    }

    /// The pointer came over the header or left it.
    pub fn hover_hero(&mut self, hovered: bool, window: &Window, cx: &mut Context<Self>) {
        if self.home.hovered == hovered {
            return;
        }
        self.home.hovered = hovered;
        self.home
            .carousel
            .pause(self.header_holds(window), cx.background_executor().now());
        cx.notify();
    }

    /// A click on one of the marks under the header.
    pub fn jump_hero(&mut self, to: usize, cx: &mut Context<Self>) {
        self.home.carousel.jump(to, cx.background_executor().now());
        self.ask_hero_images(cx);
        self.notify_hero(cx);
    }

    /// Asks the pictures the header draws that are not here yet. The header only reads them
    /// when it draws: a write to the shell during a draw would have the whole page built again.
    pub fn ask_hero_images(&mut self, cx: &mut Context<Self>) {
        let Some(features) = self.home.shared_features() else {
            return;
        };
        let current = self
            .home
            .carousel
            .current()
            .min(features.len().saturating_sub(1));
        let leaving = self
            .home
            .carousel
            .blend(cx.background_executor().now())
            .leaving;
        for (image, width) in hero::pictures(&features, current, leaving) {
            let _ = self.image(Some(&image), width, cx);
        }
    }

    /// The wheel or the touchpad over the Home: the page goes up and down, the row under the
    /// pointer sideways.
    pub fn scroll_home(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let home = &mut self.home;
        if route_wheel(&home.scroll, &home.rows, &mut home.gesture, event, window) {
            cx.notify();
        }
    }

    /// A card or the header's "Plus d'infos": the page of the title.
    pub fn open_title(&mut self, title: &Title, cx: &mut Context<Self>) {
        match title {
            Title::Film(film) => self.open_film(film.id.clone(), cx),
            Title::Series(series) => self.open_series_on(
                series.id.clone(),
                series.season.as_ref().map(|season| season.id.clone()),
                cx,
            ),
            // An episode has no page: its series has, with where it resumes.
            Title::Episode(episode) => self.open_series(episode.series.clone(), cx),
        }
    }

    /// "Plus d'infos" under a title of the header: the title is named by the button, not by its
    /// place, since the titles may change between the draw and the click.
    pub fn open_feature(&mut self, kind: FeatureKind, id: ItemId, cx: &mut Context<Self>) {
        match kind {
            FeatureKind::Film => self.open_film(id, cx),
            FeatureKind::Series => self.open_series(id, cx),
        }
    }

    /// "Play": the server opens a playback, then the player starts, like the page of the
    /// title would have.
    pub fn play_from_home(
        &mut self,
        target: PlayTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.home.busy {
            self.forget_click();
            return;
        }
        let Some(server) = self.server.clone() else {
            return;
        };
        self.home.busy = true;
        self.notify_hero(cx);
        let task = cx
            .background_executor()
            .spawn(async move { home_page::start(&*server, &target) });
        cx.spawn_in(window, async move |this, cx| {
            let started = task.await;
            this.update_in(cx, |shell, window, cx| {
                shell.home_started(started, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn home_started(
        &mut self,
        started: Result<home_page::Started, StartError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.notify_hero(cx);
        match started {
            Ok(started) => {
                let playing = Playing {
                    title: started.title,
                    subtitle: Some(started.subtitle),
                    list: Some(started.list),
                    desktop: Some(started.desktop),
                };
                self.start_playback(
                    Origin::Home,
                    started.plan,
                    started.opened,
                    playing,
                    None,
                    false,
                    window,
                    cx,
                );
            }
            Err(StartError::Server(error)) => {
                let said = self.failed(error, cx);
                self.tried(&Origin::Home, Some(said), cx);
            }
            Err(StartError::NothingToPlay) => {
                self.tried(&Origin::Home, Some("Nothing to play for this title."), cx);
            }
        }
    }

    /// Back from the player: once "stopped" reached the server, the Home is asked anew, so that
    /// what was watched leaves the header and "Continue watching" shows where the viewer is now.
    pub(super) fn reload_home_after(&mut self, reports: JoinHandle<()>, cx: &mut Context<Self>) {
        self.ask(
            cx,
            move |_| {
                let _ = reports.join();
            },
            |shell, (), cx| {
                if shell.navigator.current() == Screen::Home {
                    shell.load_home(cx);
                }
            },
        );
    }

    /// "See all" on a row.
    pub fn open_list(&mut self, kind: ListKind, library: Option<String>, cx: &mut Context<Self>) {
        self.forward.clear();
        self.list = Some(ListPage {
            kind,
            library,
            grid: Grid::default(),
            scroll: UniformListScrollHandle::default(),
            error: None,
            suggestions: ScrollHandle::new(),
        });
        self.navigator.open(Screen::HomeList);
        self.ask_list(0..1, cx);
        cx.notify();
    }

    /// The pages of the full list holding these indexes, when not asked yet.
    pub fn ask_list(&mut self, items: Range<usize>, cx: &mut Context<Self>) {
        let Some(list) = &mut self.list else {
            return;
        };
        let kind = list.kind.clone();
        let epoch = list.grid.epoch();
        let stamp = self.marks.stamp();
        for page in list.grid.pages_to_ask(items.clone(), Instant::now()) {
            let kind = kind.clone();
            self.ask(
                cx,
                {
                    let kind = kind.clone();
                    move |server| home_page::fetch(server, &kind, page)
                },
                move |shell, found, cx| shell.listed(&kind, (epoch, page), stamp, found, cx),
            );
        }
    }

    fn listed(
        &mut self,
        kind: &ListKind,
        (epoch, page): (u64, u32),
        stamp: u64,
        found: Result<bobine::provider::Page<Title>, ProviderError>,
        cx: &mut Context<Self>,
    ) {
        // The list may have been left, or another one opened, since.
        if self.list.as_ref().is_none_or(|list| &list.kind != kind) {
            return;
        }
        match found {
            Ok(found) => {
                self.marks.listed(stamp, &found.items);
                if let Some(list) = &mut self.list {
                    list.grid.loaded(epoch, page, found);
                }
            }
            Err(error) => {
                let said = self.failed(error, cx);
                if let Some(list) = &mut self.list {
                    list.grid.failed(epoch, page, Instant::now());
                    if list.grid.total.is_none() {
                        list.error = Some(said);
                    }
                }
            }
        }
    }

    /// Back on a list: what was started or is next changed with the playback in between.
    pub(super) fn refresh_list(&mut self, cx: &mut Context<Self>) {
        let Some(list) = &self.list else {
            return;
        };
        if matches!(list.kind, ListKind::Latest(..)) {
            return;
        }
        let kind = list.kind.clone();
        let epoch = list.grid.epoch();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            {
                let kind = kind.clone();
                move |server| home_page::fetch(server, &kind, 0)
            },
            move |shell, found, _| {
                if let (Ok(found), Some(list)) = (found, &mut shell.list)
                    && list.kind == kind
                {
                    shell.marks.listed(stamp, &found.items);
                    list.grid.loaded(epoch, 0, found);
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeServer;
    use bobine::provider::{Connector, MediaServer};
    use std::sync::Arc;

    /// What the server answered to one load of the Home.
    struct Answers {
        features: Vec<Feature>,
        started: Vec<Title>,
        next: Vec<Episode>,
        libraries: Vec<Library>,
        latest: Vec<(ItemId, Vec<Title>)>,
    }

    fn read(server: &FakeServer) -> Answers {
        let libraries = server.libraries().unwrap();
        Answers {
            features: server.featured(FEATURED).unwrap(),
            started: server.continue_watching().unwrap(),
            next: server.next_episodes(home_page::ROW).unwrap(),
            latest: libraries
                .iter()
                .map(|library| {
                    (
                        library.id.clone(),
                        server.latest(&library.id, home_page::ROW).unwrap(),
                    )
                })
                .collect(),
            libraries,
        }
    }

    /// The answers reach the page as the shell hands them over, for the load `asked`.
    fn deliver(home: &mut HomePage, asked: u64, answers: &Answers) {
        home.features_landed(
            asked,
            Loadable::Ready(Rc::from(answers.features.clone())),
            Instant::now(),
        );
        home.resume_landed(asked, Loadable::Ready(Rc::from(answers.started.clone())));
        home.next_up_landed(asked, Loadable::Ready(answers.next.clone()));
        let rows = home.libraries_landed(asked, answers.libraries.clone());
        for (library, titles) in &answers.latest {
            if rows.contains(library) {
                home.latest_landed(asked, library, Loadable::Ready(Rc::from(titles.clone())));
            }
        }
    }

    /// The ids of everything the page shows.
    fn shown(home: &HomePage) -> Vec<Vec<String>> {
        let ids = |titles: &[Title]| titles.iter().map(|t| t.id().0.clone()).collect();
        let latest = match &home.latest {
            Some(Loadable::Ready(rows)) => rows
                .iter()
                .flat_map(|row| match &row.titles {
                    Loadable::Ready(titles) => ids(titles),
                    _ => Vec::new(),
                })
                .collect(),
            _ => Vec::new(),
        };
        vec![
            home.featured().iter().map(|f| f.id.0.clone()).collect(),
            match &home.resume {
                Some(Loadable::Ready(titles)) => ids(titles),
                _ => Vec::new(),
            },
            ids(&home.next_row),
            latest,
        ]
    }

    /// While the header moves by itself, only the header is drawn again: the shell, which draws
    /// the whole window, is neither notified nor built again at the header's rate.
    #[gpui_kit::test]
    fn an_idle_home_does_not_rebuild_the_shell(cx: &mut gpui_kit::TestAppContext) {
        let server: Arc<dyn Connector> = Arc::new(FakeServer::new(6, true));
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(server)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        let shell = window.root(cx).unwrap();
        cx.read(|cx| {
            let shell = shell.read(cx);
            assert_eq!(shell.navigator.current(), Screen::Home);
            assert!(
                shell.home.featured().len() > 1,
                "the header must take turns"
            );
            assert!(shell.hero_ticker.is_some(), "the header's time must run");
        });
        let notified = Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            let notified = Rc::clone(&notified);
            cx.observe(&shell, move |_, _| notified.set(notified.get() + 1))
                .detach();
        });
        let drawn = |cx: &mut gpui_kit::TestAppContext| {
            cx.read(|cx| {
                let draws = shell.read(cx).draws.borrow();
                (draws.shell.len(), draws.hero.len())
            })
        };
        let (shell_before, hero_before) = drawn(cx);

        // Two seconds of the header's time, look after look.
        for _ in 0..(2000 / HERO_TICK.as_millis()) {
            cx.executor().advance_clock(HERO_TICK);
            cx.run_until_parked();
        }

        let (shell_after, hero_after) = drawn(cx);
        assert!(
            hero_after - hero_before >= 30,
            "the header must have moved: drawn {} times",
            hero_after - hero_before
        );
        assert_eq!(notified.get(), 0, "the shell was notified");
        assert_eq!(shell_after - shell_before, 0, "the shell was built again");
    }

    /// The Home of a fake server whose header takes turns, signed in and drawn.
    fn home_with_a_header(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (gpui_kit::WindowHandle<Shell>, gpui_kit::VisualTestContext) {
        let server: Arc<dyn Connector> = Arc::new(FakeServer::new(6, true));
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(server)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        (window, visual)
    }

    fn mark(
        visual: &mut gpui_kit::VisualTestContext,
        at: usize,
    ) -> gpui_kit::Bounds<gpui_kit::Pixels> {
        visual
            .debug_bounds(Box::leak(format!("hero-mark {at}").into_boxed_str()))
            .unwrap_or_else(|| panic!("the mark of title {at} was not drawn"))
    }

    /// The mark of the title shown is a pill several times as long as the others, which are
    /// dots large enough to be seen and clicked.
    #[gpui_kit::test]
    fn the_current_hero_page_is_obvious_at_a_glance(cx: &mut gpui_kit::TestAppContext) {
        let (_window, mut visual) = home_with_a_header(cx);
        let (current, other) = (mark(&mut visual, 0), mark(&mut visual, 1));
        assert!(
            current.size.width >= other.size.width * 3.,
            "the pill is {:?} long, a dot {:?}",
            current.size.width,
            other.size.width
        );
        assert!(
            other.size.height >= gpui_kit::px(6.) && other.size.width >= gpui_kit::px(6.),
            "a dot of {:?} is too small to see",
            other.size
        );
    }

    /// When the header turns, the pill shrinks into a dot and the next dot grows into the pill:
    /// halfway through, both are between the two.
    #[gpui_kit::test]
    fn the_hero_indicator_animates_when_it_turns(cx: &mut gpui_kit::TestAppContext) {
        let (window, mut visual) = home_with_a_header(cx);
        let (pill, dot) = (
            mark(&mut visual, 0).size.width,
            mark(&mut visual, 1).size.width,
        );
        let current = |visual: &mut gpui_kit::VisualTestContext| {
            window
                .read_with(visual, |shell, _| shell.home.carousel.current())
                .unwrap()
        };
        while current(&mut visual) == 0 {
            visual.executor().advance_clock(HERO_TICK);
            visual.run_until_parked();
        }
        let half = crate::motion::crossfade() / 2;
        let mut waited = Duration::ZERO;
        while waited < half {
            visual.executor().advance_clock(HERO_TICK);
            visual.run_until_parked();
            waited += HERO_TICK;
        }
        let (leaving, arriving) = (
            mark(&mut visual, 0).size.width,
            mark(&mut visual, 1).size.width,
        );
        for (which, width) in [("leaving", leaving), ("arriving", arriving)] {
            assert!(
                width > dot && width < pill,
                "the {which} mark is {width:?} long halfway through, not between {dot:?} and {pill:?}"
            );
        }
    }

    /// A card of a new season opens its series on that season, not on the one to play next.
    #[gpui_kit::test]
    fn a_card_of_a_new_season_opens_its_series_on_that_season(cx: &mut gpui_kit::TestAppContext) {
        let server: Arc<dyn Connector> = Arc::new(FakeServer::new(6, true));
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(server)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        let shell = window.root(cx).unwrap();
        // The first series is begun in its second season: that is where the page lands unasked.
        let mut series = crate::fake::series().remove(0);
        let seasons = FakeServer::new(0, true).seasons(&series.id).unwrap();
        series.season = Some(seasons[2].clone());
        cx.update(|cx| {
            shell.update(cx, |shell, cx| {
                shell.open_title(&Title::Series(series.clone()), cx)
            })
        });
        cx.run_until_parked();
        cx.read(|cx| {
            let shell = shell.read(cx);
            assert_eq!(shell.navigator.current(), Screen::SeriesDetail);
            let page = shell.series_pages.last().expect("the series page");
            assert_eq!(page.id, series.id);
            assert_eq!(page.season, 2, "the page opens on the new season");
        });
    }

    #[test]
    fn two_loads_of_the_home_answered_out_of_order_show_the_newest() {
        let server = FakeServer::new(6, false);
        let mut home = HomePage::default();
        let old = home.begin_load();
        // The first load read the server before the viewer started a film and an episode.
        let old_answers = read(&server);
        {
            let mut state = server.state.lock().unwrap();
            state.films.remove(0);
            state.films[0].resume = Some(Duration::from_secs(600));
            state.episodes[0].item.resume = Some(Duration::from_secs(300));
        }
        let new = home.begin_load();
        let new_answers = read(&server);
        let alone = |answers: &Answers| {
            let mut home = HomePage::default();
            let asked = home.begin_load();
            deliver(&mut home, asked, answers);
            shown(&home)
        };
        assert_ne!(
            alone(&old_answers),
            alone(&new_answers),
            "the two loads must differ for this test to mean anything"
        );

        // The second load answers first, the first one comes late.
        deliver(&mut home, new, &new_answers);
        let newest = shown(&home);
        deliver(&mut home, old, &old_answers);
        assert_eq!(shown(&home), newest);
    }
}
