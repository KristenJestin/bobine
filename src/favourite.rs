//! The "Ma liste" button of a film or a series page. Pressing it changes what the button shows
//! at once and asks the server; if the server fails, the button goes back and says so. While a
//! request is on its way the button asks nothing more, so that two presses never leave the
//! server and the page disagreeing about the order they came in.

use bobine::provider::{ItemId, MediaServer, ProviderError};

/// What the page says when the server did not take the change.
pub const FAILED: &str = "Ma liste n'a pas pu être modifiée.";

/// Whether the item is in the list, and whether the server is being asked to change it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggle {
    on: bool,
    pending: bool,
}

impl Toggle {
    /// The item is in the list, as far as the page shows.
    pub fn on(&self) -> bool {
        self.on
    }

    /// What the server says, when the page is loaded again. While a change is on its way the
    /// page knows better than the server does.
    pub fn sync(&mut self, server_says: bool) {
        if !self.pending {
            self.on = server_says;
        }
    }

    /// A press: the button shows the other state at once. Returns what to ask the server, or
    /// nothing when a request is already on its way.
    pub fn press(&mut self) -> Option<bool> {
        if self.pending {
            return None;
        }
        self.on = !self.on;
        self.pending = true;
        Some(self.on)
    }

    /// The server answered the request for `asked`. A failure puts the button back; returns
    /// what to tell the user then.
    pub fn answered(
        &mut self,
        asked: bool,
        answer: &Result<(), ProviderError>,
    ) -> Option<&'static str> {
        self.pending = false;
        if answer.is_ok() {
            return None;
        }
        self.on = !asked;
        Some(FAILED)
    }
}

/// Puts `item` in the list or takes it out, on the server.
pub fn ask(server: &dyn MediaServer, item: &ItemId, wanted: bool) -> Result<(), ProviderError> {
    server.set_favourite(item, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FakeServer};

    fn film() -> ItemId {
        ItemId(FILM.into())
    }

    /// A press, the request it asks for and its answer, as the page does them in order.
    fn press_and_ask(toggle: &mut Toggle, server: &FakeServer) -> Option<&'static str> {
        let wanted = toggle.press()?;
        let answer = ask(server, &film(), wanted);
        toggle.answered(wanted, &answer)
    }

    #[test]
    fn pressing_ma_liste_shows_the_new_state_before_the_server_answers() {
        let mut toggle = Toggle::default();
        assert_eq!(toggle.press(), Some(true));
        assert_eq!(
            toggle,
            Toggle {
                on: true,
                pending: true
            }
        );
        toggle.answered(true, &Ok(()));
        assert_eq!(toggle.press(), Some(false));
        assert!(!toggle.on());
    }

    #[test]
    fn an_item_put_in_the_list_is_in_the_list_on_the_server() {
        let server = FakeServer::new(3, false);
        let mut toggle = Toggle::default();
        assert_eq!(press_and_ask(&mut toggle, &server), None);
        assert_eq!(
            toggle,
            Toggle {
                on: true,
                pending: false
            }
        );
        assert!(server.state.lock().unwrap().favourites.contains(&film()));
        assert_eq!(press_and_ask(&mut toggle, &server), None);
        assert!(!toggle.on());
        assert!(!server.state.lock().unwrap().favourites.contains(&film()));
    }

    #[test]
    fn a_failed_request_puts_the_button_back_and_says_so() {
        let server = FakeServer::new(3, false);
        server.state.lock().unwrap().favourite_fails = true;
        let mut toggle = Toggle::default();
        let wanted = toggle.press().unwrap();
        assert!(toggle.on(), "shown at once, before the answer");
        let answer = ask(&server, &film(), wanted);
        assert_eq!(toggle.answered(wanted, &answer), Some(FAILED));
        assert_eq!(
            toggle,
            Toggle {
                on: false,
                pending: false
            }
        );
        assert!(!server.state.lock().unwrap().favourites.contains(&film()));
    }

    #[test]
    fn a_failure_taking_an_item_out_puts_it_back_in_the_list() {
        let mut toggle = Toggle::default();
        toggle.sync(true);
        let wanted = toggle.press().unwrap();
        assert!(!wanted && !toggle.on());
        assert_eq!(
            toggle.answered(wanted, &Err(ProviderError::Server(500))),
            Some(FAILED)
        );
        assert!(toggle.on());
    }

    #[test]
    fn a_second_press_while_the_first_is_on_its_way_asks_nothing() {
        let server = FakeServer::new(3, false);
        let mut toggle = Toggle::default();
        let wanted = toggle.press().unwrap();
        assert_eq!(toggle.press(), None);
        assert_eq!(toggle.press(), None);
        let answer = ask(&server, &film(), wanted);
        toggle.answered(wanted, &answer);
        let calls = server.state.lock().unwrap().record.favourite_calls.clone();
        assert_eq!(calls, [(film(), true)]);
        // Once answered, the button takes a press again.
        assert_eq!(toggle.press(), Some(false));
    }

    #[test]
    fn the_server_state_read_with_the_page_is_kept_when_nothing_is_on_its_way() {
        let mut toggle = Toggle::default();
        toggle.sync(true);
        assert!(toggle.on());
        toggle.sync(false);
        assert!(!toggle.on());
    }

    #[test]
    fn the_server_state_read_while_a_request_is_on_its_way_does_not_undo_the_press() {
        let mut toggle = Toggle::default();
        toggle.press().unwrap();
        // The page was loaded again before the server took the change.
        toggle.sync(false);
        assert!(toggle.on());
    }
}
