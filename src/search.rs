//! The logic of the Ctrl+K search palette: when a query is sent to the server, which answer is
//! kept, the groups of results, the selection and what the keys do. No window and no clock here:
//! the view reads it, and the tests drive it with a fake server and chosen instants.

use std::time::{Duration, Instant};

use bobine::provider::{Film, PersonHit, SearchResults, SeriesHit};

use crate::film_page;
use crate::navigation::Screen;

/// How long the query must stay unchanged before the server is asked.
pub const DEBOUNCE: Duration = Duration::from_millis(250);

/// The most results of one group the server is asked for.
pub const PER_GROUP: u32 = 5;

/// A group's heading and a result's row have a fixed height: the view lays them out by hand, so
/// the selected row can be found, scrolled to and highlighted without measuring anything.
pub const HEADING_HEIGHT: f32 = 34.;
pub const ROW_HEIGHT: f32 = 120.;
/// The space kept between two rows, under each one.
pub const ROW_GAP: f32 = 4.;
/// A film's or series' poster in a row (2:3), and a person's round photo.
pub const POSTER_HEIGHT: f32 = 100.;
pub const POSTER_WIDTH: f32 = 67.;
pub const PERSON_PHOTO: f32 = 84.;
/// The "Enter" key, drawn at the right end of the highlight.
pub const HINT_HEIGHT: f32 = 18.;
pub const ENTER_KEY: &str = "Enter";
/// A key cap's own sizes: its drawing and the room a row keeps for it both read these.
pub const KEY_TEXT: f32 = 10.;
pub const KEY_PAD_X: f32 = 5.;
pub const KEY_PAD_Y: f32 = 1.;
/// The cap's outline, `border_1`.
pub const KEY_BORDER: f32 = 1.;
/// The space around the list of rows, which the highlight and the rows stay inside.
pub const LIST_PAD: f32 = 10.;
/// How far inside the highlight's right edge the "Enter" key ends.
pub const HINT_INSET: f32 = 10.;
/// What a row keeps free between its text and the key.
const HINT_GAP: f32 = 8.;
/// What a glyph of the interface font is at most wide, in em: the text is only measured when
/// drawn, and a row has to reserve its room before.
const GLYPH_EM: f32 = 0.6;

/// How wide a key cap is for `label`.
pub fn key_cap_width(label: &str) -> f32 {
    label.chars().count() as f32 * KEY_TEXT * GLYPH_EM + 2. * (KEY_PAD_X + KEY_BORDER)
}

/// What a row keeps free at its right for the highlight's "Enter" key, counted from the row's
/// edge, which is the highlight's.
pub fn hint_room() -> f32 {
    HINT_INSET + key_cap_width(ENTER_KEY) + HINT_GAP
}

/// The height of the highlight, which is a row without the space under it.
pub fn highlight_height() -> f32 {
    ROW_HEIGHT - ROW_GAP
}

/// Where the "Enter" key starts when the highlight starts at `highlight_top`: always centred
/// in it, so it travels with the highlight, whatever the highlight's glide gives.
pub fn hint_top(highlight_top: f32) -> f32 {
    highlight_top + (highlight_height() - HINT_HEIGHT) / 2.
}

/// Where the palette starts, from the top of the window.
pub const PANEL_TOP: f32 = 110.;
/// The field and the footer, which come with any list, and room below the palette.
pub const CHROME: f32 = 60. + 44. + 32.;
/// The list of results, at the most, before it scrolls.
pub const LIST_MAX: f32 = 500.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Films,
    Series,
    People,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Films => "Films",
            Group::Series => "Series",
            Group::People => "People",
        }
    }
}

/// One result.
#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    Film(Box<Film>),
    Series(SeriesHit),
    Person(PersonHit),
}

impl Hit {
    pub fn title(&self) -> &str {
        match self {
            Hit::Film(film) => &film.title,
            Hit::Series(series) => &series.title,
            Hit::Person(person) => &person.name,
        }
    }

