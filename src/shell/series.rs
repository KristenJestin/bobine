//! The series grid and the series page: what they show, how it comes from the server, and the
//! episodes they play. Like the film page, every call to the server runs off the window's thread.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bobine::provider::{Episode, ItemId, ProviderError, Season, SeriesDetail};
use gpui_kit::{Bounds, Context, Pixels, ScrollHandle, ScrollWheelEvent, Task, Window};

use super::pages::{Chip, Loadable, Origin, route_wheel};
use super::{Film as Playing, Shell};
use crate::favourite::{self, ListPage, Toggle};
use crate::film_page::{self, Carried, Choices, From};
use crate::menu::Menus;
use crate::motion::{self, Presence};
use crate::navigation::Screen;
use crate::poster_actions::Action;
use crate::scroll;
use crate::series_page::{self, Next, Refreshes};

/// How often the "Next episode" card checks its countdown.
const UP_NEXT_TICK: Duration = Duration::from_millis(250);

pub struct SeriesPage {
    pub id: ItemId,
    /// Told apart from every other page ever opened, the closed ones included.
    generation: u64,
    pub detail: Loadable<SeriesDetail>,
    pub seasons: Vec<Season>,
    /// The season whose episodes show.
    pub season: usize,
    /// The season the page was opened for, to land on once the seasons are known.
    wanted_season: Option<ItemId>,
    /// The episodes of each season asked, by the season's id.
    pub episodes: HashMap<ItemId, Loadable<Vec<Episode>>>,
    /// What the main button plays, and the version and tracks its chips chose.
    pub next: Option<Episode>,
    pub choices: Option<Choices>,
    /// The languages picked by hand for the last episode started here, for the next one.
    pub carried: Carried,
    pub menus: Menus<Chip>,
    pub menu_fade: Presence<Chip>,
    pub chips: [Rc<Cell<Option<Bounds<Pixels>>>>; 3],
    /// "My list".
    pub favourite: Toggle,
    pub scroll: ScrollHandle,
    /// The rows that scroll sideways: the cast, similar series, and those of the request server
    /// not in the library.
    pub rows: [ScrollHandle; 3],
    gesture: scroll::Gesture,
    pub error: Option<&'static str>,
    pub busy: bool,
    refreshes: Refreshes,
    /// The episode a notification opened the page for, until the page has scrolled to it, and
    /// the frames waited for the page to be laid out.
    pub in_view: Rc<RefCell<Option<(ItemId, u8)>>>,
}

impl ListPage for SeriesPage {
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

impl SeriesPage {
    /// A mark or a playback is on its way, or the refresh after a mark is: nothing else starts.
    pub fn waiting(&self) -> bool {
        self.busy || self.refreshes.holding()
    }

    /// The episodes of the season shown.
    pub fn shown_episodes(&self) -> Option<&Loadable<Vec<Episode>>> {
        self.episodes.get(&self.seasons.get(self.season)?.id)
    }
}

/// What a series page asks of the server each time it shows anew.
struct Fresh {
    detail: SeriesDetail,
    seasons: Vec<Season>,
    next: Option<Episode>,
}

impl Shell {
    pub fn open_series(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.open_series_on(id, None, cx);
    }

    /// The page of a series, opened on `season` (a card of a new season) when it has it.
    pub fn open_series_on(&mut self, id: ItemId, season: Option<ItemId>, cx: &mut Context<Self>) {
        self.forward.clear();
        // The card belongs to the page under: it does not play over this one.
        self.cancel_up_next(cx);
        let generation = self.next_generation();
        self.series_pages.push(SeriesPage {
            id,
            generation,
            detail: Loadable::Loading,
            seasons: Vec::new(),
            season: 0,
            wanted_season: season,
            episodes: HashMap::new(),
            next: None,
            choices: None,
            carried: Carried::default(),
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            chips: Default::default(),
            favourite: Toggle::default(),
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            gesture: Default::default(),
            error: None,
            busy: false,
            refreshes: Refreshes::default(),
            in_view: Rc::default(),
        });
        self.navigator.open(Screen::SeriesDetail);
        self.load_series(self.series_pages.len() - 1, true, None, cx);
        cx.notify();
    }

    /// The page at `depth` of the series pages, if it still shows `id`.
    pub(super) fn series_page(&mut self, depth: usize, id: &ItemId) -> Option<&mut SeriesPage> {
        self.series_pages
            .get_mut(depth)
            .filter(|page| &page.id == id)
    }

