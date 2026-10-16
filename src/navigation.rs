//! Which screen is shown, and how one goes from one to another. No window here: the logic is
//! tested on its own and the views only read it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Login,
    Home,
    Library,
    FilmDetail,
    /// What a person took part in, reached from a search.
    Person,
    Player,
    Series,
    SeriesDetail,
    /// All of a row of the Home: what was started, the next episodes, what a library gained.
    HomeList,
    Collections,
    CollectionDetail,
    /// The Lists: the favorites and each playlist, as cards.
    MyList,
    /// The favorites, opened from the Lists.
    Favorites,
    /// One playlist, opened from the Lists.
    Playlist,
    /// The viewer's choices, reached from the account menu.
    Settings,
}

impl Screen {
    /// Everywhere but on the login, before there is a server, and in the player, which draws its
    /// own controls.
    pub fn shows_pill_bar(self) -> bool {
        !matches!(self, Screen::Login | Screen::Player)
    }

    /// Detail pages also say where Escape leads, under the bar. A full list does not: it is laid
    /// out like the library grids, and the bar's Home leads back.
    pub fn shows_back_link(self) -> bool {
        matches!(
            self,
            Screen::FilmDetail
                | Screen::SeriesDetail
                | Screen::Person
                | Screen::CollectionDetail
                | Screen::Favorites
                | Screen::Playlist
                | Screen::Settings
        )
    }

    /// Pages that draw a backdrop under the bar, which a shade at the top keeps readable.
    pub fn has_backdrop(self) -> bool {
        matches!(
            self,
            Screen::FilmDetail | Screen::SeriesDetail | Screen::CollectionDetail
        )
    }

    /// The word the back link of a detail page uses for the screen it goes back to.
    fn name(self) -> &'static str {
        match self {
            Screen::Login => "Sign in",
            Screen::Home => "Home",
            Screen::Library => "Movies",
            Screen::FilmDetail | Screen::SeriesDetail => "Details",
            Screen::Person => "Person",
            Screen::Player => "Player",
            Screen::Series => "Series",
            Screen::HomeList => "List",
            Screen::Collections => "Collections",
            Screen::CollectionDetail => "Collection",
            Screen::MyList => "Lists",
            Screen::Favorites => "Favorites",
            Screen::Playlist => "Playlist",
            Screen::Settings => "Settings",
        }
    }
}

/// The entries of the pill bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavEntry {
    Home,
    Films,
    Series,
    Collections,
    MyList,
}

impl NavEntry {
    pub const ALL: [NavEntry; 5] = [
        NavEntry::Home,
        NavEntry::Films,
        NavEntry::Series,
        NavEntry::Collections,
        NavEntry::MyList,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NavEntry::Home => "Home",
            NavEntry::Films => "Movies",
            NavEntry::Series => "Series",
            NavEntry::Collections => "Collections",
            NavEntry::MyList => "Lists",
        }
    }

    pub fn target(self) -> Option<Screen> {
        match self {
            NavEntry::Home => Some(Screen::Home),
            NavEntry::Films => Some(Screen::Library),
            NavEntry::Series => Some(Screen::Series),
            NavEntry::Collections => Some(Screen::Collections),
            NavEntry::MyList => Some(Screen::MyList),
        }
    }

    pub fn is_current(self, screen: Screen) -> bool {
        self.target() == Some(screen)
    }
}

pub struct Navigator {
    stack: Vec<Screen>,
}

impl Navigator {
    pub fn new() -> Self {
        Self {
            stack: vec![Screen::Login],
        }
    }

    pub fn current(&self) -> Screen {
        *self.stack.last().expect("the stack is never empty")
    }

    /// The screen of the pill bar the stack stands in: its entry is the one highlighted. The
    /// last one opened counts, so a library pushed over a film page lights its own pill.
    pub fn section(&self) -> Screen {
        self.stack
            .iter()
            .rev()
            .find(|screen| NavEntry::ALL.iter().any(|entry| entry.is_current(**screen)))
            .copied()
            .unwrap_or(self.stack[0])
    }