    /// The line under the title: year, rating and where the viewer is for a film, the year for
    /// a series. A search gives no role for a person: a person has no line.
    pub fn caption(&self) -> String {
        match self {
            Hit::Film(film) => {
                let mut parts: Vec<String> = film.year.iter().map(i32::to_string).collect();
                parts.extend(film_page::rating(film));
                if film.watched {
                    parts.push("watched".into());
                } else if film.resume.is_some() {
                    parts.push("started".into());
                }
                parts.join(" · ")
            }
            Hit::Series(series) => series.year.map(|year| year.to_string()).unwrap_or_default(),
            Hit::Person(_) => String::new(),
        }
    }
}

/// What the palette shows besides the results.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Status {
    /// Nothing typed: nothing shown.
    Idle,
    /// A query is waiting for its answer (or for the user to stop typing).
    Searching,
    Found,
    Failed(&'static str),
}

/// A query to send to the server. `ticket` comes back with the answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub ticket: u64,
    pub query: String,
}

/// What a key did that the view must act on.
#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    /// Nothing the view must do (the key is not for the palette, or the selection moved).
    Ignored,
    Moved,
    Open(Hit),
    Close,
}

/// One line of the results as the view lays it out.
#[derive(Clone, Debug, PartialEq)]
pub enum Line {
    Heading(Group),
    /// A result, with its place in the selection order.
    Hit(Hit, usize),
}

#[derive(Debug)]
pub struct Palette {
    /// What is searched, trimmed.
    query: String,
    /// When the query last changed, while its search has not gone out.
    typed: Option<Instant>,
    /// The ticket of the last search sent: an answer to any other is out of date.
    ticket: u64,
    status: Status,
    /// The last answer kept. It stays on show while the next one is awaited.
    results: SearchResults,
    selected: usize,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            query: String::new(),
            typed: None,
            ticket: 0,
            status: Status::Idle,
            results: SearchResults::default(),
            selected: 0,
        }
    }
}

impl Palette {
    /// The text of the field changed.
    pub fn typed(&mut self, text: &str, now: Instant) {
        let query = text.trim();
        if query == self.query {
            return;
        }
        self.query = query.to_string();
        if query.is_empty() {
            // Nothing to show, nothing to ask, and whatever is on its way is out of date.
            self.typed = None;
            self.ticket += 1;
            self.status = Status::Idle;
            self.results = SearchResults::default();
            self.selected = 0;
            return;
        }
        self.typed = Some(now);
        self.status = Status::Searching;
    }

    /// When the user will have stopped typing, if a query is waiting for that.
    pub fn due(&self) -> Option<Instant> {
        self.typed.map(|typed| typed + DEBOUNCE)
    }

    /// The search to send now, if the user has stopped typing.
    pub fn ask(&mut self, now: Instant) -> Option<Ask> {
        if self.due()? > now {
            return None;
        }
        self.typed = None;
        self.ticket += 1;
        Some(Ask {
            ticket: self.ticket,
            query: self.query.clone(),
        })
    }

    /// The server answered `ask`. Only the answer to the latest search of the query still typed
    /// is kept: a slower answer to an older query never replaces a newer one.
    pub fn answered(&mut self, ask: &Ask, answer: Result<SearchResults, &'static str>) -> bool {
        if ask.ticket != self.ticket || ask.query != self.query || self.typed.is_some() {
            return false;
        }
        match answer {
            Ok(results) => {
                self.results = results;
                self.status = Status::Found;
                self.selected = 0;
            }
            Err(why) => self.status = Status::Failed(why),
        }
        true
    }

    /// The results in the order the arrows go through them: films, series, people.
    pub fn hits(&self) -> Vec<Hit> {
        let results = &self.results;
        results
            .films
            .iter()
            .cloned()
            .map(|film| Hit::Film(Box::new(film)))
            .chain(results.series.iter().cloned().map(Hit::Series))
            .chain(results.people.iter().cloned().map(Hit::Person))
            .collect()
    }