    /// The series page of that number at `depth`, if it is still open there: a read answers to
    /// the page that asked it, not to another that shows the same title.
    pub(super) fn series_numbered(
        &mut self,
        depth: usize,
        generation: u64,
    ) -> Option<&mut SeriesPage> {
        favourite::numbered(&mut self.series_pages, depth, generation)
    }

    /// Asks the series, its seasons and the episode to play next, then the episodes of the
    /// season shown. `first`: the page opens on the season of the next episode. `after`: the
    /// reports of a playback, sent before anything is asked.
    pub(super) fn load_series(
        &mut self,
        depth: usize,
        first: bool,
        after: Option<JoinHandle<()>>,
        cx: &mut Context<Self>,
    ) {
        let page = &mut self.series_pages[depth];
        let refresh = page.refreshes.ask(&mut self.refreshes_asked);
        let id = page.id.clone();
        let asked = id.clone();
        let generation = page.generation;
        let read_at = page.favourite.changes();
        let stamp = self.marks.stamp();
        self.ask(
            cx,
            move |server| {
                if let Some(reports) = after {
                    let _ = reports.join();
                }
                Ok::<_, ProviderError>(Fresh {
                    detail: server.series_detail(&asked)?,
                    seasons: server.seasons(&asked)?,
                    next: series_page::next_to_play(server, &asked)?,
                })
            },
            move |shell, fresh, cx| {
                let fresh = match fresh {
                    Ok(fresh) => fresh,
                    Err(error) => {
                        let said = shell.failed(error, cx);
                        if let Some(page) = shell.series_numbered(depth, generation)
                            && page.refreshes.landed(refresh)
                            && !matches!(page.detail, Loadable::Ready(_))
                        {
                            page.detail = Loadable::Failed(said);
                        }
                        return;
                    }
                };
                shell.series.grid.update(&fresh.detail.series);
                // Its episodes marked on this page change whether all of it is watched.
                let (series, marks) = (&fresh.detail.series, &mut shell.marks);
                marks.heard(stamp, &series.id, Action::MyList, fresh.detail.favourite);
                if let Some(watched) = series.watched {
                    marks.heard(stamp, &series.id, Action::Watched, watched);
                }
                marks.listed(stamp, &fresh.detail.similar);
                let Some(page) = shell
                    .series_numbered(depth, generation)
                    .filter(|page| page.refreshes.is_newest(refresh))
                else {
                    return;
                };
                page.choices = series_page::choices_after(
                    page.next.as_ref(),
                    page.choices.take(),
                    fresh.next.as_ref(),
                    &page.carried,
                );
                if first {
                    page.season = series_page::opening_season(
                        &fresh.seasons,
                        page.wanted_season.take().as_ref(),
                        fresh.next.as_ref(),
                    );
                }
                page.season = page.season.min(fresh.seasons.len().saturating_sub(1));
                page.next = fresh.next;
                page.seasons = fresh.seasons;
                page.favourite.sync(fresh.detail.favourite, read_at);
                let tmdb = fresh.detail.tmdb;
                page.detail = Loadable::Ready(fresh.detail);
                // What was watched may have changed in every season: the one shown is asked
                // again, and stays drawn as it was until it comes; the others are forgotten.
                let shown = page.seasons.get(page.season).map(|s| s.id.clone());
                page.episodes
                    .retain(|season, _| Some(season) == shown.as_ref());
                shell.load_episodes(depth, true, cx);
                shell.load_similar(bobine::requests::MediaKind::Tv, tmdb, cx);
            },
        );
    }

    /// The episodes of the season shown, when not asked yet or failed, or `again`.
    fn load_episodes(&mut self, depth: usize, again: bool, cx: &mut Context<Self>) {
        let Some(page) = self.series_pages.get_mut(depth) else {
            return;
        };
        let Some(season) = page.seasons.get(page.season).map(|s| s.id.clone()) else {
            // No season: the refresh has nothing more to land.
            let newest = page.refreshes.newest();
            page.refreshes.landed(newest);
            return;
        };
        if !series_page::asks_episodes(page.episodes.get(&season), again) {
            return;
        }
        // The list shown stays drawn until the new one comes; a failed one shows loading.
        if !matches!(page.episodes.get(&season), Some(Loadable::Ready(_))) {
            page.episodes.insert(season.clone(), Loadable::Loading);
        }
        let id = page.id.clone();
        // Episodes asked for an older refresh would show what was before it.
        let refresh = page.refreshes.newest();
        let (series, asked) = (id.clone(), season.clone());
        self.ask(
            cx,
            move |server| server.episodes(&series, &asked),
            move |shell, episodes, cx| {
                let episodes = match episodes {
                    Ok(episodes) => Loadable::Ready(episodes),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                };
                if let Some(page) = shell.series_page(depth, &id) {
                    // `again` is the refresh's own question; a tab click does not release.
                    series_page::land_episodes(
                        &mut page.episodes,
                        &mut page.refreshes,
                        refresh,
                        again,
                        season,
                        episodes,
                    );
                }
            },
        );
    }

