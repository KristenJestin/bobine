//! The "My list" button of a film or a series page. Pressing it changes what the button shows
//! at once and asks the server; if the server fails, the button goes back and says so. While a
//! request is on its way the button asks nothing more, so that two presses never leave the
//! server and the page disagreeing about the order they came in.

use bobine::provider::{ItemId, MediaServer, ProviderError};

use crate::navigation::Screen;

/// What the page says when the server did not take the change.
pub const FAILED: &str = "My list could not be changed.";

/// Whether the item is in the list, and whether the server is being asked to change it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Toggle {
    on: bool,
    pending: bool,
    /// How many times the state changed on the page's own account (a press, an answer, another
    /// page of the same title): a read of the server sent before that is out of date.
    changes: u32,
}

impl Toggle {
    /// The item is in the list, as far as the page shows.
    pub fn on(&self) -> bool {
        self.on
    }

    /// The mark to take when a read of the server is sent, and give back with its answer.
    pub fn changes(&self) -> u32 {
        self.changes
    }

    /// What the server says, when the page is loaded again; `read_at` is the mark taken when
    /// the read was sent. While a change is on its way the page knows better than the server
    /// does, and so it does when a change happened since the read was sent: the read may have
    /// been answered before the server took it.
    pub fn sync(&mut self, server_says: bool, read_at: u32) {
        if !self.pending && read_at == self.changes {
            self.on = server_says;
        }
    }

    /// Another page of the same title changed it on the server: this one shows it too, unless
    /// it has a change of its own on its way (its answer carries the last word).
    pub fn follow(&mut self, now: bool) {
        if !self.pending {
            self.on = now;
            self.changes += 1;
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
        self.changes += 1;
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
        self.changes += 1;
        if answer.is_ok() {
            return None;
        }
        self.on = !asked;
        Some(FAILED)
    }
}

/// A page with a "My list" button: a film page or a series page.
pub trait ListPage {
    fn item(&self) -> &ItemId;
    /// Told apart from every other page ever opened, the ones closed included.
    fn generation(&self) -> u64;
    /// The button, and the message of the page where a failure is said.
    fn button(&mut self) -> (&mut Toggle, &mut Option<&'static str>);
}

/// A press that was sent to the server: which page made it and what it asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pressed {
    pub page: u64,
    pub item: ItemId,
    pub wanted: bool,
}

/// A press on `page`'s button, if it takes one.
pub fn press(page: &mut dyn ListPage) -> Option<Pressed> {
    let page_id = page.generation();
    let item = page.item().clone();
    let (toggle, error) = page.button();
    let wanted = toggle.press()?;
    *error = None;
    Some(Pressed {
        page: page_id,
        item,
        wanted,
    })
}

/// The server answered `pressed`. Only the page that pressed it takes the answer (a page closed
/// meanwhile has no one to take it, even if the same title was opened in its place); on a
/// success every other page of the same title shows the change too. Returns whether the user's
/// list, if it is the screen shown, has to be asked again.
pub fn answered<'a>(
    pages: impl IntoIterator<Item = &'a mut dyn ListPage>,
    current: Screen,
    pressed: &Pressed,
    answer: &Result<(), ProviderError>,
) -> bool {
    for page in pages {
        if page.generation() == pressed.page {
            let (toggle, error) = page.button();
            if let Some(said) = toggle.answered(pressed.wanted, answer) {
                *error = Some(said);
            }
        } else if answer.is_ok() && page.item() == &pressed.item {
            page.button().0.follow(pressed.wanted);
        }
    }
    answer.is_ok() && current == Screen::MyList
}

/// The page numbered `generation`, at `depth` of its stack, if it is still open there: the read
/// of a page answers to it and to no other that shows the same title.
pub fn numbered<P: ListPage>(stack: &mut [P], depth: usize, generation: u64) -> Option<&mut P> {
    stack
        .get_mut(depth)
        .filter(|page| page.generation() == generation)
}