    /// The results with the headings of the groups that have any.
    pub fn lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        let mut next = 0;
        for (group, hits) in [
            (
                Group::Films,
                self.results
                    .films
                    .iter()
                    .cloned()
                    .map(|film| Hit::Film(Box::new(film)))
                    .collect::<Vec<_>>(),
            ),
            (
                Group::Series,
                self.results
                    .series
                    .iter()
                    .cloned()
                    .map(Hit::Series)
                    .collect(),
            ),
            (
                Group::People,
                self.results
                    .people
                    .iter()
                    .cloned()
                    .map(Hit::Person)
                    .collect(),
            ),
        ] {
            if hits.is_empty() {
                continue;
            }
            lines.push(Line::Heading(group));
            for hit in hits {
                lines.push(Line::Hit(hit, next));
                next += 1;
            }
        }
        lines
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// The mouse is over a result.
    pub fn select(&mut self, index: usize) {
        if index < self.hits().len() {
            self.selected = index;
        }
    }

    fn step(&mut self, forward: bool) {
        let count = self.hits().len();
        if count == 0 {
            return;
        }
        self.selected = if forward {
            (self.selected + 1) % count
        } else {
            (self.selected + count - 1) % count
        };
    }

    pub fn selected_hit(&self) -> Option<Hit> {
        self.hits().into_iter().nth(self.selected)
    }

    /// The keys of the palette: the arrows move, Enter opens, Escape closes.
    pub fn key(&mut self, key: &str) -> Key {
        match key {
            "down" => self.step(true),
            "up" => self.step(false),
            "enter" => {
                return match self.selected_hit() {
                    Some(hit) => Key::Open(hit),
                    None => Key::Ignored,
                };
            }
            "escape" => return Key::Close,
            _ => return Key::Ignored,
        }
        Key::Moved
    }

    /// What to say instead of results, if anything.
    pub fn message(&self) -> Option<String> {
        if !self.hits().is_empty() {
            return None;
        }
        match &self.status {
            Status::Idle => None,
            Status::Searching => Some("Searching…".into()),
            Status::Found => Some(format!("No results for “{}”", self.query)),
            Status::Failed(why) => Some((*why).into()),
        }
    }

    /// The failure of the last search, shown over the results kept.
    pub fn failure(&self) -> Option<&'static str> {
        match self.status {
            Status::Failed(why) if !self.hits().is_empty() => Some(why),
            _ => None,
        }
    }
}

/// Where the lines of the results start, from the top of the list, for the line at `lines` index.
pub fn line_top(lines: &[Line], index: usize) -> f32 {
    lines[..index.min(lines.len())]
        .iter()
        .map(|line| match line {
            Line::Heading(_) => HEADING_HEIGHT,
            Line::Hit(..) => ROW_HEIGHT,
        })
        .sum()
}

/// The height of all the lines.
pub fn lines_height(lines: &[Line]) -> f32 {
    line_top(lines, lines.len())
}

/// Where the result number `selected` starts in the list.
pub fn row_top(lines: &[Line], selected: usize) -> Option<f32> {
    let at = lines
        .iter()
        .position(|line| matches!(line, Line::Hit(_, n) if *n == selected))?;
    Some(line_top(lines, at))
}

/// How far the list must be scrolled for the selected row to show whole in a view `view` tall,
/// given how far it is scrolled now. A group's first row brings its heading when it scrolls up.
pub fn scroll_to_show(lines: &[Line], selected: usize, scrolled: f32, view: f32) -> f32 {
    let Some(top) = row_top(lines, selected) else {
        return 0.;
    };
    let bottom = top + ROW_HEIGHT;
    let heading_above = lines
        .iter()
        .position(|line| matches!(line, Line::Hit(_, n) if *n == selected))
        .and_then(|at| at.checked_sub(1))
        .is_some_and(|before| matches!(lines[before], Line::Heading(_)));
    let top = if heading_above {
        top - HEADING_HEIGHT
    } else {
        top
    };
    let max = (lines_height(lines) - view).max(0.);
    let wanted = if top < scrolled {
        top
    } else if bottom > scrolled + view {
        bottom - view
    } else {
        scrolled
    };
    wanted.clamp(0., max)
}

