//! Which screen is shown, and how one goes from one to another. No window here: the logic is
//! tested on its own and the views only read it.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Login,
    Home,
    Library,
    FilmDetail,
    Player,
    Series,
    SeriesDetail,
}

impl Screen {
    pub fn shows_pill_bar(self) -> bool {
        matches!(self, Screen::Home | Screen::Library | Screen::Series)
    }

    /// Detail pages have no pill bar: they say where Escape leads instead. The player draws its
    /// own controls.
    pub fn shows_back_link(self) -> bool {
        matches!(self, Screen::FilmDetail | Screen::SeriesDetail)
    }

    /// The word the back link of a detail page uses for the screen it goes back to.
    fn name(self) -> &'static str {
        match self {
            Screen::Login => "Connexion",
            Screen::Home => "Accueil",
            Screen::Library => "Films",
            Screen::FilmDetail | Screen::SeriesDetail => "Fiche",
            Screen::Player => "Lecteur",
            Screen::Series => "Séries",
        }
    }
}

/// The entries of the pill bar. Those whose screen does not exist yet are visible but inert.
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
            NavEntry::Home => "Accueil",
            NavEntry::Films => "Films",
            NavEntry::Series => "Séries",
            NavEntry::Collections => "Collections",
            NavEntry::MyList => "Ma liste",
        }
    }

    pub fn target(self) -> Option<Screen> {
        match self {
            NavEntry::Home => Some(Screen::Home),
            NavEntry::Films => Some(Screen::Library),
            NavEntry::Series => Some(Screen::Series),
            NavEntry::Collections | NavEntry::MyList => None,
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

    /// The label of the back link: `Films` when the page was reached from the library.
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
    fn the_pill_bar_shows_on_home_and_library_only() {
        let shown: Vec<Screen> = [
            Screen::Login,
            Screen::Home,
            Screen::Library,
            Screen::FilmDetail,
            Screen::Player,
        ]
        .into_iter()
        .filter(|screen| screen.shows_pill_bar())
        .collect();
        assert_eq!(shown, [Screen::Home, Screen::Library]);
    }

    #[test]
    fn a_detail_page_shows_the_back_link_and_not_the_pill_bar() {
        assert!(Screen::FilmDetail.shows_back_link());
        assert!(!Screen::FilmDetail.shows_pill_bar());
        assert!(!Screen::Home.shows_back_link());
        assert!(!Screen::Library.shows_back_link());
    }

    #[test]
    fn the_back_link_names_the_screen_it_returns_to() {
        let mut nav = on_library();
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Films"));

        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        nav.open(Screen::FilmDetail);
        assert_eq!(nav.back_label(), Some("Accueil"));
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
    fn the_entries_not_built_yet_are_inert() {
        let mut nav = on_library();
        for entry in [NavEntry::Collections, NavEntry::MyList] {
            assert_eq!(entry.target(), None);
            assert!(!nav.select(entry));
            assert_eq!(nav.current(), Screen::Library);
        }
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
    fn a_series_page_has_a_back_link_to_the_series_and_no_pill_bar() {
        let mut nav = Navigator::new();
        nav.switch_root(Screen::Home);
        assert!(nav.select(NavEntry::Series));
        nav.open(Screen::SeriesDetail);
        assert!(!Screen::SeriesDetail.shows_pill_bar());
        assert_eq!(nav.back_label(), Some("Séries"));
        nav.open(Screen::Player);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::SeriesDetail);
        assert!(nav.back());
        assert_eq!(nav.current(), Screen::Series);
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
    fn the_labels_are_in_french() {
        let labels: Vec<&str> = NavEntry::ALL.into_iter().map(NavEntry::label).collect();
        assert_eq!(
            labels,
            ["Accueil", "Films", "Séries", "Collections", "Ma liste"]
        );
    }
}