    /// A click on a season's tab.
    pub fn select_season(&mut self, season: usize, cx: &mut Context<Self>) {
        let depth = self.series_pages.len().wrapping_sub(1);
        let Some(page) = self.series_pages.last_mut() else {
            return;
        };
        // The tab shown chosen again asks its episodes again if they failed.
        if season >= page.seasons.len() {
            return;
        }
        page.season = season;
        self.load_episodes(depth, false, cx);
        cx.notify();
    }

    /// The check of an episode: watched, or no longer.
    pub fn toggle_episode_watched(&mut self, episode: Episode, cx: &mut Context<Self>) {
        self.mark_series(
            move |server| film_page::toggle_watched(server, &episode.item).map(drop),
            cx,
        );
    }

    /// The season's check: all its episodes watched, or, when they all are, no longer.
    pub fn toggle_season_watched(&mut self, cx: &mut Context<Self>) {
        let Some(season) = self
            .series_pages
            .last()
            .and_then(|page| page.seasons.get(page.season).cloned())
        else {
            return;
        };
        self.mark_series(
            move |server| series_page::toggle_season_watched(server, &season),
            cx,
        );
    }

    /// Marks something watched on the server, then shows the page as it is now.
    fn mark_series(
        &mut self,
        mark: impl FnOnce(&dyn bobine::provider::MediaServer) -> Result<(), ProviderError>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        let depth = self.series_pages.len().wrapping_sub(1);
        let Some(page) = self.series_pages.last_mut() else {
            return;
        };
        if page.waiting() {
            return;
        }
        page.busy = true;
        page.error = None;
        let id = page.id.clone();
        self.ask(cx, mark, move |shell, marked, cx| {
            let marked = marked.map_err(|error| shell.failed(error, cx));
            let Some(page) = shell.series_page(depth, &id) else {
                return;
            };
            page.busy = false;
            match marked {
                Ok(()) => {
                    // The list shown is from before the mark until the refresh lands.
                    page.refreshes.hold();
                    shell.load_series(depth, false, None, cx);
                }
                Err(said) => page.error = Some(said),
            }
        });
    }

    /// A chip's choice, for the episode the main button plays.
    pub(super) fn choose_for_episode(&mut self, chip: Chip, value: Option<i32>) {
        let Some(page) = self.series_pages.last_mut() else {
            return;
        };
        page.menus.close();
        let (Some(next), Some(choices)) = (&page.next, &mut page.choices) else {
            return;
        };
        chip.choose(choices, &next.versions, value);
    }

    pub fn scroll_series_page(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.series_pages.last_mut() else {
            return;
        };
        if route_wheel(&page.scroll, &page.rows, &mut page.gesture, event, window) {
            page.menus.close();
            cx.notify();
        }
    }

    /// Plays an episode from where the viewer stopped: the one of the main button with the
    /// chips' choices, any other with its defaults. The episode after it is looked for at the
    /// same time, for the end.
    pub fn play_episode(&mut self, episode: Episode, window: &mut Window, cx: &mut Context<Self>) {
        self.play_episode_as(episode, false, window, cx);
    }

