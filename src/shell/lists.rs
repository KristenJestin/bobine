//! The user's list ("Ma liste") and the collections: what the pages show of them, and how it
//! comes from the server. Every call runs off the window's thread, like the others.

use bobine::provider::ItemId;
use gpui_kit::Context;

use super::Shell;
use crate::favourite;
use crate::navigation::Screen;

impl Shell {
    /// The "Ma liste" button of the film or series page shown: the button changes at once, the
    /// server is asked, and a failure puts the button back and says so on the page.
    pub fn toggle_favourite(&mut self, cx: &mut Context<Self>) {
        let screen = self.navigator.current();
        let (depth, id, wanted) = match screen {
            Screen::FilmDetail => {
                let depth = self.details.len().wrapping_sub(1);
                let Some(page) = self.details.last_mut() else {
                    return;
                };
                let Some(wanted) = page.favourite.press() else {
                    return;
                };
                page.error = None;
                (depth, page.id.clone(), wanted)
            }
            Screen::SeriesDetail => {
                let depth = self.series_pages.len().wrapping_sub(1);
                let Some(page) = self.series_pages.last_mut() else {
                    return;
                };
                let Some(wanted) = page.favourite.press() else {
                    return;
                };
                page.error = None;
                (depth, page.id.clone(), wanted)
            }
            _ => return,
        };
        cx.notify();
        let asked = id.clone();
        self.ask(
            cx,
            move |server| favourite::ask(server, &asked, wanted),
            move |shell, answer, cx| {
                if let Err(error) = &answer {
                    // An expired session brings the login back; the page says what failed.
                    shell.failed(error.clone(), cx);
                }
                shell.favourite_answered(screen, depth, &id, wanted, &answer);
            },
        );
    }

    fn favourite_answered(
        &mut self,
        screen: Screen,
        depth: usize,
        id: &ItemId,
        wanted: bool,
        answer: &Result<(), bobine::provider::ProviderError>,
    ) {
        let (toggle, error) = match screen {
            Screen::FilmDetail => match self.detail_page(depth, id) {
                Some(page) => (&mut page.favourite, &mut page.error),
                None => return,
            },
            _ => match self.series_page(depth, id) {
                Some(page) => (&mut page.favourite, &mut page.error),
                None => return,
            },
        };
        if let Some(said) = toggle.answered(wanted, answer) {
            *error = Some(said);
        }
    }
}
