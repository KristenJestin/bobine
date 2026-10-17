//! The page of a TMDb collection on the request server (#81): every movie of it, marked in the
//! library, requested or missing, those missing checked, and "Request collection", which asks for
//! the checked ones one after the other, as Seerr's own collection request does. Reached by
//! "Part of …" on a request page and on a library movie's page.

use bobine::requests::{
    self, Ask, CollectionParts, CollectionRef, CollectionRun, MediaKind, RequestServer, TmdbId,
};
use gpui_kit::{Context, ScrollHandle, ScrollWheelEvent, Window};

use super::pages::route_wheel;
use super::{Loadable, Shell};
use crate::discover::{self, FilmPicker};
use crate::navigation::Screen;
use crate::scroll;

pub struct SeerrCollectionPage {
    pub id: TmdbId,
    /// Told apart from every other page ever opened, the closed ones included.
    generation: u64,
    pub parts: Loadable<CollectionParts>,
    /// The movies it may ask for, once the library is indexed: none is offered before.
    pub picker: Option<FilmPicker>,
    /// The requests on their way: once the page is gone, what is left is not sent.
    run: Option<CollectionRun>,
    /// How the last request went, movie by movie, and whether all of them went.
    pub outcome: Option<(String, bool)>,
    pub scroll: ScrollHandle,
    pub rows: [ScrollHandle; 1],
    gesture: scroll::Gesture,
}

impl SeerrCollectionPage {
    fn new(id: TmdbId, generation: u64) -> Self {
        Self {
            id,
            generation,
            parts: Loadable::Loading,
            picker: None,
            run: None,
            outcome: None,
            scroll: ScrollHandle::new(),
            rows: Default::default(),
            gesture: scroll::Gesture::default(),
        }
    }
}

impl SeerrCollectionPage {
    pub fn busy(&self) -> bool {
        self.run.is_some()
    }

    /// The movies checked, at this moment.
    fn wanted(&self) -> Vec<TmdbId> {
        self.picker
            .as_ref()
            .map(FilmPicker::chosen)
            .unwrap_or_default()
    }
}

impl Shell {
    /// The collection's page, from a request page or a library movie's.
    pub fn open_seerr_collection(&mut self, id: TmdbId, cx: &mut Context<Self>) {
        if self.seerr.client.is_none() {
            return;
        }
        self.forward.clear();
        let generation = self.next_generation();
        self.seerr
            .collections
            .push(SeerrCollectionPage::new(id, generation));
        self.navigator.open(Screen::SeerrCollection);
        self.want_tmdb_index(cx);
        self.load_seerr_collection(generation, cx);
        cx.notify();
    }

    fn seerr_collection_numbered(&mut self, generation: u64) -> Option<&mut SeerrCollectionPage> {
        self.seerr
            .collections
            .iter_mut()
            .find(|page| page.generation == generation)
    }

    fn load_seerr_collection(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(page) = self.seerr_collection_numbered(generation) else {
            return;
        };
        let id = page.id;
        self.ask_seerr(
            cx,
            move |seerr| seerr.collection(id),
            move |shell, parts, _| {
                let Some(page) = shell.seerr_collection_numbered(generation) else {
                    return;
                };
                match parts {
                    Ok(parts) => page.parts = Loadable::Ready(parts),
                    Err(error) if !matches!(page.parts, Loadable::Ready(_)) => {
                        page.parts = Loadable::Failed(discover::error_line(&error));
                    }
                    Err(_) => {}
                }
            },
        );
    }

    /// The collection page shown, its movies offered once the library is indexed: those
    /// neither in it nor had or asked for by the request server, all checked at first.
    pub fn seerr_collection_shown(&mut self) -> Option<&mut SeerrCollectionPage> {
        let index = self.seerr.index.clone();
        let page = self.seerr.collections.last_mut()?;
        if page.picker.is_none()
            && let (Some(index), Loadable::Ready(parts)) = (&index, &page.parts)
        {
            let missing = requests::missing(&parts.films, |kind, tmdb| {
                discover::in_library(Some(index), kind, tmdb).is_some()
            });
            page.picker = Some(FilmPicker::new(missing));
        }
        Some(page)
    }