/// Ctrl+K, with nothing else held.
pub fn is_shortcut(key: &str, control: bool, others: bool) -> bool {
    control && !others && key == "k"
}

/// Whether the palette opens over `screen`: not on the login (no server yet) nor in the player.
pub fn opens_from(screen: Screen) -> bool {
    !matches!(screen, Screen::Login | Screen::Player)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeServer, PASSWORD, USER};
    use crate::motion::Glide;
    use bobine::provider::{Connector, MediaServer};
    use std::sync::Arc;

    fn server() -> Arc<dyn MediaServer> {
        fake().1
    }

    fn fake() -> (FakeServer, Arc<dyn MediaServer>) {
        let fake = FakeServer::new(16, false);
        let server = fake
            .login("https://media.example.org", USER, PASSWORD)
            .unwrap();
        (fake, server)
    }

    /// Types `text` at `now`, waits out the debounce and sends the search to the server.
    fn search(
        palette: &mut Palette,
        server: &dyn MediaServer,
        text: &str,
        now: Instant,
    ) -> (Ask, Instant) {
        palette.typed(text, now);
        let later = now + DEBOUNCE;
        let ask = palette.ask(later).expect("the search goes out");
        let answer = server.search(&ask.query, PER_GROUP).map_err(|_| "failed");
        assert!(palette.answered(&ask, answer));
        (ask, later)
    }

    fn titles(palette: &Palette) -> Vec<String> {
        palette
            .hits()
            .iter()
            .map(|h| h.title().to_string())
            .collect()
    }

    #[test]
    fn an_empty_query_shows_nothing_and_asks_nothing() {
        let t = Instant::now();
        let mut palette = Palette::default();
        assert_eq!(palette.message(), None);
        assert!(palette.lines().is_empty());
        palette.typed("   ", t);
        assert_eq!(palette.due(), None);
        assert_eq!(palette.ask(t + DEBOUNCE * 4), None);
        assert_eq!(palette.message(), None);
    }

    #[test]
    fn nothing_is_asked_while_the_user_is_still_typing() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut palette = Palette::default();
        palette.typed("h", t);
        palette.typed("hi", ms(100));
        palette.typed("hiv", ms(200));
        // 250 ms after the first key, but only 50 after the last.
        assert_eq!(palette.ask(ms(250)), None);
        assert_eq!(palette.due(), Some(ms(450)));
        let ask = palette.ask(ms(450)).expect("the user stopped");
        assert_eq!(ask.query, "hiv");
        // One search for the whole word; nothing more is waiting.
        assert_eq!(palette.ask(ms(900)), None);
    }

    #[test]
    fn the_server_gets_one_search_for_a_word_typed_quickly() {
        let t = Instant::now();
        let (fake, server) = fake();
        let mut palette = Palette::default();
        for (n, text) in ["h", "hi", "hiv"].into_iter().enumerate() {
            let at = t + Duration::from_millis(60 * n as u64);
            palette.typed(text, at);
            if let Some(ask) = palette.ask(at) {
                server.search(&ask.query, PER_GROUP).unwrap();
            }
        }
        let ask = palette.ask(t + Duration::from_millis(400)).unwrap();
        server.search(&ask.query, PER_GROUP).unwrap();
        assert_eq!(fake.state.lock().unwrap().record.searches, ["hiv"]);
    }

    #[test]
    fn results_are_grouped_as_films_series_and_people() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hiv", t);
        let lines = palette.lines();
        let headings: Vec<_> = lines
            .iter()
            .filter_map(|l| match l {
                Line::Heading(g) => Some(g.label()),
                Line::Hit(..) => None,
            })
            .collect();
        assert_eq!(headings, ["Films", "Series"]);
        assert_eq!(
            titles(&palette),
            ["Le Dernier Hiver", "Hivernage", "Hiver blanc"]
        );
    }

    #[test]
    fn people_are_found_by_name_in_their_own_group() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "mercier", t);
        assert_eq!(titles(&palette), ["Paul Mercier"]);
        assert!(matches!(palette.lines()[0], Line::Heading(Group::People)));
        assert!(matches!(palette.selected_hit(), Some(Hit::Person(_))));
    }

    #[test]
    fn a_group_with_no_result_has_no_heading() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "mercier", t);
        assert_eq!(palette.lines().len(), 2);
    }

    #[test]
    fn the_arrows_move_through_the_results_across_groups() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hiv", t);
        assert_eq!(palette.selected(), 0);
        assert_eq!(palette.key("down"), Key::Moved);
        assert_eq!(palette.key("down"), Key::Moved);
        // The third result is in the second group.
        assert!(matches!(palette.selected_hit(), Some(Hit::Series(s)) if s.title == "Hiver blanc"));
        assert_eq!(palette.key("up"), Key::Moved);
        assert_eq!(palette.selected(), 1);
    }

    #[test]
    fn the_selection_wraps_at_both_ends() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hiv", t);
        palette.key("up");
        assert_eq!(palette.selected(), 2);
        palette.key("down");
        assert_eq!(palette.selected(), 0);
    }

    #[test]
    fn enter_opens_the_selected_film_series_or_person() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hiv", t);
        assert!(
            matches!(palette.key("enter"), Key::Open(Hit::Film(f)) if f.title == "Le Dernier Hiver")
        );
        palette.key("down");
        assert!(
            matches!(palette.key("enter"), Key::Open(Hit::Series(s)) if s.title == "Hivernage")
        );
        search(
            &mut palette,
            &*server(),
            "mercier",
            t + Duration::from_secs(1),
        );
        assert!(
            matches!(palette.key("enter"), Key::Open(Hit::Person(p)) if p.name == "Paul Mercier")
        );
    }

    #[test]
    fn enter_with_no_result_opens_nothing() {
        let t = Instant::now();
        let mut palette = Palette::default();
        assert_eq!(palette.key("enter"), Key::Ignored);
        search(&mut palette, &*server(), "zzzz", t);
        assert_eq!(palette.key("enter"), Key::Ignored);
    }

    #[test]
    fn escape_closes_the_palette() {
        let mut palette = Palette::default();
        assert_eq!(palette.key("escape"), Key::Close);
    }

    #[test]
    fn other_keys_are_left_to_the_text_field() {
        let mut palette = Palette::default();
        assert_eq!(palette.key("a"), Key::Ignored);
        assert_eq!(palette.key("backspace"), Key::Ignored);
    }

    #[test]
    fn a_query_with_no_match_says_so() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "zzzz", t);
        assert_eq!(palette.message().as_deref(), Some("No results for “zzzz”"));
    }

    #[test]
    fn a_search_on_its_way_says_so() {
        let t = Instant::now();
        let mut palette = Palette::default();
        palette.typed("hiv", t);
        assert_eq!(palette.message().as_deref(), Some("Searching…"));
    }

    #[test]
    fn the_last_results_stay_while_the_next_ones_are_awaited() {
        let t = Instant::now();
        let server = server();
        let mut palette = Palette::default();
        let (_, later) = search(&mut palette, &*server, "hiv", t);
        palette.typed("hive", later);
        assert_eq!(palette.message(), None);
        assert_eq!(titles(&palette).len(), 3);
    }

    #[test]
    fn a_failed_search_says_so() {
        let t = Instant::now();
        let mut palette = Palette::default();
        palette.typed("hiv", t);
        let ask = palette.ask(t + DEBOUNCE).unwrap();
        assert!(palette.answered(&ask, Err("The search failed.")));
        assert_eq!(palette.message().as_deref(), Some("The search failed."));
    }

    #[test]
    fn a_slower_answer_for_an_older_query_never_replaces_a_newer_one() {
        let t = Instant::now();
        let server = server();
        let mut palette = Palette::default();
        palette.typed("hiv", t);
        let old = palette.ask(t + DEBOUNCE).unwrap();
        // The user keeps typing; the second search goes out before the first came back.
        let t2 = t + Duration::from_millis(400);
        palette.typed("mercier", t2);
        let new = palette.ask(t2 + DEBOUNCE).unwrap();

        let newer = server.search(&new.query, PER_GROUP).map_err(|_| "failed");
        assert!(palette.answered(&new, newer));
        let older = server.search(&old.query, PER_GROUP).map_err(|_| "failed");
        assert!(!palette.answered(&old, older));
        assert_eq!(titles(&palette), ["Paul Mercier"]);
    }

    #[test]
    fn an_answer_for_a_query_no_longer_typed_is_dropped() {
        let t = Instant::now();
        let server = server();
        let mut palette = Palette::default();
        palette.typed("hiv", t);
        let old = palette.ask(t + DEBOUNCE).unwrap();
        // The user typed again, and the new search has not gone out yet.
        palette.typed("hive", t + DEBOUNCE);
        let answer = server.search(&old.query, PER_GROUP).map_err(|_| "failed");
        assert!(!palette.answered(&old, answer));
        assert!(palette.hits().is_empty());
    }

    #[test]
    fn an_answer_that_comes_after_the_query_was_cleared_is_dropped() {
        let t = Instant::now();
        let server = server();
        let mut palette = Palette::default();
        palette.typed("hiv", t);
        let ask = palette.ask(t + DEBOUNCE).unwrap();
        palette.typed("", t + DEBOUNCE);
        let answer = server.search(&ask.query, PER_GROUP).map_err(|_| "failed");
        assert!(!palette.answered(&ask, answer));
        assert!(palette.hits().is_empty());
        assert_eq!(palette.message(), None);
    }

    #[test]
    fn clearing_the_query_forgets_the_results_and_the_search_waiting() {
        let t = Instant::now();
        let mut palette = Palette::default();
        let (_, later) = search(&mut palette, &*server(), "hiv", t);
        palette.typed("hiv again", later);
        palette.typed("", later);
        assert!(palette.hits().is_empty());
        assert_eq!(palette.due(), None);
        assert_eq!(palette.message(), None);
    }

    #[test]
    fn a_space_after_the_query_asks_nothing_again() {
        let t = Instant::now();
        let mut palette = Palette::default();
        let (_, later) = search(&mut palette, &*server(), "hiv", t);
        palette.typed("hiv ", later);
        assert_eq!(palette.due(), None);
        assert_eq!(palette.ask(later + DEBOUNCE), None);
    }

    #[test]
    fn new_results_start_at_the_first_one() {
        let t = Instant::now();
        let server = server();
        let mut palette = Palette::default();
        let (_, later) = search(&mut palette, &*server, "hiv", t);
        palette.key("down");
        search(&mut palette, &*server, "hive", later);
        assert_eq!(palette.selected(), 0);
    }

    #[test]
    fn the_mouse_selects_the_result_it_is_over() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hiv", t);
        palette.select(2);
        assert_eq!(palette.selected(), 2);
        palette.select(9);
        assert_eq!(palette.selected(), 2);
    }

    #[test]
    fn a_poster_in_a_row_is_about_twice_the_size_it_was() {
        // It was 36 by 54 in a 62 px row.
        let (height, width, photo) =
            std::hint::black_box((POSTER_HEIGHT, POSTER_WIDTH, PERSON_PHOTO));
        assert!((100. ..=112.).contains(&height), "{height}");
        assert!((width / height - 2. / 3.).abs() < 0.02);
        assert!(photo >= 80.);
    }

    #[test]
    fn a_row_holds_its_poster_and_the_space_under_it() {
        for image in std::hint::black_box([POSTER_HEIGHT, PERSON_PHOTO]) {
            assert!(highlight_height() >= image + 12., "{image} in a row");
        }
        assert_eq!(highlight_height() + ROW_GAP, ROW_HEIGHT);
    }

    #[test]
    fn a_longer_label_makes_a_wider_key() {
        assert!(key_cap_width("Esc") < key_cap_width("Enter"));
        assert_eq!(key_cap_width(""), 2. * (KEY_PAD_X + KEY_BORDER));
    }

    #[test]
    fn the_enter_hint_follows_the_highlight_through_its_glide() {
        let t = Instant::now();
        let mut glide = Glide::new(0., crate::motion::quick());
        glide.set(ROW_HEIGHT * 3., t);
        let mut last = None;
        for n in [0, 30, 60, 90, 120, 200] {
            let top = glide.value(t + Duration::from_millis(n));
            // Centred in the highlight at each step, not waiting for it to land.
            let hint = hint_top(top);
            assert_eq!(
                hint - top,
                (highlight_height() - HINT_HEIGHT) / 2.,
                "at {n} ms"
            );
            if let Some((last_top, last_hint)) = last {
                assert_eq!(hint - last_hint, top - last_top, "at {n} ms");
            }
            last = Some((top, hint));
        }
        let (top, hint) = last.unwrap();
        assert_eq!(top, ROW_HEIGHT * 3.);
        assert!(hint > ROW_HEIGHT * 3. && hint + HINT_HEIGHT < top + highlight_height());
    }

    #[test]
    fn the_selected_row_scrolls_into_view() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "e", t);
        let lines = palette.lines();
        let view = 220.;
        // The first result needs no scrolling.
        assert_eq!(scroll_to_show(&lines, 0, 0., view), 0.);
        // A row below the view brings its bottom to the view's bottom.
        let top = row_top(&lines, 3).unwrap();
        let wanted = scroll_to_show(&lines, 3, 0., view);
        assert_eq!(wanted, top + ROW_HEIGHT - view);
        // The first row, above the view, brings the heading of its group with it.
        assert_eq!(scroll_to_show(&lines, 0, wanted, view), 0.);
        // A row already in view leaves the list where it is.
        assert_eq!(scroll_to_show(&lines, 3, wanted, view), wanted);
    }

    #[test]
    fn the_list_never_scrolls_past_its_end() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hiv", t);
        let lines = palette.lines();
        assert_eq!(scroll_to_show(&lines, 2, 0., 10_000.), 0.);
    }

    #[test]
    fn a_film_result_reads_its_year_rating_and_where_the_viewer_is() {
        let t = Instant::now();
        let mut palette = Palette::default();
        // The first film of the fake server is started; the fourth is watched.
        search(&mut palette, &*server(), "dernier", t);
        let started = palette.hits().remove(0);
        assert!(
            started.caption().starts_with("2025 · ★ "),
            "{}",
            started.caption()
        );
        assert!(started.caption().ends_with(" · started"));
        let watched = server().search("Racines", 1).unwrap().films.remove(0);
        assert!(
            Hit::Film(Box::new(watched))
                .caption()
                .ends_with(" · watched")
        );
    }

    #[test]
    fn a_series_reads_its_year_and_a_person_has_no_line() {
        let t = Instant::now();
        let mut palette = Palette::default();
        search(&mut palette, &*server(), "hivernage", t);
        assert_eq!(palette.hits()[0].caption(), "2021");
        search(
            &mut palette,
            &*server(),
            "mercier",
            t + Duration::from_secs(1),
        );
        assert_eq!(palette.hits()[0].caption(), "");
    }

    #[test]
    fn ctrl_k_alone_opens_the_palette() {
        assert!(is_shortcut("k", true, false));
        assert!(!is_shortcut("k", false, false));
        assert!(!is_shortcut("k", true, true));
        assert!(!is_shortcut("j", true, false));
    }

    #[test]
    fn search_opens_from_every_screen_but_the_login_and_the_player() {
        for screen in [
            Screen::Home,
            Screen::Library,
            Screen::Series,
            Screen::FilmDetail,
            Screen::SeriesDetail,
            Screen::Person,
            Screen::HomeList,
            Screen::Collections,
            Screen::CollectionDetail,
            Screen::MyList,
            Screen::Favorites,
            Screen::Playlist,
        ] {
            assert!(opens_from(screen), "{screen:?}");
        }
        assert!(!opens_from(Screen::Login));
        assert!(!opens_from(Screen::Player));
    }
}
