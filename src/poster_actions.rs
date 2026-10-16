//! The two small buttons of a poster under the pointer: "My list" and "Watched". A press shows
//! the new state at once and asks the server; if the server refuses, the button goes back and
//! the window says so, like the "My list" button of a page. The posters hold the titles as the
//! server listed them: what changed since is kept here, by title, and drawn over them, so that
//! no row or grid is read again for it. A list the server gave after a change has the last word:
//! the change shown takes what it says, over the lists kept from before as well.

use std::collections::HashMap;

use bobine::provider::{Favourite, Film, ItemId, MediaServer, ProviderError, Series, Title};

use crate::favourite;

/// What the window says when the server did not take a change of the watched state.
pub const WATCHED_FAILED: &str = "Watched could not be changed.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    MyList,
    Watched,
}

impl Action {
    fn failed(self) -> &'static str {
        match self {
            Self::MyList => favourite::FAILED,
            Self::Watched => WATCHED_FAILED,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Mark {
    on: bool,
    /// The server is being asked: the button takes no other press until it answers.
    pending: bool,
    /// When it last changed, on the clock of `Marks::stamp`.
    at: u64,
}

/// What the posters show of their titles that the lists they come from do not say yet.
#[derive(Debug, Default)]
pub struct Marks {
    changed: HashMap<(ItemId, Action), Mark>,
    /// Goes up at each change: a list asked at a later time is newer than the change.
    clock: u64,
}

impl Marks {
    /// Whether the button of `item` is on, `listed` being what its list says.
    pub fn shown(&self, item: &ItemId, action: Action, listed: bool) -> bool {
        self.changed
            .get(&(item.clone(), action))
            .map_or(listed, |mark| mark.on)
    }

    /// A press: the button shows the other state at once. Returns what to ask the server, or
    /// nothing when a request for it is already on its way.
    pub fn press(&mut self, item: &ItemId, action: Action, listed: bool) -> Option<bool> {
        let at = self.tick();
        let mark = self.changed.entry((item.clone(), action)).or_insert(Mark {
            on: listed,
            pending: false,
            at,
        });
        if mark.pending {
            return None;
        }
        mark.on = !mark.on;
        mark.pending = true;
        mark.at = at;
        Some(mark.on)
    }

    /// The server answered the request for `asked`. A failure puts the button back; returns
    /// what to tell the user then.
    pub fn answered(
        &mut self,
        item: &ItemId,
        action: Action,
        asked: bool,
        answer: &Result<(), ProviderError>,
    ) -> Option<&'static str> {
        let at = self.tick();
        let mark = self.changed.get_mut(&(item.clone(), action))?;
        mark.pending = false;
        // A list asked while the server was changing it may not say the change yet.
        mark.at = at;
        if answer.is_ok() {
            return None;
        }
        mark.on = !asked;
        Some(action.failed())
    }

    /// A page changed the item on the server: its posters show it too, unless one of them has a
    /// change of its own on its way (its answer carries the last word).
    pub fn follow(&mut self, item: &ItemId, action: Action, now: bool) {
        let at = self.tick();
        let mark = self.changed.entry((item.clone(), action)).or_insert(Mark {
            on: now,
            pending: false,
            at,
        });
        if !mark.pending {
            mark.on = now;
            mark.at = at;
        }
    }

    /// The time a list is asked at: what it says, once it comes, is newer than the changes made
    /// before.
    pub fn stamp(&self) -> u64 {
        self.clock
    }

    /// A list asked at `asked` says the item is `now`: a change made before it takes it, while
    /// one made since, or still on its way, keeps its own.
    pub fn heard(&mut self, asked: u64, item: &ItemId, action: Action, now: bool) {
        if let Some(mark) = self.changed.get_mut(&(item.clone(), action))
            && !mark.pending
            && mark.at <= asked
        {
            mark.on = now;
        }
    }

    /// The titles a list asked at `asked` gave, with what they say of themselves.
    pub fn listed<'a, T: Said + 'a>(
        &mut self,
        asked: u64,
        titles: impl IntoIterator<Item = &'a T>,
    ) {
        if self.changed.is_empty() {
            return;
        }
        for title in titles {
            if let Some((item, in_list, watched)) = title.said() {
                self.heard(asked, item, Action::MyList, in_list);
                self.heard(asked, item, Action::Watched, watched);
            }
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }
}

/// What a listed title says of itself: whether it is in the user's list, and watched.
pub trait Said {
    fn said(&self) -> Option<(&ItemId, bool, bool)>;
}

impl Said for Film {
    fn said(&self) -> Option<(&ItemId, bool, bool)> {
        Some((&self.id, self.favourite, self.watched))
    }
}

impl Said for Series {
    /// A new season handed for its series says nothing of the series.
    fn said(&self) -> Option<(&ItemId, bool, bool)> {
        self.season
            .is_none()
            .then_some((&self.id, self.favourite, self.watched))
    }
}

impl Said for Favourite {
    fn said(&self) -> Option<(&ItemId, bool, bool)> {
        match self {
            Favourite::Film(film) => film.said(),
            Favourite::Series(series) => series.said(),
        }
    }
}

impl Said for Title {
    /// An episode has no poster to mark.
    fn said(&self) -> Option<(&ItemId, bool, bool)> {
        match self {
            Title::Film(film) => film.said(),
            Title::Series(series) => series.said(),
            Title::Episode(_) => None,
        }
    }
}

/// Asks the server for the change: a series is marked as a whole, all its episodes with it.
pub fn ask(
    server: &dyn MediaServer,
    item: &ItemId,
    action: Action,
    wanted: bool,
) -> Result<(), ProviderError> {
    match action {
        Action::MyList => server.set_favourite(item, wanted),
        Action::Watched => server.set_watched(item, wanted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FakeServer};

    fn film() -> ItemId {
        ItemId(FILM.into())
    }

    /// A press, the request it asks for and its answer, as the window does them in order.
    fn press_and_ask(
        marks: &mut Marks,
        server: &FakeServer,
        item: &ItemId,
        action: Action,
        listed: bool,
    ) -> Option<&'static str> {
        let wanted = marks.press(item, action, listed)?;
        let answer = ask(server, item, action, wanted);
        marks.answered(item, action, wanted, &answer)
    }

    #[test]
    fn my_list_adds_and_removes() {
        let server = FakeServer::new(3, false);
        let mut marks = Marks::default();
        assert_eq!(
            press_and_ask(&mut marks, &server, &film(), Action::MyList, false),
            None
        );
        assert!(marks.shown(&film(), Action::MyList, false));
        assert!(server.state.lock().unwrap().favourites.contains(&film()));
        assert_eq!(
            press_and_ask(&mut marks, &server, &film(), Action::MyList, false),
            None
        );
        assert!(!marks.shown(&film(), Action::MyList, false));
        assert!(!server.state.lock().unwrap().favourites.contains(&film()));
    }

    #[test]
    fn my_list_shows_the_change_before_the_server_answers() {
        let mut marks = Marks::default();
        assert_eq!(marks.press(&film(), Action::MyList, true), Some(false));
        assert!(!marks.shown(&film(), Action::MyList, true));
        // A second press while the first is on its way asks nothing.
        assert_eq!(marks.press(&film(), Action::MyList, true), None);
        assert!(!marks.shown(&film(), Action::MyList, true));
    }

    #[test]
    fn a_refused_my_list_change_is_put_back_and_told() {
        let server = FakeServer::new(3, false);
        server.state.lock().unwrap().favourite_fails = true;
        let mut marks = Marks::default();
        assert_eq!(
            press_and_ask(&mut marks, &server, &film(), Action::MyList, false),
            Some("My list could not be changed.")
        );
        assert!(!marks.shown(&film(), Action::MyList, false));
        // Back as it was, it takes the next press.
        assert_eq!(marks.press(&film(), Action::MyList, false), Some(true));
    }

    #[test]
    fn watched_marks_and_unmarks() {
        let server = FakeServer::new(3, false);
        let listed = server.state.lock().unwrap().films[0].clone();
        assert!(!listed.watched);
        let mut marks = Marks::default();
        assert_eq!(
            press_and_ask(&mut marks, &server, &listed.id, Action::Watched, false),
            None
        );
        assert!(marks.shown(&listed.id, Action::Watched, false));
        assert!(server.state.lock().unwrap().films[0].watched);
        assert_eq!(
            press_and_ask(&mut marks, &server, &listed.id, Action::Watched, false),
            None
        );
        assert!(!marks.shown(&listed.id, Action::Watched, false));
        assert!(!server.state.lock().unwrap().films[0].watched);
    }

    #[test]
    fn a_series_marked_watched_from_its_poster_is_marked_as_a_whole() {
        let server = FakeServer::new(3, false);
        let listed = server.state.lock().unwrap().series[0].clone();
        let mut marks = Marks::default();
        press_and_ask(&mut marks, &server, &listed.id, Action::Watched, false);
        assert!(marks.shown(&listed.id, Action::Watched, false));
        let state = server.state.lock().unwrap();
        assert!(
            state
                .episodes
                .iter()
                .filter(|e| e.series == listed.id)
                .all(|e| e.item.watched)
        );
    }

    #[test]
    fn a_refused_watched_change_is_put_back_and_told() {
        let mut marks = Marks::default();
        let wanted = marks.press(&film(), Action::Watched, true).unwrap();
        let refused = Err(ProviderError::Unreachable("timed out".into()));
        assert_eq!(
            marks.answered(&film(), Action::Watched, wanted, &refused),
            Some(WATCHED_FAILED)
        );
        assert!(marks.shown(&film(), Action::Watched, true));
    }

    #[test]
    fn a_change_made_on_a_page_shows_on_the_posters_unless_one_of_theirs_is_on_its_way() {
        let mut marks = Marks::default();
        marks.follow(&film(), Action::MyList, true);
        assert!(marks.shown(&film(), Action::MyList, false));
        assert_eq!(marks.press(&film(), Action::Watched, false), Some(true));
        marks.follow(&film(), Action::Watched, false);
        assert!(marks.shown(&film(), Action::Watched, false));
    }

    #[test]
    fn a_list_asked_after_a_mark_shows_what_the_server_says_now() {
        let mut marks = Marks::default();
        let wanted = marks.press(&film(), Action::Watched, false).unwrap();
        marks.answered(&film(), Action::Watched, wanted, &Ok(()));
        // Unmarked since, elsewhere: the list asked now says so.
        let asked = marks.stamp();
        marks.heard(asked, &film(), Action::Watched, false);
        assert!(!marks.shown(&film(), Action::Watched, false));
        // Even over a list kept from before the mark, not asked again.
        assert!(!marks.shown(&film(), Action::Watched, true));
    }

    #[test]
    fn a_list_asked_before_a_mark_does_not_undo_it() {
        let mut marks = Marks::default();
        let asked = marks.stamp();
        let wanted = marks.press(&film(), Action::Watched, false).unwrap();
        marks.answered(&film(), Action::Watched, wanted, &Ok(()));
        marks.heard(asked, &film(), Action::Watched, false);
        assert!(marks.shown(&film(), Action::Watched, false));
    }

    #[test]
    fn a_list_does_not_undo_a_change_on_its_way() {
        let mut marks = Marks::default();
        marks.press(&film(), Action::MyList, false).unwrap();
        let asked = marks.stamp();
        marks.heard(asked, &film(), Action::MyList, false);
        assert!(marks.shown(&film(), Action::MyList, false));
    }

    #[test]
    fn a_list_tells_the_state_of_its_films_and_series_but_not_of_a_new_season() {
        let mut marks = Marks::default();
        let mut film = crate::fake::films(1).remove(0);
        let mut series = crate::fake::series().remove(0);
        for item in [&film.id, &series.id] {
            marks.follow(item, Action::MyList, true);
            marks.follow(item, Action::Watched, true);
        }
        let asked = marks.stamp();
        (film.favourite, film.watched) = (false, false);
        (series.favourite, series.watched) = (false, false);
        let mut season = series.clone();
        season.season = Some(bobine::provider::Season {
            id: ItemId("season".into()),
            name: "Season 3".into(),
            number: Some(3),
            unwatched: 4,
        });
        // A new season says nothing of its series: what the list of it says is not heard.
        marks.listed(asked, [&Title::Series(season)]);
        assert!(marks.shown(&series.id, Action::Watched, false));
        marks.listed(
            asked,
            [&Title::Film(film.clone()), &Title::Series(series.clone())],
        );
        for item in [&film.id, &series.id] {
            assert!(!marks.shown(item, Action::MyList, true));
            assert!(!marks.shown(item, Action::Watched, true));
        }
    }
}