    /// A movie's box checked or unchecked.
    pub fn toggle_collection_film(&mut self, film: TmdbId, cx: &mut Context<Self>) {
        let Some(page) = self.seerr_collection_shown() else {
            return;
        };
        if page.busy() {
            return;
        }
        if let Some(picker) = &mut page.picker {
            picker.toggle(film);
            cx.notify();
        }
    }

    /// "Request collection": the checked movies are asked for, one after the other, off the
    /// window's thread. A quota reached or a failure stops them, and a line names which went
    /// and which did not.
    pub fn request_collection(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.seerr_collection_shown() else {
            return;
        };
        if page.busy() || page.wanted().is_empty() {
            return;
        }
        page.run = Some(CollectionRun::default());
        page.outcome = None;
        let generation = page.generation;
        self.ask_seerr(
            cx,
            |seerr| seerr.quota_reached(MediaKind::Movie),
            move |shell, quota, cx| {
                let Some(page) = shell.seerr_collection_numbered(generation) else {
                    return;
                };
                let wanted = page.wanted();
                if let Some(run) = &mut page.run {
                    run.quota(quota, &wanted);
                }
                shell.request_next_film(generation, cx);
            },
        );
        cx.notify();
    }

    /// The next movie of the run asked for, once the answer for the one before came; the line
    /// of the run once none is left. A page gone meanwhile asks for nothing more.
    fn request_next_film(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some(page) = self.seerr_collection_numbered(generation) else {
            return;
        };
        let wanted = page.wanted();
        let Some(run) = &page.run else {
            return;
        };
        let Some(film) = run.next(&wanted) else {
            let outcome = page.run.take().map(|run| run.outcome).unwrap_or_default();
            if let Loadable::Ready(parts) = &page.parts {
                let all = outcome.not_requested.is_empty();
                page.outcome = Some((discover::collection_line(&outcome, &parts.films), all));
            }
            // What was asked for is offered no more; what was not stays checked.
            if let Some(picker) = &mut page.picker {
                picker.asked(&outcome.requested);
            }
            return;
        };
        self.ask_seerr(
            cx,
            move |seerr| seerr.request(&Ask::Movie(film)),
            move |shell, answer, cx| {
                if answer.is_ok() {
                    shell.mark_requested(MediaKind::Movie, film);
                }
                let Some(page) = shell.seerr_collection_numbered(generation) else {
                    return;
                };
                let wanted = page.wanted();
                if let Some(run) = &mut page.run {
                    run.answered(film, answer, &wanted);
                }
                shell.request_next_film(generation, cx);
            },
        );
    }

    /// The collection a library movie is part of, for its "Part of …": Jellyfin's TMDb
    /// collection id when it has one (Seerr then names it), else what Seerr says of the movie.
    /// Read once per movie.
    pub(super) fn load_collection_link(
        &mut self,
        tmdb: Option<TmdbId>,
        collection: Option<TmdbId>,
        cx: &mut Context<Self>,
    ) {
        let Some(tmdb) = tmdb else {
            return;
        };
        if self.seerr.client.is_none() || self.seerr.links.contains_key(&tmdb) {
            return;
        }
        self.seerr.links.insert(tmdb, None);
        let read = move |seerr: &dyn RequestServer| match collection {
            Some(id) => seerr.collection(id).map(|parts| {
                Some(CollectionRef {
                    id: parts.id,
                    name: parts.name,
                })
            }),
            None => seerr
                .title(MediaKind::Movie, tmdb)
                .map(|detail| detail.collection),
        };
        self.ask_seerr(cx, read, move |shell, link, _| match link {
            Ok(link) => {
                shell.seerr.links.insert(tmdb, link);
            }
            // Asked again the next time the page opens.
            Err(_) => {
                shell.seerr.links.remove(&tmdb);
            }
        });
    }