/// The page to read again after the server's answer: when a press failed, the page that made
/// it shows the state it had before, which another page's change may have made out of date.
pub fn reread(pressed: &Pressed, answer: &Result<(), ProviderError>) -> Option<u64> {
    answer.is_err().then_some(pressed.page)
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
                pending: true,
                changes: 1
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
                pending: false,
                changes: 2
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
                pending: false,
                changes: 2
            }
        );
        assert!(!server.state.lock().unwrap().favourites.contains(&film()));
    }

    #[test]
    fn a_failure_taking_an_item_out_puts_it_back_in_the_list() {
        let mut toggle = Toggle::default();
        toggle.sync(true, toggle.changes());
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
        toggle.sync(true, toggle.changes());
        assert!(toggle.on());
        toggle.sync(false, toggle.changes());
        assert!(!toggle.on());
    }

    #[test]
    fn the_server_state_read_while_a_request_is_on_its_way_does_not_undo_the_press() {
        let mut toggle = Toggle::default();
        toggle.press().unwrap();
        // The page was loaded again before the server took the change.
        toggle.sync(false, toggle.changes());
        assert!(toggle.on());
    }

    #[test]
    fn a_read_sent_before_a_press_does_not_undo_it() {
        let mut toggle = Toggle::default();
        toggle.sync(true, toggle.changes());
        // The player closed and the film was asked; the user pressed before it came back.
        let read_at = toggle.changes();
        let wanted = toggle.press().unwrap();
        assert!(!wanted);
        // The press was answered first, so nothing is on its way when the old read lands.
        toggle.answered(wanted, &Ok(()));
        toggle.sync(true, read_at);
        assert!(!toggle.on(), "the old read said the title was still in");
    }

    #[test]
    fn a_read_sent_while_a_press_is_on_its_way_does_not_undo_it_either() {
        let mut toggle = Toggle::default();
        let wanted = toggle.press().unwrap();
        // The server may have answered this read before it took the change.
        let read_at = toggle.changes();
        toggle.answered(wanted, &Ok(()));
        toggle.sync(false, read_at);
        assert!(toggle.on());
    }

    #[test]
    fn a_read_sent_after_the_last_press_was_answered_is_kept() {
        let mut toggle = Toggle::default();
        let wanted = toggle.press().unwrap();
        toggle.answered(wanted, &Ok(()));
        toggle.sync(false, toggle.changes());
        assert!(!toggle.on());
    }

    #[test]
    fn a_title_changed_on_another_page_shows_the_change_here() {
        let mut toggle = Toggle::default();
        toggle.sync(true, toggle.changes());
        let read_at = toggle.changes();
        toggle.follow(false);
        assert!(!toggle.on());
        // A read sent before that is out of date too.
        toggle.sync(true, read_at);
        assert!(!toggle.on());
    }

    #[test]
    fn a_page_with_a_press_of_its_own_on_its_way_does_not_follow_another() {
        let mut toggle = Toggle::default();
        let wanted = toggle.press().unwrap();
        toggle.follow(false);
        assert!(toggle.on());
        toggle.answered(wanted, &Ok(()));
        assert!(toggle.on());
    }

    /// A page of the stack, as the shell holds them.
    struct Page {
        item: ItemId,
        generation: u64,
        toggle: Toggle,
        error: Option<&'static str>,
    }

    impl Page {
        fn new(item: &str, generation: u64, in_list: bool) -> Self {
            let mut toggle = Toggle::default();
            toggle.sync(in_list, toggle.changes());
            Page {
                item: ItemId(item.into()),
                generation,
                toggle,
                error: None,
            }
        }
    }

    impl ListPage for Page {
        fn item(&self) -> &ItemId {
            &self.item
        }

        fn generation(&self) -> u64 {
            self.generation
        }

        fn button(&mut self) -> (&mut Toggle, &mut Option<&'static str>) {
            (&mut self.toggle, &mut self.error)
        }
    }

    fn answer_to(
        stack: &mut [Page],
        current: Screen,
        pressed: &Pressed,
        answer: Result<(), ProviderError>,
    ) -> bool {
        answered(
            stack.iter_mut().map(|p| p as &mut dyn ListPage),
            current,
            pressed,
            &answer,
        )
    }

    #[test]
    fn a_change_shows_on_every_page_of_the_same_title() {
        // My list, film A, its collection, film A again: removed on the top page.
        let mut stack = [
            Page::new("film-a", 1, true),
            Page::new("film-b", 2, false),
            Page::new("film-a", 3, true),
        ];
        let pressed = press(&mut stack[2]).unwrap();
        assert_eq!(
            pressed,
            Pressed {
                page: 3,
                item: ItemId("film-a".into()),
                wanted: false
            }
        );
        answer_to(&mut stack, Screen::FilmDetail, &pressed, Ok(()));
        assert!(!stack[2].toggle.on());
        assert!(!stack[0].toggle.on(), "back on the first film A page");
        assert!(!stack[1].toggle.on(), "another title is left alone");
        assert_eq!(stack[0].error, None);
    }

    #[test]
    fn a_failure_is_said_on_the_page_that_pressed_and_changes_no_other() {
        let mut stack = [Page::new("film-a", 1, true), Page::new("film-a", 2, true)];
        let pressed = press(&mut stack[1]).unwrap();
        answer_to(
            &mut stack,
            Screen::FilmDetail,
            &pressed,
            Err(ProviderError::Server(500)),
        );
        assert!(stack[1].toggle.on(), "put back");
        assert_eq!(stack[1].error, Some(FAILED));
        assert!(stack[0].toggle.on());
        assert_eq!(stack[0].error, None);
    }

    #[test]
    fn a_change_answered_after_its_page_closed_still_shows_on_the_pages_left() {
        let mut stack = [Page::new("film-a", 1, true), Page::new("film-a", 2, true)];
        let pressed = press(&mut stack[1]).unwrap();
        // Escape before the answer.
        let mut stack = [stack.into_iter().next().unwrap()];
        answer_to(&mut stack, Screen::FilmDetail, &pressed, Ok(()));
        assert!(!stack[0].toggle.on());
    }

    #[test]
    fn an_answer_for_a_closed_page_never_reaches_a_reopened_one() {
        let mut first = [Page::new("film-a", 1, true)];
        let pressed = press(&mut first[0]).unwrap();
        // Escape, then the same title is opened again at the same depth: another page.
        let mut reopened = [Page::new("film-a", 2, true)];
        let again = press(&mut reopened[0]).unwrap();
        // The first press fails, late: the reopened page's own press is still on its way.
        answer_to(
            &mut reopened,
            Screen::FilmDetail,
            &pressed,
            Err(ProviderError::Server(500)),
        );
        assert_eq!(reopened[0].error, None, "the error is not for this page");
        assert!(!reopened[0].toggle.on(), "its own press still shows");
        assert_eq!(
            press(&mut reopened[0]),
            None,
            "still waiting for its own answer"
        );
        answer_to(&mut reopened, Screen::FilmDetail, &again, Ok(()));
        assert_eq!(press(&mut reopened[0]).map(|p| p.wanted), Some(true));
    }

    #[test]
    fn the_read_of_a_page_never_reaches_a_page_opened_after_it_at_its_depth() {
        let mut stack = [Page::new("film-a", 1, true)];
        assert!(numbered(&mut stack, 0, 1).is_some());
        // Escape, the same title is opened again at the same depth: another page.
        let mut stack = [Page::new("film-a", 2, true)];
        assert!(numbered(&mut stack, 0, 1).is_none());
        assert!(numbered(&mut stack, 0, 2).is_some());
        // And not at another depth.
        assert!(numbered(&mut stack, 1, 2).is_none());
    }

    #[test]
    fn a_press_that_fails_after_another_page_changed_the_same_title_is_read_again() {
        // Two pages of one title, both showing it out of the list.
        let mut stack = [Page::new("film-a", 1, false), Page::new("film-a", 2, false)];
        let first = press(&mut stack[0]).unwrap();
        let second = press(&mut stack[1]).unwrap();
        assert_eq!((first.wanted, second.wanted), (true, true));
        // The first is answered: the server has the title, the second page has a press of its
        // own on its way and so does not follow.
        answer_to(&mut stack, Screen::FilmDetail, &first, Ok(()));
        assert!(stack[1].toggle.on(), "its own press still shows");
        // The second fails: it puts itself back to out of the list, which the server does not say.
        let failed = Err(ProviderError::Server(500));
        let again = reread(&second, &failed);
        answer_to(&mut stack, Screen::FilmDetail, &second, failed);
        assert!(!stack[1].toggle.on(), "stale until read again");
        assert_eq!(again, Some(2));
        // The read sent then, answered: the page shows what the server holds.
        let read_at = stack[1].toggle.changes();
        stack[1].toggle.sync(true, read_at);
        assert!(stack[1].toggle.on());
    }

    #[test]
    fn a_press_that_succeeds_is_not_read_again() {
        let mut stack = [Page::new("film-a", 1, false)];
        let pressed = press(&mut stack[0]).unwrap();
        assert_eq!(reread(&pressed, &Ok(())), None);
    }

    #[test]
    fn ma_liste_is_asked_again_when_the_change_is_answered_on_top_of_it() {
        let mut stack = [Page::new("film-a", 1, true)];
        let pressed = press(&mut stack[0]).unwrap();
        // My list → film → remove → Escape before the answer: My list is the screen shown,
        // and it was asked while the server was taking the change.
        assert!(answer_to(&mut stack, Screen::MyList, &pressed, Ok(())));
    }

    #[test]
    fn ma_liste_is_left_alone_when_it_is_not_the_screen_shown_or_the_change_failed() {
        let mut stack = [Page::new("film-a", 1, true)];
        let pressed = press(&mut stack[0]).unwrap();
        assert!(!answer_to(&mut stack, Screen::FilmDetail, &pressed, Ok(())));
        let mut stack = [Page::new("film-a", 1, true)];
        let pressed = press(&mut stack[0]).unwrap();
        assert!(!answer_to(
            &mut stack,
            Screen::MyList,
            &pressed,
            Err(ProviderError::Server(500))
        ));
    }
}
