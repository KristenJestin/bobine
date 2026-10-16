//! The user's list ("Ma liste") and the collections: what the pages show of them, and how it
//! comes from the server. Every call runs off the window's thread, like the others.

use bobine::provider::{CollectionCard, CollectionDetail, Favourite, ItemId, ProviderError};
use gpui_kit::{Context, ScrollHandle, UniformListScrollHandle};

use super::Shell;
use super::pages::Loadable;
use crate::favourite;
use crate::navigation::Screen;

/// The collections of the server.
#[derive(Default)]
pub struct CollectionsPage {
    pub cards: Option<Loadable<Vec<CollectionCard>>>,
    pub scroll: UniformListScrollHandle,
}

/// One collection and its films.
pub struct CollectionPage {
    pub id: ItemId,
    pub detail: Loadable<CollectionDetail>,
    pub scroll: ScrollHandle,
}

/// The user's list: films and series together.
#[derive(Default)]
pub struct MyListPage {
    pub items: Option<Loadable<Vec<Favourite>>>,
    pub scroll: UniformListScrollHandle,
}

impl<T> Loadable<T> {
    /// What shows while the server is asked again: what was drawn stays, a first or a failed
    /// load shows the wait.
    fn asking(slot: &mut Option<Loadable<T>>) {
        if !matches!(slot, Some(Loadable::Ready(_))) {
            *slot = Some(Loadable::Loading);
        }
    }
}

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

    /// The collections, once; asked again when the first try failed.
    pub fn load_collections(&mut self, cx: &mut Context<Self>) {
        if matches!(
            self.collections.cards,
            Some(Loadable::Ready(_) | Loadable::Loading)
        ) {
            return;
        }
        self.collections.cards = Some(Loadable::Loading);
        self.ask(
            cx,
            |server| server.collections(),
            |shell, cards, cx| {
                shell.collections.cards = Some(match cards {
                    Ok(cards) => Loadable::Ready(cards),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                });
            },
        );
    }

    /// The user's list, each time it shows: a title put in it from a page shows at once.
    pub fn load_my_list(&mut self, cx: &mut Context<Self>) {
        Loadable::asking(&mut self.my_list.items);
        self.ask(
            cx,
            |server| server.favourites(),
            |shell, items, cx| {
                shell.my_list.items = Some(match items {
                    Ok(items) => Loadable::Ready(items),
                    Err(error) => Loadable::Failed(shell.failed(error, cx)),
                });
            },
        );
    }

    pub fn open_collection(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.collection_pages.push(CollectionPage {
            id,
            detail: Loadable::Loading,
            scroll: ScrollHandle::new(),
        });
        self.navigator.open(Screen::CollectionDetail);
        self.load_collection_page(self.collection_pages.len() - 1, cx);
        cx.notify();
    }

    /// Back on a collection page from a film: its films' marks are asked again.
    pub(super) fn reload_collection(&mut self, cx: &mut Context<Self>) {
        if let Some(depth) = self.collection_pages.len().checked_sub(1) {
            self.load_collection_page(depth, cx);
        }
    }

    fn load_collection_page(&mut self, depth: usize, cx: &mut Context<Self>) {
        let id = self.collection_pages[depth].id.clone();
        let asked = id.clone();
        self.ask(
            cx,
            move |server| server.collection(&asked),
            move |shell, detail: Result<CollectionDetail, ProviderError>, cx| {
                let detail = match detail {
                    Ok(detail) => Loadable::Ready(detail),
                    // What was drawn stays when the refresh fails.
                    Err(error) => {
                        let said = shell.failed(error, cx);
                        match shell.collection_pages.get(depth) {
                            Some(page) if matches!(page.detail, Loadable::Ready(_)) => return,
                            _ => Loadable::Failed(said),
                        }
                    }
                };
                if let Some(page) = shell
                    .collection_pages
                    .get_mut(depth)
                    .filter(|page| page.id == id)
                {
                    page.detail = detail;
                }
            },
        );
    }
}
