//! The films of a library as the grid shows them: how many columns fit, and the pages asked of
//! the server as rows come into view. A library may hold thousands of films: only the pages of
//! the visible rows are asked, each once.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::time::{Duration, Instant};

use bobine::provider::{Film, ItemId, Page, Series};

/// Films per request.
pub const PAGE: u32 = 100;
/// The grid's margin, left and right, and the gap between two posters.
pub const MARGIN: f32 = 40.;
pub const GAP: f32 = 16.;
const MAX_COLUMNS: usize = 8;
const MIN_POSTER: f32 = 150.;
/// A page that failed is asked again no sooner than this, and only this many times: a server
/// answering 500 must not be asked at every render.
pub const RETRY_AFTER: Duration = Duration::from_secs(5);
pub const MAX_RETRIES: u32 = 3;

/// Eight columns at 1440 px wide, fewer when narrower, never less than two.
pub fn columns(width: f32) -> usize {
    let room = width - 2. * MARGIN + GAP;
    ((room / (MIN_POSTER + GAP)).floor() as usize).clamp(2, MAX_COLUMNS)
}

/// `1 284 titres`, with the narrow space French puts between thousands.
pub fn count(total: u32) -> String {
    let digits = total.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{202f}');
        }
        grouped.push(digit);
    }
    let noun = if total > 1 { "titres" } else { "titre" };
    format!("{grouped} {noun}")
}

/// The row at the top of a list scrolled by `scrolled` pixels. `contents` is the height of all
/// the rows together: one row is a share of it, not the height of what is in view.
pub fn top_row(scrolled: f32, contents: f32, rows: usize) -> usize {
    if rows == 0 || contents <= 0. || scrolled <= 0. {
        return 0;
    }
    ((scrolled / (contents / rows as f32)) as usize).min(rows - 1)
}

/// What a grid shows: films or series.
pub trait Listed: Clone {
    fn id(&self) -> &ItemId;
    fn title(&self) -> &str;
}

impl Listed for Film {
    fn id(&self) -> &ItemId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.title
    }
}

impl Listed for Series {
    fn id(&self) -> &ItemId {
        &self.id
    }

    fn title(&self) -> &str {
        &self.title
    }
}

pub struct Grid<T> {
    /// Known once the first page came back.
    pub total: Option<u32>,
    /// Which list this is: it changes each time the grid starts over, so that an answer meant
    /// for the list before it is told apart and dropped.
    epoch: u64,
    pages: HashMap<u32, Vec<T>>,
    asked: HashSet<u32>,
    /// How many times a page failed, and when it last did.
    failures: HashMap<u32, (u32, Instant)>,
    /// The list before the last restart, kept on screen until the new one takes its place.
    stale: Option<Stale<T>>,
}

struct Stale<T> {
    total: u32,
    pages: HashMap<u32, Vec<T>>,
}

impl<T> Default for Grid<T> {
    fn default() -> Self {
        Self {
            total: None,
            epoch: 0,
            pages: HashMap::new(),
            asked: HashSet::new(),
            failures: HashMap::new(),
            stale: None,
        }
    }
}

impl<T: Listed> Grid<T> {
    /// Starts over from the top, for new filters or a new order: nothing of the old list stays,
    /// and the answers still on their way for it will not be taken.
    pub fn restart(&mut self) {
        self.epoch += 1;
        // What is on screen stays there, to fade out; if it is already leaving, it keeps leaving.
        if let (Some(total), None) = (self.total.filter(|total| *total > 0), &self.stale) {
            self.stale = Some(Stale {
                total,
                pages: std::mem::take(&mut self.pages),
            });
        }
        self.total = None;
        self.pages.clear();
        self.asked.clear();
        self.failures.clear();
    }

    /// Whether the list before the last restart is still the one on screen.
    pub fn has_stale(&self) -> bool {
        self.stale.is_some()
    }

    /// The new list takes the old one's place.
    pub fn drop_stale(&mut self) {
        self.stale = None;
    }

    /// The size of the list on screen: the old one while it leaves, else the current one.
    pub fn shown_total(&self) -> Option<u32> {
        self.stale
            .as_ref()
            .map_or(self.total, |old| Some(old.total))
    }

