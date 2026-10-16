//! The Home: what its header and rows hold, how it comes from the server, what its buttons
//! start, and the full list behind "Tout voir". Like every page, each call to the server runs
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
    /// What "Prochain épisode" shows, worked out when `resume` or `next_up` changes rather than
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
    pub error: Option<&'static str>,
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
            error: None,
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

    /// The row under "Reprendre" worked out again: what is started is not asked as next.
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

/// A full list: all of "Reprendre", of "Prochain épisode", or of what a library gained.
pub struct ListPage {
    pub kind: ListKind,
    /// The library a list of the latest belongs to.
    pub library: Option<String>,
    pub grid: Grid<Title>,
    pub scroll: UniformListScrollHandle,
    pub error: Option<&'static str>,
}

impl Shell {
    /// Asks everything the Home shows. What is drawn stays drawn until the new answer comes.
    pub fn load_home(&mut self, cx: &mut Context<Self>) {
        // The pointer may have been over the header when the Home was left, and its leaving
        // never heard: the header says again whether it is over it.
        self.home.hovered = false;
        let asked = self.home.asked.next();
        let home = &mut self.home;
        home.features.get_or_insert(Loadable::Loading);
        home.resume.get_or_insert(Loadable::Loading);
        home.next_up.get_or_insert(Loadable::Loading);
        home.latest.get_or_insert(Loadable::Loading);

        self.ask(
            cx,
            |server| server.featured(FEATURED),
            move |shell, features, cx| {
                if !shell.home.asked.is_latest(asked) {
                    return;
                }
                let features = match features {
                    Ok(features) => Loadable::Ready(Rc::from(features)),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                shell.show_features(features);
            },
        );
        self.ask(
            cx,
            |server| server.continue_watching(),
            move |shell, started, cx| {
                if shell.home.asked.is_latest(asked) {
                    shell.home.resume = Some(match started {
                        Ok(started) => Loadable::Ready(Rc::from(started)),
                        Err(error) => Loadable::Failed(shell.failed(error, cx)),
                    });
                    shell.home.work_out_next_row();
                }
            },
        );
        self.ask(
            cx,
            |server| server.next_episodes(home_page::ROW),
            move |shell, next, cx| {
                if shell.home.asked.is_latest(asked) {
                    shell.home.next_up = Some(match next {
                        Ok(next) => Loadable::Ready(next),
                        Err(error) => Loadable::Failed(shell.failed(error, cx)),
                    });
                    shell.home.work_out_next_row();
                }
            },
        );
        self.ask(
            cx,
            |server| server.libraries(),
            move |shell, libraries, cx| {
                if !shell.home.asked.is_latest(asked) {
                    return;
                }
                match libraries {
                    Ok(libraries) => shell.load_latest(libraries, asked, cx),
                    Err(error) => {
                        shell.home.latest = Some(Loadable::Failed(shell.failed(error, cx)));
                    }
                }
            },
        );
    }

    /// The header's titles came: the one shown stays shown when it is still among them.
    fn show_features(&mut self, features: Loadable<Rc<[Feature]>>) {
        let now = Instant::now();
        let shown = self
            .home
            .featured()
            .get(self.home.carousel.current())
            .map(|feature| feature.id.clone());
        let list: &[Feature] = match &features {
            Loadable::Ready(features) => features,
            _ => &[],
        };
        let kept = hero::kept(shown.as_ref(), list);
        self.home.carousel.retarget(list.len(), kept, now);
        self.home.features = Some(features);
    }

    /// What the header shows changed: it is drawn again, alone.
    pub fn notify_hero(&self, cx: &mut Context<Self>) {
        self.hero_view.update(cx, |_, cx| cx.notify());
    }

    /// One row for each library of films or of series.
    fn load_latest(&mut self, libraries: Vec<Library>, asked: u64, cx: &mut Context<Self>) {
        let libraries: Vec<Library> = libraries
            .into_iter()
            .filter(|library| library.kind != LibraryKind::Other)
            .collect();
        // Rows already drawn keep their titles until the new ones come.
        let mut rows: Vec<LatestRow> = Vec::new();
        for library in &libraries {
            let before = match &mut self.home.latest {
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
        self.home.latest = Some(Loadable::Ready(rows));
        for library in libraries {
            let id = library.id.clone();
            self.ask(
                cx,
                {
                    let id = id.clone();
                    move |server| server.latest(&id, home_page::ROW)
                },
                move |shell, titles, cx| {
                    if !shell.home.asked.is_latest(asked) {
                        return;
                    }
                    let titles = match titles {
                        Ok(titles) => Loadable::Ready(Rc::from(titles)),
                        Err(error) => Loadable::Failed(shell.failed(error, cx)),
                    };
                    if let Some(Loadable::Ready(rows)) = &mut shell.home.latest
                        && let Some(row) = rows.iter_mut().find(|row| row.library.id == id)
                    {
                        row.titles = titles;
                    }
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
                    shell.tick_hero(Instant::now(), window, cx)
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
            .pause(self.header_holds(window), Instant::now());
        cx.notify();
    }

    /// A click on one of the marks under the header.
    pub fn jump_hero(&mut self, to: usize, cx: &mut Context<Self>) {
        self.home.carousel.jump(to, Instant::now());
        self.notify_hero(cx);
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
            Title::Series(series) => self.open_series(series.id.clone(), cx),
            // An episode has no page: its series has, with where it resumes.
            Title::Episode(episode) => self.open_series(episode.series.clone(), cx),
        }
    }

    /// "Plus d'infos" under the header's `at`th title.
    pub fn open_feature(&mut self, at: usize, cx: &mut Context<Self>) {
        let Some(feature) = self.home.featured().get(at) else {
            return;
        };
        let id = feature.id.clone();
        match feature.kind {
            FeatureKind::Film => self.open_film(id, cx),
            FeatureKind::Series => self.open_series(id, cx),
        }
    }

    /// "Lecture" under the header's `at`th title.
    pub fn play_feature(&mut self, at: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(feature) = self.home.featured().get(at) else {
            return;
        };
        let target = home_page::play_target(feature);
        self.play_from_home(target, window, cx);
    }

    /// "Lecture": the server opens a playback, then the player starts, like the page of the
    /// title would have.
    pub fn play_from_home(
        &mut self,
        target: PlayTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.home.busy {
            return;
        }
        let Some(server) = self.server.clone() else {
            return;
        };
        self.home.busy = true;
        self.home.error = None;
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

    fn home_started(
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
                };
                self.start_playback(
                    Origin::Home,
                    started.plan,
                    started.opened,
                    playing,
                    None,
                    window,
                    cx,
                );
            }
            Err(StartError::Server(error)) => {
                let said = self.failed(error, cx);
                self.tried(&Origin::Home, Some(said));
            }
            Err(StartError::NothingToPlay) => {
                self.tried(&Origin::Home, Some("Rien à lire pour ce titre."));
            }
        }
    }

    /// Back from the player: once "stopped" reached the server, the Home is asked anew, so that
    /// what was watched leaves the header and "Reprendre" shows where the viewer is now.
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

    /// "Tout voir" on a row.
    pub fn open_list(&mut self, kind: ListKind, library: Option<String>, cx: &mut Context<Self>) {
        self.list = Some(ListPage {
            kind,
            library,
            grid: Grid::default(),
            scroll: UniformListScrollHandle::default(),
            error: None,
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
        for page in list.grid.pages_to_ask(items.clone(), Instant::now()) {
            let kind = kind.clone();
            self.ask(
                cx,
                {
                    let kind = kind.clone();
                    move |server| home_page::fetch(server, &kind, page)
                },
                move |shell, found, cx| shell.listed(&kind, page, found, cx),
            );
        }
    }

    fn listed(
        &mut self,
        kind: &ListKind,
        page: u32,
        found: Result<bobine::provider::Page<Title>, ProviderError>,
        cx: &mut Context<Self>,
    ) {
        // The list may have been left, or another one opened, since.
        if self.list.as_ref().is_none_or(|list| &list.kind != kind) {
            return;
        }
        match found {
            Ok(found) => {
                if let Some(list) = &mut self.list {
                    list.grid.loaded(page, found);
                }
            }
            Err(error) => {
                let said = self.failed(error, cx);
                if let Some(list) = &mut self.list {
                    list.grid.failed(page, Instant::now());
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
                    list.grid.loaded(0, found);
                }
            },
        );
    }
}
