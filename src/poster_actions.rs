//! The two small buttons of a poster under the pointer: "My list" and "Watched". A press shows
//! the new state at once and asks the server; if the server refuses, the button goes back and
//! the window says so, like the "My list" button of a page. The posters hold the titles as the
//! server listed them: what changed since is kept here, by title, and drawn over them, so that
//! no row or grid is read again for it.

use std::collections::HashMap;

use bobine::provider::{ItemId, MediaServer, ProviderError};

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
}

/// What the posters show of their titles that the lists they come from do not say yet.
#[derive(Debug, Default)]
pub struct Marks {
    changed: HashMap<(ItemId, Action), Mark>,
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
        let mark = self.changed.entry((item.clone(), action)).or_insert(Mark {
            on: listed,
            pending: false,
        });
        if mark.pending {
            return None;
        }
        mark.on = !mark.on;
        mark.pending = true;
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
        let mark = self.changed.get_mut(&(item.clone(), action))?;
        mark.pending = false;
        if answer.is_ok() {
            return None;
        }
        mark.on = !asked;
        Some(action.failed())
    }

    /// A page changed the item on the server: its posters show it too, unless one of them has a
    /// change of its own on its way (its answer carries the last word).
    pub fn follow(&mut self, item: &ItemId, action: Action, now: bool) {
        let mark = self.changed.entry((item.clone(), action)).or_insert(Mark {
            on: now,
            pending: false,
        });
        if !mark.pending {
            mark.on = now;
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
}