    /// The collection a library movie is part of, once known.
    pub fn collection_link(&self, tmdb: Option<TmdbId>) -> Option<CollectionRef> {
        self.seerr.client.as_ref()?;
        self.seerr.links.get(&tmdb?)?.clone()
    }

    pub fn scroll_seerr_collection(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.seerr.collections.last_mut() else {
            return;
        };
        if route_wheel(&page.scroll, &page.rows, &mut page.gesture, event, window) {
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use bobine::provider::{ItemId, ProviderError};
    use gpui_kit::{TestAppContext, WindowHandle};

    use super::super::discover::tests::{address_only, click, configured, configured_on, drawn};
    use super::*;
    use crate::fake::{FILM, FakeServer, IN_LIBRARY_FILM};
    use crate::fake_seerr::{COLLECTION, FakeSeerr, WINTER_LIGHT, WINTERS_END};

    /// A shell on the collection page of the fake's Winter Saga: The Tides and The Last Winter
    /// in the library, Winter Light and Winter's End missing.
    fn on_collection(seerr: &FakeSeerr, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        on_collection_of(FakeServer::new(6, true), seerr, cx)
    }

    fn on_collection_of(
        server: FakeServer,
        seerr: &FakeSeerr,
        cx: &mut TestAppContext,
    ) -> WindowHandle<Shell> {
        let window = configured_on(server, seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_seerr_collection(COLLECTION, cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    fn outcome(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<String> {
        window
            .update(cx, |shell, _, _| {
                let page = shell.seerr.collections.last().unwrap();
                page.outcome.as_ref().map(|(line, _)| line.clone())
            })
            .unwrap()
    }

    #[gpui_kit::test]
    fn a_collection_with_two_movies_in_the_library_and_two_missing_requests_only_the_two_missing_ones(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        let window = on_collection(&seerr, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("collection-page").is_some());
        for card in [
            "discover-card Winter Light",
            "discover-card The Tides",
            "discover-card The Last Winter",
            "discover-card Winter's End",
        ] {
            assert!(visual.debug_bounds(card).is_some(), "{card} is listed");
        }
        // Only the missing ones have a box; those in the library say so.
        assert!(
            visual
                .debug_bounds("collection-check Winter Light")
                .is_some()
        );
        assert!(
            visual
                .debug_bounds("collection-check Winter's End")
                .is_some()
        );
        assert!(visual.debug_bounds("collection-check The Tides").is_none());
        assert!(
            visual
                .debug_bounds("collection-check The Last Winter")
                .is_none()
        );
        assert!(visual.debug_bounds("status In library").is_some());
        assert!(seerr.requests().is_empty(), "opening asks for nothing");
        click(&mut visual, "collection-request");
        assert_eq!(
            seerr.requests(),
            [Ask::Movie(WINTER_LIGHT), Ask::Movie(WINTERS_END)]
        );
        assert_eq!(
            outcome(&window, cx).as_deref(),
            Some("Requested: Winter Light, Winter's End.")
        );
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("collection-outcome").is_some());
        assert!(visual.debug_bounds("status Pending").is_some());
        // Nothing is left to ask for.
        assert!(visual.debug_bounds("collection-request").is_none());
    }

    #[gpui_kit::test]
    fn a_movie_unchecked_is_not_requested(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_collection(&seerr, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "collection-check Winter Light");
        click(&mut visual, "collection-request");
        assert_eq!(seerr.requests(), [Ask::Movie(WINTERS_END)]);
        assert_eq!(
            screen(&window, cx),
            Screen::SeerrCollection,
            "a box opens nothing"
        );
    }

    #[gpui_kit::test]
    fn a_quota_reached_after_the_first_request_says_which_movie_was_requested_and_which_was_not(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().movie_quota = Some(1);
        let window = on_collection(&seerr, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "collection-request");
        assert_eq!(seerr.requests(), [Ask::Movie(WINTER_LIGHT)]);
        assert_eq!(
            outcome(&window, cx).as_deref(),
            Some("Requested: Winter Light. Not requested (quota reached): Winter's End.")
        );
        // The one not requested is still offered, checked.
        window
            .update(cx, |shell, _, _| {
                let picker = shell
                    .seerr
                    .collections
                    .last()
                    .unwrap()
                    .picker
                    .clone()
                    .unwrap();
                assert_eq!(picker.chosen(), [WINTERS_END]);
                assert!(!picker.is_offered(WINTER_LIGHT));
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("collection-request").is_some());
    }

    #[gpui_kit::test]
    fn a_double_press_sends_each_movie_once(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_collection(&seerr, cx);
        drawn(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.request_collection(cx);
                shell.request_collection(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            seerr.requests(),
            [Ask::Movie(WINTER_LIGHT), Ask::Movie(WINTERS_END)]
        );
    }

    #[gpui_kit::test]
    fn closing_the_collection_page_stops_the_requests_not_sent_yet(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_collection(&seerr, cx);
        drawn(&window, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.request_collection(cx);
                assert!(shell.go_back(cx));
            })
            .unwrap();
        cx.run_until_parked();
        assert!(seerr.requests().is_empty());
    }

    #[gpui_kit::test]
    fn signing_out_stops_the_requests_not_sent_yet(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = on_collection(&seerr, cx);
        drawn(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                shell.request_collection(cx);
                shell.sign_out(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert!(seerr.requests().is_empty());
    }

    #[gpui_kit::test]
    fn without_the_library_indexed_no_movie_is_offered(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().tmdb_index_error =
            Some(ProviderError::Unreachable("down".into()));
        let window = on_collection_of(server, &seerr, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("discover-card Winter Light").is_some());
        assert!(
            visual
                .debug_bounds("collection-check Winter Light")
                .is_none()
        );
        assert!(visual.debug_bounds("collection-request").is_none());
    }

    #[gpui_kit::test]
    fn a_library_movie_in_a_collection_links_to_the_collection_page(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().tmdb_collection = Some(COLLECTION);
        let window = configured_on(server, &seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        // Jellyfin knows the collection: Seerr is asked its name, not the movie's.
        assert!(
            !seerr
                .state
                .lock()
                .unwrap()
                .record
                .titles
                .contains(&IN_LIBRARY_FILM),
            "the movie was read on Seerr"
        );
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("part-of-collection").is_some());
        click(&mut visual, "part-of-collection");
        assert_eq!(screen(&window, cx), Screen::SeerrCollection);
        let mut visual = drawn(&window, cx);
        assert!(
            visual
                .debug_bounds("collection-check Winter Light")
                .is_some()
        );
        // Back on the movie's page.
        window.update(cx, |shell, _, cx| shell.escape(cx)).unwrap();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
    }

    #[gpui_kit::test]
    fn a_library_movie_jellyfin_knows_no_collection_of_takes_it_from_seerr(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "part-of-collection");
        assert_eq!(screen(&window, cx), Screen::SeerrCollection);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.seerr.collections.last().unwrap().id, COLLECTION);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn without_seerr_a_library_movie_shows_no_collection_link(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = address_only(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("film-play").is_some());
        assert!(visual.debug_bounds("part-of-collection").is_none());
    }

    #[gpui_kit::test]
    fn a_request_page_of_a_movie_in_a_collection_links_to_the_collection_page(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_request(MediaKind::Movie, WINTER_LIGHT, cx)
            })
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "part-of-collection");
        assert_eq!(screen(&window, cx), Screen::SeerrCollection);
        window
            .update(cx, |shell, _, cx| assert!(shell.go_back(cx)))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Request);
    }
}