    /// Goes deeper (a film from a library, the player from a film): `back` returns here.
    pub fn open(&mut self, screen: Screen) {
        self.stack.push(screen);
    }

    /// Makes `screen` the root: nothing is left to go back to (the pill bar, a successful login).
    pub fn switch_root(&mut self, screen: Screen) {
        self.stack = vec![screen];
    }

    /// A click on the pill bar. Returns false when the entry is inert.
    pub fn select(&mut self, entry: NavEntry) -> bool {
        let Some(screen) = entry.target() else {
            return false;
        };
        self.switch_root(screen);
        true
    }

    /// Escape or the back link. Returns false at the root, where there is nothing to go back to.
    pub fn back(&mut self) -> bool {
        if self.stack.len() < 2 {
            return false;
        }
        self.stack.pop();
        true
    }

    /// The label of the back link: `Movies` when the page was reached from the library.
    pub fn back_label(&self) -> Option<&'static str> {
        if !self.current().shows_back_link() {
            return None;
        }
        let below = self.stack.len().checked_sub(2)?;
        Some(self.stack[below].name())
    }
}

impl Default for Navigator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_library() -> Navigator {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        assert!(nav.select(NavEntry::Films));
        nav
    }

    #[test]
    fn the_application_starts_on_the_login_screen() {
        assert_eq!(Navigator::new().current(), Screen::Login);
    }

    #[test]
    fn the_pill_bar_shows_on_every_screen_but_the_login_and_the_player() {
        let hidden: Vec<Screen> = [
            Screen::Login,
            Screen::Home,
            Screen::Library,
            Screen::FilmDetail,
            Screen::Person,
            Screen::Player,
            Screen::Series,
            Screen::SeriesDetail,
            Screen::HomeList,
            Screen::Collections,
            Screen::CollectionDetail,
            Screen::MyList,
            Screen::Favorites,
            Screen::Playlist,
            Screen::Settings,
        ]
        .into_iter()
        .filter(|screen| !screen.shows_pill_bar())
        .collect();
        assert_eq!(hidden, [Screen::Login, Screen::Player]);
    }

    #[test]
    fn a_detail_page_shows_the_back_link_under_the_pill_bar() {
        assert!(Screen::FilmDetail.shows_back_link());
        assert!(Screen::FilmDetail.shows_pill_bar());
        assert!(!Screen::Home.shows_back_link());
        assert!(!Screen::Library.shows_back_link());
    }

    #[test]
    fn the_settings_open_over_the_screen_they_were_reached_from_and_back_returns_to_it() {
        let mut nav = on_library();
        nav.open(Screen::Settings);
        assert_eq!(nav.current(), Screen::Settings);
        assert!(Screen::Settings.shows_pill_bar());
        assert_eq!(nav.back_label(), Some("Movies"));
        // The pill of the screen the settings were reached from stays the one highlighted.
        assert_eq!(nav.section(), Screen::Library);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Library);
    }

    #[test]
    fn the_shade_at_the_top_is_for_the_pages_with_a_backdrop_and_not_the_person_page() {
        assert!(Screen::FilmDetail.has_backdrop());
        assert!(Screen::SeriesDetail.has_backdrop());
        assert!(Screen::CollectionDetail.has_backdrop());
        assert!(!Screen::Person.has_backdrop());
        assert!(!Screen::Home.has_backdrop());
    }

    #[test]
    fn the_pill_of_the_section_a_page_belongs_to_is_highlighted() {
        let mut nav = on_library();
        nav.open(Screen::FilmDetail);
        nav.open(Screen::Person);
        let current: Vec<NavEntry> = NavEntry::ALL
            .into_iter()
            .filter(|entry| entry.is_current(nav.section()))
            .collect();
        assert_eq!(current, [NavEntry::Films]);

        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::HomeList);
        assert_eq!(nav.section(), Screen::Home);
    }

    #[test]
    fn a_library_pushed_over_a_page_lights_its_own_pill_and_back_returns_to_the_page() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::FilmDetail);
        nav.open(Screen::Library);
        assert_eq!(nav.section(), Screen::Library);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::FilmDetail);
        assert_eq!(nav.section(), Screen::Home);
    }

    #[test]
    fn the_back_link_names_the_screen_it_returns_to() {
        let mut nav = on_library();
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Movies"));

        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Home"));
    }

    #[test]
    fn a_person_page_has_a_back_link_to_where_it_was_opened_from() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::Person);
        assert!(Screen::Person.shows_back_link() && Screen::Person.shows_pill_bar());
        assert_eq!(nav.back_label(), Some("Home"));
        // A film opened from the person goes back to the person.
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Person"));
        assert!(nav.back() && nav.back());
        assert_eq!(nav.current(), Screen::Home);
    }

    #[test]
    fn a_series_found_or_on_a_person_page_opens_with_a_back_link_to_where_it_was() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Library);
        // From the palette, over the films.
        nav.open(Screen::SeriesDetail);
        assert_eq!(nav.back_label(), Some("Movies"));
        assert!(nav.back());
        // From a person's page.
        nav.open(Screen::Person);
        nav.open(Screen::SeriesDetail);
        assert_eq!(nav.back_label(), Some("Person"));
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Person);
    }

    #[test]
    fn there_is_no_back_link_on_a_screen_that_does_not_show_one() {
        assert_eq!(on_library().back_label(), None);
    }

    #[test]
    fn escape_on_a_film_goes_back_to_the_library() {
        let mut nav = on_library();
        nav.open(Screen::FilmDetail);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Library);
    }

    #[test]
    fn escape_in_the_player_goes_back_to_the_film() {
        let mut nav = on_library();
        nav.open(Screen::FilmDetail);
        nav.open(Screen::Player);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::FilmDetail);
    }

    #[test]
    fn back_from_the_root_does_nothing() {
        let mut nav = Navigator::new();
        assert!(!nav.back());
        assert_eq!(nav.current(), Screen::Login);

        nav.switch_root(Screen::Home);
        assert!(!nav.back());
        assert_eq!(nav.current(), Screen::Home);
    }

    #[test]
    fn a_successful_login_cannot_be_gone_back_to() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        assert!(!nav.back());
        assert_eq!(nav.current(), Screen::Home);
    }

    #[test]
    fn the_pill_bar_switches_between_home_and_library_without_history() {
        let mut nav = on_library();
        assert_eq!(nav.current(), Screen::Library);
        assert!(nav.select(NavEntry::Home));
        assert_eq!(nav.current(), Screen::Home);
        assert!(!nav.back());
        assert_eq!(nav.current(), Screen::Home);
    }

    #[test]
    fn every_entry_of_the_pill_bar_opens_a_screen() {
        let mut nav = on_library();
        for entry in NavEntry::ALL {
            assert!(entry.target().is_some(), "{entry:?}");
            assert!(nav.select(entry));
            assert!(entry.is_current(nav.current()));
            assert!(!nav.back(), "{entry:?} is a root");
        }
    }

    #[test]
    fn the_collections_and_my_list_entries_open_their_screens_under_the_pill_bar() {
        let mut nav = on_library();
        assert!(nav.select(NavEntry::Collections));
        assert_eq!(nav.current(), Screen::Collections);
        assert!(nav.select(NavEntry::MyList));
        assert_eq!(nav.current(), Screen::MyList);
        assert!(Screen::Collections.shows_pill_bar() && Screen::MyList.shows_pill_bar());
    }

    #[test]
    fn a_collection_page_has_a_back_link_to_the_collections_under_the_pill_bar() {
        let mut nav = on_library();
        assert!(nav.select(NavEntry::Collections));
        nav.open(Screen::CollectionDetail);
        assert!(Screen::CollectionDetail.shows_back_link());
        assert!(Screen::CollectionDetail.shows_pill_bar());
        assert_eq!(nav.back_label(), Some("Collections"));
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Collections);
    }

    #[test]
    fn a_film_opened_from_a_collection_goes_back_to_the_collection() {
        let mut nav = on_library();
        assert!(nav.select(NavEntry::Collections));
        nav.open(Screen::CollectionDetail);
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Collection"));
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::CollectionDetail);
    }

    #[test]
    fn a_collection_opened_from_a_film_page_goes_back_to_that_page() {
        let mut nav = on_library();
        nav.open(Screen::FilmDetail);
        nav.open(Screen::CollectionDetail);
        assert_eq!(nav.back_label(), Some("Details"));
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::FilmDetail);
    }

    #[test]
    fn a_film_or_a_series_opened_from_my_list_goes_back_to_it() {
        let mut nav = on_library();
        assert!(nav.select(NavEntry::MyList));
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Lists"));
        assert!(nav.back());
        nav.open(Screen::SeriesDetail);
        assert_eq!(nav.back_label(), Some("Lists"));
    }

    #[test]
    fn the_series_entry_opens_the_series_grid_under_the_pill_bar() {
        let mut nav = on_library();
        assert!(nav.select(NavEntry::Series));
        assert_eq!(nav.current(), Screen::Series);
        assert!(Screen::Series.shows_pill_bar());
        assert!(NavEntry::Series.is_current(Screen::Series));
    }

    #[test]
    fn a_series_page_has_a_back_link_to_the_series_under_the_pill_bar() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        assert!(nav.select(NavEntry::Series));
        nav.open(Screen::SeriesDetail);
        assert!(Screen::SeriesDetail.shows_pill_bar());
        assert_eq!(nav.back_label(), Some("Series"));
        nav.open(Screen::Player);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::SeriesDetail);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Series);
    }

    #[test]
    fn a_full_list_has_the_pill_bar_and_no_back_link_like_the_library_grids() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::HomeList);
        assert!(Screen::HomeList.shows_pill_bar());
        assert!(!Screen::HomeList.shows_back_link());
        assert_eq!(nav.back_label(), None);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Home);
    }

    #[test]
    fn a_title_opened_from_a_full_list_goes_back_to_the_list() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::HomeList);
        nav.open(Screen::SeriesDetail);
        assert_eq!(nav.back_label(), Some("List"));
        nav.open(Screen::Player);
        assert!(nav.back() && nav.back());
        assert_eq!(nav.current(), Screen::HomeList);
    }

    #[test]
    fn the_pill_bar_marks_the_entry_of_the_current_screen() {
        let current: Vec<NavEntry> = NavEntry::ALL
            .into_iter()
            .filter(|entry| entry.is_current(Screen::Library))
            .collect();
        assert_eq!(current, [NavEntry::Films]);
    }

    #[test]
    fn the_labels_are_in_english() {
        let labels: Vec<&str> = NavEntry::ALL.into_iter().map(NavEntry::label).collect();
        assert_eq!(labels, ["Home", "Movies", "Series", "Collections", "Lists"]);
    }

    #[test]
    fn the_favorites_and_a_playlist_open_from_the_lists_and_go_back_to_them() {
        let mut nav = on_library();
        assert!(nav.select(NavEntry::MyList));
        for screen in [Screen::Favorites, Screen::Playlist] {
            nav.open(screen);
            assert!(screen.shows_pill_bar() && screen.shows_back_link());
            assert_eq!(nav.back_label(), Some("Lists"));
            assert_eq!(nav.section(), Screen::MyList, "the Lists pill stays lit");
            nav.open(Screen::FilmDetail);
            let back = if screen == Screen::Playlist {
                "Playlist"
            } else {
                "Favorites"
            };
            assert_eq!(nav.back_label(), Some(back));
            assert!(nav.back() && nav.back());
            assert_eq!(nav.current(), Screen::MyList);
        }
    }
}