    pub fn shown_rows(&self, columns: usize) -> usize {
        match &self.stale {
            Some(old) => (old.total as usize).div_ceil(columns.max(1)),
            None => self.rows(columns),
        }
    }

    pub fn shown_film(&self, index: usize) -> Option<&T> {
        let Some(old) = &self.stale else {
            return self.film(index);
        };
        let page = (index / PAGE as usize) as u32;
        old.pages.get(&page)?.get(index % PAGE as usize)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Whether an answer asked for under `epoch` is still for the list shown.
    pub fn is_current(&self, epoch: u64) -> bool {
        epoch == self.epoch
    }

    pub fn rows(&self, columns: usize) -> usize {
        (self.total.unwrap_or(0) as usize).div_ceil(columns.max(1))
    }

    pub fn film(&self, index: usize) -> Option<&T> {
        let page = (index / PAGE as usize) as u32;
        self.pages.get(&page)?.get(index % PAGE as usize)
    }

    /// The pages holding `films` (indexes) not asked yet; they count as asked from now on.
    pub fn pages_to_ask(&mut self, films: Range<usize>, now: Instant) -> Vec<u32> {
        if films.is_empty() {
            return Vec::new();
        }
        let first = (films.start / PAGE as usize) as u32;
        let last = ((films.end - 1) / PAGE as usize) as u32;
        (first..=last)
            .filter(|page| self.may_ask(*page, now) && self.asked.insert(*page))
            .collect()
    }

    fn may_ask(&self, page: u32, now: Instant) -> bool {
        match self.failures.get(&page) {
            None => true,
            Some(&(failures, at)) => failures <= MAX_RETRIES && now >= at + RETRY_AFTER,
        }
    }

    pub fn loaded(&mut self, epoch: u64, page: u32, films: Page<T>) {
        if !self.is_current(epoch) {
            return;
        }
        self.failures.remove(&page);
        self.total = Some(films.total);
        self.pages.insert(page, films.items);
    }

    /// Whether the page failed so often that it is not asked any more.
    pub fn gave_up(&self, page: u32) -> bool {
        self.failures
            .get(&page)
            .is_some_and(|&(failures, _)| failures > MAX_RETRIES)
    }

    /// The failures are forgotten: the pages that gave up are asked again at once.
    pub fn forget_failures(&mut self) {
        self.failures.clear();
    }

    /// The page may be asked again, a few times, once `RETRY_AFTER` has passed.
    pub fn failed(&mut self, epoch: u64, page: u32, now: Instant) {
        if !self.is_current(epoch) {
            return;
        }
        self.asked.remove(&page);
        let failures = self.failures.get(&page).map_or(0, |&(count, _)| count);
        self.failures.insert(page, (failures + 1, now));
    }

    /// A film or a series changed (watched, resume point, episodes left): the grid shows it as
    /// it is now.
    pub fn update(&mut self, film: &T) {
        for page in self.pages.values_mut() {
            for found in page.iter_mut().filter(|f| f.id() == film.id()) {
                *found = film.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeServer;

    fn loaded_grid(total: u32) -> Grid<Film> {
        let server = FakeServer::new(total as usize, true);
        let mut grid = Grid::<Film>::default();
        grid.pages_to_ask(0..8, Instant::now());
        grid.loaded(grid.epoch(), 0, some(&server, 0));
        grid
    }

    #[test]
    fn the_old_list_stays_on_screen_when_the_grid_starts_over() {
        let mut grid = loaded_grid(1284);
        let first = grid.film(0).cloned().unwrap();
        grid.restart();
        assert_eq!(grid.total, None);
        assert!(grid.has_stale());
        assert_eq!(grid.shown_total(), Some(1284));
        assert_eq!(grid.shown_rows(8), 161);
        assert_eq!(grid.shown_film(0).map(|f| &f.id), Some(&first.id));
        // What is asked and answered is the new list: nothing of the old one is in it.
        assert!(grid.film(0).is_none());
    }

    #[test]
    fn the_new_list_waits_to_take_the_place_of_the_old_one() {
        let mut grid = loaded_grid(1284);
        grid.restart();
        grid.pages_to_ask(0..8, Instant::now());
        grid.loaded(grid.epoch(), 0, some(&FakeServer::new(40, true), 0));
        assert_eq!(grid.total, Some(40));
        assert_eq!(grid.shown_total(), Some(1284));
        grid.drop_stale();
        assert!(!grid.has_stale());
        assert_eq!(grid.shown_total(), Some(40));
        assert_eq!(grid.shown_rows(8), 5);
        assert!(grid.shown_film(0).is_some());
    }

    #[test]
    fn starting_over_twice_keeps_showing_the_list_on_screen() {
        let mut grid = loaded_grid(1284);
        grid.restart();
        grid.restart();
        assert_eq!(grid.shown_total(), Some(1284));
    }

    #[test]
    fn a_list_with_nothing_in_it_leaves_nothing_to_fade() {
        let mut grid = Grid::<Film>::default();
        grid.pages_to_ask(0..8, Instant::now());
        grid.loaded(
            grid.epoch(),
            0,
            Page {
                items: Vec::new(),
                total: 0,
            },
        );
        grid.restart();
        assert!(!grid.has_stale());
    }

    #[test]
    fn a_grid_that_showed_nothing_has_nothing_to_keep() {
        let mut grid = Grid::<Film>::default();
        grid.restart();
        assert!(!grid.has_stale());
        assert_eq!(grid.shown_total(), None);
    }

    #[test]
    fn the_top_row_is_the_scroll_over_one_rows_height() {
        // 100 rows of 300 px: scrolled to the 61st row's top.
        assert_eq!(top_row(60. * 300., 100. * 300., 100), 60);
        assert_eq!(top_row(60. * 300. + 299., 100. * 300., 100), 60);
    }

    #[test]
    fn the_top_row_is_the_first_before_any_scroll_or_measure() {
        assert_eq!(top_row(0., 30_000., 100), 0);
        assert_eq!(top_row(-12., 30_000., 100), 0);
        assert_eq!(top_row(500., 0., 0), 0);
        assert_eq!(top_row(500., 0., 10), 0);
    }

    #[test]
    fn the_top_row_never_passes_the_last_one() {
        assert_eq!(top_row(1.0e9, 30_000., 100), 99);
    }

    use crate::library_tools::Tools;
    use bobine::provider::MediaServer;

    fn query(page: u32) -> bobine::provider::FilmQuery {
        Tools::default().query(page)
    }

    fn some(server: &FakeServer, page: u32) -> Page<Film> {
        server
            .films(&ItemId(crate::fake::FILMS_LIBRARY.into()), &query(page))
            .unwrap()
    }

    #[test]
    fn eight_columns_at_1440_fewer_when_narrower() {
        assert_eq!(columns(1440.), 8);
        assert_eq!(columns(1920.), 8);
        assert_eq!(columns(1100.), 6);
        assert_eq!(columns(800.), 4);
        assert_eq!(columns(300.), 2);
    }

    #[test]
    fn the_count_reads_in_french() {
        assert_eq!(count(1284), "1\u{202f}284 titres");
        assert_eq!(count(1), "1 titre");
        assert_eq!(count(1234567), "1\u{202f}234\u{202f}567 titres");
    }

    #[test]
    fn only_the_pages_of_the_visible_rows_are_asked_once() {
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        assert_eq!(grid.pages_to_ask(0..24, now), [0]);
        assert_eq!(grid.pages_to_ask(0..24, now), Vec::<u32>::new());
        assert_eq!(grid.pages_to_ask(96..120, now), [1]);
        assert_eq!(grid.pages_to_ask(2000..2024, now), [20]);
    }

    #[test]
    fn a_failed_page_is_not_asked_again_at_every_render() {
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        assert_eq!(grid.pages_to_ask(0..8, now), [0]);
        grid.failed(grid.epoch(), 0, now);
        // The renders that follow the failure, one frame apart.
        for frame in 1..=100 {
            let later = now + Duration::from_millis(16 * frame);
            assert_eq!(grid.pages_to_ask(0..8, later), Vec::<u32>::new());
        }
    }

    #[test]
    fn a_failed_page_is_asked_again_after_the_delay() {
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        grid.pages_to_ask(0..8, now);
        grid.failed(grid.epoch(), 0, now);
        let after = now + RETRY_AFTER;
        assert_eq!(grid.pages_to_ask(0..8, after), [0]);
    }

    #[test]
    fn a_page_that_always_fails_is_given_up_after_a_few_retries() {
        let mut grid = Grid::<Film>::default();
        let mut now = Instant::now();
        let mut asks = 0;
        for _ in 0..50 {
            asks += grid.pages_to_ask(0..8, now).len();
            grid.failed(grid.epoch(), 0, now);
            now += RETRY_AFTER;
        }
        assert_eq!(asks, 1 + MAX_RETRIES as usize);
    }

    #[test]
    fn a_page_that_failed_does_not_hold_back_the_others() {
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        grid.pages_to_ask(0..8, now);
        grid.failed(grid.epoch(), 0, now);
        assert_eq!(grid.pages_to_ask(0..220, now), [1, 2]);
    }

    #[test]
    fn thousands_of_films_make_rows_from_the_total_and_show_as_pages_come() {
        let server = FakeServer::new(1284, true);
        let mut grid = Grid::<Film>::default();
        assert_eq!(grid.rows(8), 0);
        let library = ItemId(crate::fake::FILMS_LIBRARY.into());
        grid.loaded(0, 0, server.films(&library, &query(0)).unwrap());
        assert_eq!(grid.rows(8), 161);
        assert_eq!(grid.film(99).map(|f| f.id.0.as_str()), Some("film-99"));
        assert!(grid.film(100).is_none());
        grid.loaded(0, 12, server.films(&library, &query(12)).unwrap());
        assert_eq!(grid.film(1283).map(|f| f.id.0.as_str()), Some("film-1283"));
    }

    #[test]
    fn changing_a_filter_restarts_the_paging_from_the_top() {
        let server = FakeServer::new(1284, true);
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        grid.pages_to_ask(0..8, now);
        grid.loaded(grid.epoch(), 0, some(&server, 0));
        grid.pages_to_ask(1000..1008, now);
        grid.loaded(grid.epoch(), 10, some(&server, 10));
        assert_eq!(grid.total, Some(1284));

        grid.restart();
        assert_eq!(grid.total, None);
        assert_eq!(grid.rows(8), 0);
        assert!(grid.film(0).is_none() && grid.film(1000).is_none());
        // The pages asked before are asked again, for the new list.
        assert_eq!(grid.pages_to_ask(0..8, now), [0]);
    }

    #[test]
    fn an_older_answer_never_lands_into_the_new_list() {
        let server = FakeServer::new(1284, true);
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        grid.pages_to_ask(0..8, now);
        let before = grid.epoch();

        grid.restart();
        assert_ne!(grid.epoch(), before);
        grid.loaded(before, 0, some(&server, 0));
        assert_eq!(grid.total, None);
        assert!(grid.film(0).is_none());

        grid.pages_to_ask(0..8, now);
        grid.loaded(grid.epoch(), 0, some(&server, 0));
        assert_eq!(grid.total, Some(1284));
    }

    #[test]
    fn a_failure_of_the_old_list_does_not_hold_back_the_new_one() {
        let mut grid = Grid::<Film>::default();
        let now = Instant::now();
        grid.pages_to_ask(0..8, now);
        let before = grid.epoch();
        grid.restart();
        assert_eq!(grid.pages_to_ask(0..8, now), [0]);

        // The old page 0 fails late: the new page 0 is still on its way, not to be asked twice,
        // nor counted as failed.
        grid.failed(before, 0, now);
        assert_eq!(
            grid.pages_to_ask(0..8, now + RETRY_AFTER),
            Vec::<u32>::new()
        );
        grid.failed(grid.epoch(), 0, now);
        assert_eq!(grid.pages_to_ask(0..8, now + RETRY_AFTER), [0]);
    }

    #[test]
    fn an_answer_is_taken_only_for_the_list_it_was_asked_for() {
        let mut grid = Grid::<Film>::default();
        let asked = grid.epoch();
        assert!(grid.is_current(asked));
        grid.restart();
        assert!(!grid.is_current(asked));
        assert!(grid.is_current(grid.epoch()));
    }
}