    /// `seamless`: it follows the episode that just ended, so the player does not open again.
    fn play_episode_as(
        &mut self,
        episode: Episode,
        seamless: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let depth = self.series_pages.len().wrapping_sub(1);
        let Some(page) = self.series_pages.last_mut() else {
            return;
        };
        // Another episode chosen while the card counts down: the card goes, unless the page
        // ignores the click.
        if !series_page::starts_episode(page.waiting(), &mut self.up_next) {
            self.forget_click();
            return;
        }
        self.up_next_ticker = None;
        let choices = series_page::choices_for(
            page.next.as_ref(),
            page.choices.as_ref(),
            &episode,
            &page.carried,
        );
        let Some(plan) = film_page::plan(&episode.item, &episode.versions, &choices, From::Resume)
        else {
            page.error = Some("This episode has no version to play.");
            cx.notify();
            self.forget_click();
            return;
        };
        let Some(server) = self.server.clone() else {
            return;
        };
        page.carried = choices.carried();
        page.busy = true;
        page.error = None;
        let origin = Origin::Series(depth, page.id.clone());
        let (title, mut subtitle) = series_page::player_title(&episode);
        if let Some(source) = choices.source(&episode.versions) {
            subtitle.push_str(&format!(" · {}", film_page::version_label(source)));
        }
        let playing = Playing {
            title,
            subtitle: Some(subtitle),
            list: Some(page.id.clone()),
            desktop: Some(crate::mpris::Title::episode(&episode)),
        };
        let quality = self.settings.default_quality.for_player();
        let asked = plan.clone();
        let task = cx
            .background_executor()
            .spawn(async move { series_page::open_episode(&*server, &episode, &asked, quality) });
        cx.spawn_in(window, async move |this, cx| {
            let (opened, after) = task.await;
            this.update_in(cx, |shell, window, cx| {
                let after = after.map(Next::Episode);
                shell.start_playback(origin, plan, opened, playing, after, seamless, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The end of an episode: the card counts down over the series page, then plays the next;
    /// the end of a playlist's title: over the playlist's page, the title after.
    pub(super) fn start_up_next(
        &mut self,
        next: Next,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.card_page(&next) else {
            return;
        };
        self.up_next.start(next, at, Instant::now());
        let ticker: Task<()> = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(UP_NEXT_TICK).await;
                let counting = this.update_in(cx, |shell, window, cx| {
                    cx.notify();
                    match shell.take_up_next(false) {
                        Some(next) => {
                            shell.play_next(next, window, cx);
                            false
                        }
                        None => shell.up_next.episode().is_some(),
                    }
                });
                if !matches!(counting, Ok(true)) {
                    break;
                }
            }
        });
        self.up_next_ticker = Some(ticker);
        cx.notify();
    }

    /// "Play" on the card: the next episode at once.
    pub fn play_up_next_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.take_up_next(true) {
            self.up_next_ticker = None;
            self.play_next(next, window, cx);
        }
    }

    /// What the card offered plays, on its page.
    pub(super) fn play_next(&mut self, next: Next, window: &mut Window, cx: &mut Context<Self>) {
        match next {
            Next::Episode(episode) => self.play_episode_as(episode, true, window, cx),
            Next::Entry(entry) => self.play_entry_as(entry, true, true, window, cx),
        }
    }

    /// The page on top the card of `next` belongs to: its depth, and what it shows.
    fn card_page(&self, next: &Next) -> Option<(usize, ItemId)> {
        match next {
            Next::Episode(_) => {
                let page = self.series_pages.last()?;
                Some((self.series_pages.len() - 1, page.id.clone()))
            }
            Next::Entry(_) => {
                let page = self.playlist_pages.last()?;
                Some((self.playlist_pages.len() - 1, page.id.clone()))
            }
        }
    }

    /// The card's title, when it is time to play it on the page it belongs to: the series page
    /// or the playlist's page shown, unless it waits.
    fn take_up_next(&mut self, now_asked: bool) -> Option<Next> {
        let (top, waiting) = match self.navigator.current() {
            Screen::SeriesDetail => {
                let page = self.series_pages.last();
                (
                    page.map(|page| (self.series_pages.len() - 1, &page.id)),
                    page.is_some_and(SeriesPage::waiting),
                )
            }
            Screen::Playlist => {
                let page = self.playlist_pages.last();
                (
                    page.map(|page| (self.playlist_pages.len() - 1, &page.id)),
                    page.is_some_and(|page| page.busy),
                )
            }
            _ => (None, false),
        };
        self.up_next.take(Instant::now(), now_asked, top, waiting)
    }

    /// "Cancel" on the card, or Escape. Whether there was a card to cancel.
    pub fn cancel_up_next(&mut self, cx: &mut Context<Self>) -> bool {
        if self.up_next.episode().is_none() {
            return false;
        }
        self.up_next.cancel();
        self.up_next_ticker = None;
        cx.notify();
        true
    }
}
