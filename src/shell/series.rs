//! The series grid and the series page: what they show, how it comes from the server, and the
//! episodes they play. Like the film page, every call to the server runs off the window's thread.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bobine::provider::{Episode, ItemId, ProviderError, Season, SeriesDetail};
use gpui_kit::{Bounds, Context, Pixels, ScrollHandle, ScrollWheelEvent, Task, Window};

use super::pages::{Chip, Loadable, Origin, route_wheel};
use super::{Film as Playing, Shell};
use crate::film_page::{self, Choices, From};
use crate::menu::Menus;
use crate::motion::{self, Presence};
use crate::navigation::Screen;
use crate::scroll;
use crate::series_page;

/// How often the "Épisode suivant" card checks its countdown.
const UP_NEXT_TICK: Duration = Duration::from_millis(250);

pub struct SeriesPage {
    pub id: ItemId,
    pub detail: Loadable<SeriesDetail>,
    pub seasons: Vec<Season>,
    /// The season whose episodes show.
    pub season: usize,
    /// The episodes of each season asked, by the season's id.
    pub episodes: HashMap<ItemId, Loadable<Vec<Episode>>>,
    /// What the main button plays, and the version and tracks its chips chose.
    pub next: Option<Episode>,
    pub choices: Option<Choices>,
    pub menus: Menus<Chip>,
    pub menu_fade: Presence<Chip>,
    pub chips: [Rc<Cell<Option<Bounds<Pixels>>>>; 3],
    pub scroll: ScrollHandle,
    /// The rows that scroll sideways: the cast, similar series.
    pub rows: [ScrollHandle; 2],
    gesture: scroll::Gesture,
    pub error: Option<&'static str>,
    pub busy: bool,
}

impl SeriesPage {
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
        self.series_pages.push(SeriesPage {
            id,
            detail: Loadable::Loading,
            seasons: Vec::new(),
            season: 0,
            episodes: HashMap::new(),
            next: None,
            choices: None,
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            chips: Default::default(),
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            gesture: Default::default(),
            error: None,
            busy: false,
        });
        self.navigator.open(Screen::SeriesDetail);
        self.load_series(self.series_pages.len() - 1, true, None, cx);
        cx.notify();
    }

    /// The page at `depth` of the series pages, if it still shows `id`.
    fn series_page(&mut self, depth: usize, id: &ItemId) -> Option<&mut SeriesPage> {
        self.series_pages
            .get_mut(depth)
            .filter(|page| &page.id == id)
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
        let id = self.series_pages[depth].id.clone();
        let asked = id.clone();
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
                        if let Some(page) = shell.series_page(depth, &id)
                            && !matches!(page.detail, Loadable::Ready(_))
                        {
                            page.detail = Loadable::Failed(said);
                        }
                        return;
                    }
                };
                shell.series.grid.update(&fresh.detail.series);
                let Some(page) = shell.series_page(depth, &id) else {
                    return;
                };
                // The chips keep their choice while the episode to play stays the same.
                let same = page.next.as_ref().map(|e| &e.item.id)
                    == fresh.next.as_ref().map(|e| &e.item.id);
                if !same || page.choices.is_none() {
                    page.choices = fresh.next.as_ref().map(|e| Choices::new(&e.versions));
                }
                if first {
                    page.season = fresh
                        .next
                        .as_ref()
                        .and_then(|next| fresh.seasons.iter().position(|s| s.id == next.season))
                        .unwrap_or(0);
                }
                page.season = page.season.min(fresh.seasons.len().saturating_sub(1));
                page.next = fresh.next;
                page.seasons = fresh.seasons;
                page.detail = Loadable::Ready(fresh.detail);
                // What was watched may have changed in every season: the one shown is asked
                // again, and stays drawn as it was until it comes; the others are forgotten.
                let shown = page.seasons.get(page.season).map(|s| s.id.clone());
                page.episodes
                    .retain(|season, _| Some(season) == shown.as_ref());
                shell.load_episodes(depth, true, cx);
            },
        );
    }

    /// The episodes of the season shown, when not asked yet, or `again`.
    fn load_episodes(&mut self, depth: usize, again: bool, cx: &mut Context<Self>) {
        let Some(page) = self.series_pages.get_mut(depth) else {
            return;
        };
        let Some(season) = page.seasons.get(page.season).map(|s| s.id.clone()) else {
            return;
        };
        if page.episodes.contains_key(&season) && !again {
            return;
        }
        page.episodes
            .entry(season.clone())
            .or_insert(Loadable::Loading);
        let id = page.id.clone();
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
                    page.episodes.insert(season, episodes);
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
        if season >= page.seasons.len() || season == page.season {
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
        if page.busy {
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
                Ok(()) => shell.load_series(depth, false, None, cx),
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
        match chip {
            Chip::Version => choices.set_version(&next.versions, value.unwrap_or(0) as usize),
            Chip::Audio => choices.audio = value,
            Chip::Subtitle => choices.subtitle = value,
        }
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
        let depth = self.series_pages.len().wrapping_sub(1);
        let Some(page) = self.series_pages.last_mut() else {
            return;
        };
        if page.busy {
            return;
        }
        let choices = match (&page.next, &page.choices) {
            (Some(next), Some(choices)) if next.item.id == episode.item.id => choices.clone(),
            _ => Choices::new(&episode.versions),
        };
        let Some(plan) = film_page::plan(&episode.item, &episode.versions, &choices, From::Resume)
        else {
            page.error = Some("Cet épisode n'a aucune version à lire.");
            cx.notify();
            return;
        };
        let Some(server) = self.server.clone() else {
            return;
        };
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
        };
        let (film, source) = (plan.film.clone(), plan.source.clone());
        let task = cx.background_executor().spawn(async move {
            let opened = server.play(&film, &source);
            // Without it the end shows no card: the playback goes on all the same.
            let after = series_page::following(&*server, &episode).ok().flatten();
            (opened, after)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (opened, after) = task.await;
            this.update_in(cx, |shell, window, cx| {
                shell.start_playback(origin, plan, opened, playing, after, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// The end of an episode: the card counts down over the series page, then plays the next.
    pub(super) fn start_up_next(
        &mut self,
        episode: Episode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.up_next.start(episode, Instant::now());
        let ticker: Task<()> = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(UP_NEXT_TICK).await;
                let counting = this.update_in(cx, |shell, window, cx| {
                    cx.notify();
                    if shell.navigator.current() != Screen::SeriesDetail {
                        shell.up_next.cancel();
                    }
                    match shell.up_next.take(Instant::now(), false) {
                        Some(episode) => {
                            shell.play_episode(episode, window, cx);
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

    /// "Lire" on the card: the next episode at once.
    pub fn play_up_next_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(episode) = self.up_next.take(Instant::now(), true) {
            self.up_next_ticker = None;
            self.play_episode(episode, window, cx);
        }
    }

    /// "Annuler" on the card, or Escape. Whether there was a card to cancel.
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
