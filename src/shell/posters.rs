//! The buttons a poster shows under the pointer: "My list" and "Watched" change at once and ask
//! the server; a refusal puts them back and the window says so for a moment.

use std::time::Duration;

use bobine::provider::{ItemId, ProviderError};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Context, ElementId, Entity, Window};

use super::Shell;
use super::pages::Loadable;
use crate::favourite::ListPage;
use crate::navigation::Screen;
use crate::poster_actions::{self, Action};
use crate::screens::{self, Card, Offer};

/// How long a message of the window stays before it fades out.
const NOTICE_FOR: Duration = Duration::from_secs(4);

impl Shell {
    /// The card `id`, a view of its own.
    pub fn card(&mut self, id: &ElementId, offer: Offer, cx: &mut Context<Self>) -> Entity<Card> {
        self.cards.view(id, offer, cx)
    }

    /// A press on "My list" or "Watched" of a poster; `listed`: what the poster's list says.
    pub fn press_poster(
        &mut self,
        item: ItemId,
        action: Action,
        listed: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(wanted) = self.marks.press(&item, action, listed) else {
            return;
        };
        cx.notify();
        let asked = item.clone();
        self.ask(
            cx,
            move |server| poster_actions::ask(server, &asked, action, wanted),
            move |shell, answer, cx| shell.poster_answered(&item, action, wanted, answer, cx),
        );
    }

    fn poster_answered(
        &mut self,
        item: &ItemId,
        action: Action,
        wanted: bool,
        answer: Result<(), ProviderError>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = &answer {
            // An expired session brings the login back.
            self.failed(error.clone(), cx);
        }
        if let Some(said) = self.marks.answered(item, action, wanted, &answer) {
            self.show_notice(said, cx);
            return;
        }
        // The pages of the same title show the change too.
        match action {
            Action::MyList => {
                for page in &mut self.details {
                    if &page.id == item {
                        page.button().0.follow(wanted);
                    }
                }
                for page in &mut self.series_pages {
                    if &page.id == item {
                        page.button().0.follow(wanted);
                    }
                }
                if self.navigator.current() == Screen::Favorites {
                    self.load_my_list(cx);
                }
            }
            Action::Watched => {
                for page in &mut self.details {
                    if &page.id == item
                        && let Loadable::Ready(detail) = &mut page.detail
                    {
                        detail.film.watched = wanted;
                        if wanted {
                            detail.film.resume = None;
                        }
                    }
                }
            }
        }
    }

    /// Says `said` over the page for a moment, then fades it out.
    pub fn show_notice(&mut self, said: &'static str, cx: &mut Context<Self>) {
        self.notice = Some(said);
        let timer = cx.background_executor().timer(NOTICE_FOR);
        self.notice_timer = Some(cx.spawn(async move |this, cx| {
            timer.await;
            this.update(cx, |shell, cx| {
                shell.notice = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// The message of the window, over a page, as much of it as shows now.
    pub(super) fn notice_pill(
        &mut self,
        screen: Screen,
        window: &mut Window,
        cx: &App,
    ) -> Option<AnyElement> {
        // The executor's clock, which the tests drive with `advance_clock`.
        let now = cx.background_executor().now();
        let wanted = self
            .notice
            .filter(|_| !matches!(screen, Screen::Login | Screen::Player));
        let drawn = self.notice_fade.follow(wanted, now);
        if self.notice_fade.moving(now) {
            window.request_animation_frame();
        }
        let (said, level) = drawn?;
        Some(screens::notice_pill(said).opacity(level).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::Instant;

    use bobine::provider::{Connector, MediaServer};
    use gpui_kit::SharedString;
    use gpui_kit::{
        Bounds, Modifiers, Pixels, TestAppContext, VisualTestContext, WindowHandle, point, px, size,
    };

    use super::*;
    use crate::fake::FakeServer;
    use crate::home_page::{self, PlayTarget};
    use crate::navigation::NavEntry;
    use crate::screens::{Open, Play, Quick};
    use crate::series_page;

    /// The Home of a server whose playbacks are refused once asked (no player starts), in a
    /// window tall enough for its rows of posters to be drawn.
    fn home(
        cx: &mut TestAppContext,
    ) -> (
        FakeServer,
        WindowHandle<Shell>,
        &'static mut VisualTestContext,
    ) {
        home_with(cx, |_| {})
    }

    /// The Home of `home`, of a server first changed by `change`.
    fn home_with(
        cx: &mut TestAppContext,
        change: impl FnOnce(&mut crate::fake::State),
    ) -> (
        FakeServer,
        WindowHandle<Shell>,
        &'static mut VisualTestContext,
    ) {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().play_error =
            Some(ProviderError::Unreachable("refused".into()));
        change(&mut server.state.lock().unwrap());
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        let cx = VisualTestContext::from_window(*window, cx).into_mut();
        cx.simulate_resize(size(px(1600.), px(3000.)));
        cx.run_until_parked();
        (server, window, cx)
    }

    /// The first poster of a Home row whose play button starts what `wanted` accepts.
    fn poster(
        window: &WindowHandle<Shell>,
        cx: &mut VisualTestContext,
        wanted: impl Fn(&PlayTarget) -> bool,
    ) -> (ElementId, ItemId) {
        let shell = window.root(cx).unwrap();
        cx.read(|cx| {
            let shell = shell.read(cx);
            let mut found: Vec<(ElementId, ItemId)> = shell
                .cards
                .ids()
                .into_iter()
                .filter(|id| id.to_string().contains("home-poster"))
                .filter_map(|id| {
                    let offer = shell.cards.offer(&id)?;
                    match (offer.play, offer.quick) {
                        (Play::Target(target), Some(quick)) if wanted(&target) => {
                            Some((id, quick.item))
                        }
                        _ => None,
                    }
                })
                .collect();
            found.sort_by_key(|(id, _)| id.to_string());
            found.into_iter().next().expect("a poster on the Home")
        })
    }

    /// A Home whose Recently added lists the first series by its third season, new, as a
    /// server does when a series gains a season: the card, and the series it stands for.
    fn home_with_a_new_season(
        cx: &mut TestAppContext,
    ) -> (
        FakeServer,
        WindowHandle<Shell>,
        &'static mut VisualTestContext,
        ElementId,
        ItemId,
    ) {
        let series = crate::fake::series().remove(0);
        let season = FakeServer::new(0, true).seasons(&series.id).unwrap()[2].clone();
        let (server, window, cx) = home_with(cx, |state| {
            state.series[0].season = Some(season);
            // A season says nothing of its series' state.
            state.series[0].favourite = None;
            state.series[0].watched = None;
        });
        let shell = window.root(cx).unwrap();
        let id = cx.read(|cx| {
            let shell = shell.read(cx);
            shell
                .cards
                .ids()
                .into_iter()
                .filter(|id| id.to_string().contains("home-poster"))
                .find(|id| {
                    shell
                        .cards
                        .offer(id)
                        .is_some_and(|offer| matches!(offer.open, Open::Series(_, Some(_))))
                })
                .expect("the card of the new season")
        });
        (server, window, cx, id, series.id)
    }

    fn film_with_resume(server: &FakeServer) -> impl Fn(&PlayTarget) -> bool {
        let started: Vec<ItemId> = server
            .state
            .lock()
            .unwrap()
            .films
            .iter()
            .filter(|f| f.resume.is_some())
            .map(|f| f.id.clone())
            .collect();
        move |target| matches!(target, PlayTarget::Film(id) if started.contains(id))
    }

    fn film_out_of_the_list(server: &FakeServer) -> impl Fn(&PlayTarget) -> bool {
        let listed = server.state.lock().unwrap().favourites.clone();
        move |target| matches!(target, PlayTarget::Film(id) if !listed.contains(id))
    }

    fn a_series(target: &PlayTarget) -> bool {
        matches!(target, PlayTarget::Series(_))
    }

    /// Where the element named `what` of the card `id` was drawn.
    fn bounds(cx: &mut VisualTestContext, what: &str, id: &ElementId) -> Bounds<Pixels> {
        let selector: &'static str = Box::leak(format!("{what} {id}").into_boxed_str());
        cx.debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"))
    }

    /// Where the element named `what` of the card `id` was drawn, if it was.
    fn bounds_of(cx: &mut VisualTestContext, what: &str, id: &ElementId) -> Option<Bounds<Pixels>> {
        let selector: &'static str = Box::leak(format!("{what} {id}").into_boxed_str());
        cx.debug_bounds(selector)
    }

    /// The pointer over the card `id`, which brings its buttons.
    fn hover(cx: &mut VisualTestContext, id: &ElementId) {
        let card = bounds(cx, "card-hover", id);
        cx.simulate_mouse_move(card.center(), None, Modifiers::none());
        cx.run_until_parked();
    }

    fn click(cx: &mut VisualTestContext, what: &str, id: &ElementId) {
        hover(cx, id);
        let at = bounds(cx, what, id).center();
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
    }

    /// How much the layer of the card `id` shows once its fade is over.
    fn veil(window: &WindowHandle<Shell>, cx: &mut VisualTestContext, id: &ElementId) -> f32 {
        let shell = window.root(cx).unwrap();
        cx.read(|cx| {
            let view = shell.read(cx).cards.find(id).unwrap();
            view.read(cx)
                .hovered(Instant::now() + Duration::from_secs(5))
        })
    }

    fn quick(window: &WindowHandle<Shell>, cx: &mut VisualTestContext, id: &ElementId) -> Quick {
        let shell = window.root(cx).unwrap();
        cx.read(|cx| shell.read(cx).cards.offer(id).unwrap().quick.unwrap())
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut VisualTestContext) -> Screen {
        window
            .read_with(cx, |shell, _| shell.navigator.current())
            .unwrap()
    }

    /// How many times the shell was drawn, and each card.
    fn draws(
        window: &WindowHandle<Shell>,
        cx: &mut VisualTestContext,
    ) -> (usize, std::collections::HashMap<ElementId, usize>) {
        window
            .read_with(cx, |shell, _| {
                let draws = shell.draws.borrow();
                let mut cards = std::collections::HashMap::new();
                for (id, _) in &draws.cards {
                    *cards.entry(id.clone()).or_default() += 1;
                }
                (draws.shell.len(), cards)
            })
            .unwrap()
    }

    #[gpui_kit::test]
    fn the_pointer_over_a_poster_shows_the_veil_and_the_buttons_and_leaving_hides_them(
        cx: &mut TestAppContext,
    ) {
        let (_, window, cx) = home(cx);
        let (id, _) = poster(&window, cx, |_| true);
        assert_eq!(veil(&window, cx, &id), 0.);
        let card = bounds(cx, "card-hover", &id);

        hover(cx, &id);
        assert_eq!(veil(&window, cx, &id), 1.);
        // The play button in the middle, the three buttons in the top right corner.
        let play = bounds(cx, "card-play", &id);
        assert!(card.contains(&play.center()));
        for button in ["card-my-list", "card-watched", "card-playlist"] {
            let button = bounds(cx, button, &id);
            assert!(
                button.top() < card.top() + px(40.) && button.right() > card.right() - px(120.)
            );
        }

        cx.simulate_mouse_move(point(px(5.), px(5.)), None, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(veil(&window, cx, &id), 0.);
    }

    #[gpui_kit::test]
    fn only_the_card_under_the_pointer_is_drawn_again(cx: &mut TestAppContext) {
        let (_, window, cx) = home(cx);
        let (id, _) = poster(&window, cx, |_| true);
        let card = bounds(cx, "card-hover", &id);
        // Between two posters of the row.
        let aside = point(card.right() + px(8.), card.center().y);
        cx.simulate_mouse_move(aside, None, Modifiers::none());
        cx.run_until_parked();
        let notified = Rc::new(std::cell::Cell::new(0));
        let shell = window.root(cx).unwrap();
        cx.update(|_, cx| {
            let notified = Rc::clone(&notified);
            cx.observe(&shell, move |_, _| notified.set(notified.get() + 1))
                .detach();
        });
        let (shell_before, cards_before) = draws(&window, cx);

        // In, over the play button and the corner's buttons, and out.
        cx.simulate_mouse_move(card.center(), None, Modifiers::none());
        cx.run_until_parked();
        for at in [
            bounds(cx, "card-my-list", &id).center(),
            bounds(cx, "card-watched", &id).center(),
            aside,
        ] {
            cx.simulate_mouse_move(at, None, Modifiers::none());
            cx.run_until_parked();
        }

        let (shell_after, cards_after) = draws(&window, cx);
        let drawn = |cards: &std::collections::HashMap<ElementId, usize>, card: &ElementId| {
            cards.get(card).copied().unwrap_or(0)
        };
        assert!(
            drawn(&cards_after, &id) - drawn(&cards_before, &id) >= 4,
            "the card must have been drawn"
        );
        let others: Vec<&ElementId> = cards_after
            .keys()
            .filter(|card| *card != &id && drawn(&cards_after, card) != drawn(&cards_before, card))
            .collect();
        assert!(
            others.is_empty(),
            "other cards were drawn again: {others:?}"
        );
        assert_eq!(notified.get(), 0, "the shell was notified");
        assert_eq!(shell_after - shell_before, 0, "the shell was built again");
    }

    #[gpui_kit::test]
    fn the_play_button_of_a_film_plays_it_from_its_resume_point(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, film_with_resume(&server));
        click(cx, "card-play", &id);
        let plays = server.state.lock().unwrap().record.plays.clone();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].0, film);
        // What the button started plays from where the viewer stopped.
        let resume = server
            .state
            .lock()
            .unwrap()
            .films
            .iter()
            .find(|f| f.id == film)
            .and_then(|f| f.resume)
            .unwrap();
        let started = home_page::start(&server, &PlayTarget::Film(film)).unwrap();
        assert_eq!(started.plan.start, resume);
        assert_eq!(screen(&window, cx), Screen::Home, "the page was opened");
    }

    #[gpui_kit::test]
    fn the_play_button_of_a_series_plays_its_next_episode(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, series) = poster(&window, cx, a_series);
        let next = series_page::next_to_play(&server, &series)
            .unwrap()
            .expect("a next episode");
        click(cx, "card-play", &id);
        let plays = server.state.lock().unwrap().record.plays.clone();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].0, next.item.id);
        assert_eq!(screen(&window, cx), Screen::Home, "the page was opened");
    }

    #[gpui_kit::test]
    fn the_corner_of_a_poster_opens_add_to_playlist_under_its_button(cx: &mut TestAppContext) {
        let (_, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, |target| matches!(target, PlayTarget::Film(_)));
        click(cx, "card-playlist", &id);
        let button = bounds(cx, "card-playlist", &id);
        let (item, anchor) = window
            .read_with(cx, |shell, _| {
                let menu = shell.playlist_menu.as_ref().expect("the menu opened");
                (menu.picker.item.clone(), menu.anchor)
            })
            .unwrap();
        assert_eq!(item, film);
        assert_eq!(anchor, Some(button));
        let menu = cx.debug_bounds("playlist-menu").expect("the menu is drawn");
        assert!(menu.top() >= button.bottom(), "under its button");
        assert_eq!(screen(&window, cx), Screen::Home, "the page was opened");
    }

    #[gpui_kit::test]
    fn my_list_adds_and_removes(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, film_out_of_the_list(&server));
        assert!(!quick(&window, cx, &id).my_list.now);

        click(cx, "card-my-list", &id);
        assert!(quick(&window, cx, &id).my_list.now);
        assert!(server.state.lock().unwrap().favourites.contains(&film));

        click(cx, "card-my-list", &id);
        assert!(!quick(&window, cx, &id).my_list.now);
        assert!(!server.state.lock().unwrap().favourites.contains(&film));
        assert_eq!(
            server.state.lock().unwrap().record.favourite_calls,
            [(film.clone(), true), (film, false)]
        );
        assert_eq!(screen(&window, cx), Screen::Home, "the page was opened");
    }

    #[gpui_kit::test]
    fn a_refused_my_list_change_is_put_back_and_told(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, film_out_of_the_list(&server));
        server.state.lock().unwrap().favourite_fails = true;

        click(cx, "card-my-list", &id);

        assert!(!quick(&window, cx, &id).my_list.now);
        assert!(!server.state.lock().unwrap().favourites.contains(&film));
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert_eq!(said, Some("Favorite could not be changed."));
        // It goes by itself.
        cx.executor()
            .advance_clock(NOTICE_FOR + Duration::from_millis(10));
        cx.run_until_parked();
        assert_eq!(window.read_with(cx, |shell, _| shell.notice).unwrap(), None);
    }

    #[gpui_kit::test]
    fn watched_marks_and_unmarks_and_the_mark_on_the_card_follows(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, film_with_resume(&server));
        assert!(!quick(&window, cx, &id).watched.now);
        assert!(cx.debug_bounds(watched_mark(&id)).is_none());

        click(cx, "card-watched", &id);
        assert!(quick(&window, cx, &id).watched.now);
        let watched = |server: &FakeServer| {
            let state = server.state.lock().unwrap();
            state.films.iter().find(|f| f.id == film).unwrap().watched
        };
        assert!(watched(&server));
        // The check of a watched film, top right, drawn without the row being asked again.
        assert!(cx.debug_bounds(watched_mark(&id)).is_some());

        click(cx, "card-watched", &id);
        assert!(!quick(&window, cx, &id).watched.now);
        assert!(!watched(&server));
        assert!(cx.debug_bounds(watched_mark(&id)).is_none());
        assert_eq!(screen(&window, cx), Screen::Home, "the page was opened");
    }

    fn watched_mark(id: &ElementId) -> &'static str {
        Box::leak(format!("watched-mark {id}").into_boxed_str())
    }

    #[gpui_kit::test]
    fn watched_on_a_series_marks_the_whole_series(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, series) = poster(&window, cx, a_series);
        click(cx, "card-watched", &id);
        assert!(quick(&window, cx, &id).watched.now);
        assert_eq!(
            server.state.lock().unwrap().record.watched,
            [(series.clone(), true)]
        );
        let state = server.state.lock().unwrap();
        assert!(
            state
                .episodes
                .iter()
                .filter(|e| e.series == series)
                .all(|e| e.item.watched)
        );
    }

    #[gpui_kit::test]
    fn a_poster_of_a_library_plays_and_a_refusal_is_told_there(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Films, cx))
            .unwrap();
        cx.run_until_parked();
        let id = ElementId::from(("film", 0_usize));
        let film = window
            .read_with(cx, |shell, _| {
                let offer = shell.cards.offer(&id).expect("the first film of the grid");
                offer.quick.unwrap().item
            })
            .unwrap();

        click(cx, "card-play", &id);

        let plays = server.state.lock().unwrap().record.plays.clone();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].0, film);
        assert_eq!(screen(&window, cx), Screen::Library);
        let said = window.read_with(cx, |shell, _| shell.notice).unwrap();
        assert!(said.is_some(), "the refusal was not told");
    }

    #[gpui_kit::test]
    fn a_click_on_a_button_never_opens_the_page_and_a_click_elsewhere_does(
        cx: &mut TestAppContext,
    ) {
        let (_, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, |target| matches!(target, PlayTarget::Film(_)));
        for button in ["card-my-list", "card-watched"] {
            click(cx, button, &id);
            assert_eq!(
                screen(&window, cx),
                Screen::Home,
                "{button} opened the page"
            );
        }
        // On the veil, away from the buttons.
        let card = bounds(cx, "card-hover", &id);
        let at = point(card.left() + px(12.), card.bottom() - px(12.));
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        let opened = window
            .read_with(cx, |shell, _| {
                shell.details.last().map(|page| page.id.clone())
            })
            .unwrap();
        assert_eq!(opened, Some(film));
    }

    #[gpui_kit::test]
    fn a_click_on_a_card_of_a_new_season_opens_its_series_on_that_season(cx: &mut TestAppContext) {
        let (_, window, cx, id, series) = home_with_a_new_season(cx);
        let card = bounds(cx, "card-hover", &id);
        let at = point(card.left() + px(12.), card.bottom() - px(12.));
        cx.simulate_mouse_move(at, None, Modifiers::none());
        cx.simulate_click(at, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::SeriesDetail);
        let (opened, season) = window
            .read_with(cx, |shell, _| {
                let page = shell.series_pages.last().expect("the series page");
                (page.id.clone(), page.season)
            })
            .unwrap();
        assert_eq!(opened, series);
        // Unasked, the page lands on the second season, where the series is begun.
        assert_eq!(season, 2, "the page opens on the new season");
    }

    /// The card of a new season knows the season, not whether its series is in the list or
    /// watched: it has the veil and the play button, and no button in its corner.
    #[gpui_kit::test]
    fn a_card_of_a_new_season_plays_its_series_and_has_no_corner_buttons(cx: &mut TestAppContext) {
        let (server, window, cx, id, series) = home_with_a_new_season(cx);
        let card = bounds(cx, "card-hover", &id);
        cx.simulate_mouse_move(card.center(), None, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(veil(&window, cx, &id), 1.);
        for button in ["card-my-list", "card-watched"] {
            let selector: &'static str = Box::leak(format!("{button} {id}").into_boxed_str());
            assert!(cx.debug_bounds(selector).is_none(), "{button} was drawn");
        }
        let next = series_page::next_to_play(&server, &series)
            .unwrap()
            .expect("a next episode");
        click(cx, "card-play", &id);
        let plays = server.state.lock().unwrap().record.plays.clone();
        assert_eq!(plays.len(), 1);
        assert_eq!(plays[0].0, next.item.id, "the series' next up plays");
        assert_eq!(screen(&window, cx), Screen::Home, "the page was opened");
    }

    /// A series listed through a lone new episode: whether it is in the list or watched is not
    /// known, so its card has the veil and the play button and no button in its corner.
    #[gpui_kit::test]
    fn a_card_of_a_lone_new_episode_has_no_corner_buttons(cx: &mut TestAppContext) {
        let (_, window, cx) = home_with(cx, |state| {
            state.series[0].favourite = None;
            state.series[0].watched = None;
        });
        let shell = window.root(cx).unwrap();
        let id = cx.read(|cx| {
            let shell = shell.read(cx);
            shell
                .cards
                .ids()
                .into_iter()
                .filter(|id| id.to_string().contains("home-poster"))
                .find(|id| {
                    shell.cards.offer(id).is_some_and(|offer| {
                        matches!(offer.play, Play::Target(PlayTarget::Series(_)))
                            && offer.quick.is_none()
                    })
                })
                .expect("the card of the series of a lone episode")
        });
        let card = bounds(cx, "card-hover", &id);
        cx.simulate_mouse_move(card.center(), None, Modifiers::none());
        cx.run_until_parked();
        assert_eq!(veil(&window, cx, &id), 1.);
        assert!(bounds_of(cx, "card-play", &id).is_some());
        for button in ["card-my-list", "card-watched"] {
            assert!(bounds_of(cx, button, &id).is_none(), "{button} was drawn");
        }
    }

    #[gpui_kit::test]
    fn a_mark_then_a_refresh_with_another_server_truth_shows_the_servers_value(
        cx: &mut TestAppContext,
    ) {
        let (server, window, cx) = home(cx);
        let (id, film) = poster(&window, cx, film_with_resume(&server));
        click(cx, "card-watched", &id);
        assert!(quick(&window, cx, &id).watched.now);
        // Unmarked since, from another device.
        {
            let mut state = server.state.lock().unwrap();
            state
                .films
                .iter_mut()
                .find(|f| f.id == film)
                .unwrap()
                .watched = false;
        }
        window
            .update(cx, |shell, _, cx| shell.load_home(cx))
            .unwrap();
        cx.run_until_parked();
        assert!(!quick(&window, cx, &id).watched.now);
    }

    #[gpui_kit::test]
    fn a_series_marked_watched_then_an_episode_unmarked_on_its_page_is_unwatched_on_its_poster(
        cx: &mut TestAppContext,
    ) {
        let (server, window, cx) = home(cx);
        let (id, series) = poster(&window, cx, a_series);
        click(cx, "card-watched", &id);
        assert!(quick(&window, cx, &id).watched.now);
        window
            .update(cx, |shell, _, cx| shell.open_series(series.clone(), cx))
            .unwrap();
        cx.run_until_parked();
        let episode = server
            .state
            .lock()
            .unwrap()
            .episodes
            .iter()
            .find(|e| e.series == series)
            .cloned()
            .unwrap();
        window
            .update(cx, |shell, _, cx| shell.toggle_episode_watched(episode, cx))
            .unwrap();
        cx.run_until_parked();
        window.update(cx, |shell, _, cx| shell.go_back(cx)).unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Home);
        assert!(
            !quick(&window, cx, &id).watched.now,
            "the poster still says watched"
        );
    }

    /// The buttons show with the pointer: before it came, a click where they will be opens the
    /// page, as a click anywhere else on the card does.
    #[gpui_kit::test]
    fn a_click_on_a_cards_centre_or_corner_without_hover_opens_the_page(cx: &mut TestAppContext) {
        for corner in [false, true] {
            let (server, window, cx) = home(cx);
            let (id, film) = poster(&window, cx, |target| matches!(target, PlayTarget::Film(_)));
            let card = bounds(cx, "card-hover", &id);
            let at = if corner {
                point(card.right() - px(20.), card.top() + px(20.))
            } else {
                card.center()
            };
            // The pointer never moved over it: a page that came under it.
            cx.simulate_click(at, Modifiers::none());
            cx.run_until_parked();
            assert_eq!(screen(&window, cx), Screen::FilmDetail, "corner: {corner}");
            let opened = window
                .read_with(cx, |shell, _| {
                    shell.details.last().map(|page| page.id.clone())
                })
                .unwrap();
            assert_eq!(opened, Some(film));
            let record = &server.state.lock().unwrap().record;
            assert!(record.plays.is_empty() && record.favourite_calls.is_empty());
            assert!(record.watched.is_empty());
        }
    }

    fn pill_drawn(cx: &mut VisualTestContext, said: &str) -> bool {
        let selector: &'static str = Box::leak(format!("pill {said}").into_boxed_str());
        cx.debug_bounds(selector).is_some()
    }

    fn notice(window: &WindowHandle<Shell>, cx: &mut VisualTestContext) -> Option<&'static str> {
        window.read_with(cx, |shell, _| shell.notice).unwrap()
    }

    #[gpui_kit::test]
    fn a_play_refused_from_a_library_poster_leaves_no_error_on_the_home(cx: &mut TestAppContext) {
        let (_, window, cx) = home(cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Films, cx))
            .unwrap();
        cx.run_until_parked();
        click(cx, "card-play", &ElementId::from(("film", 0_usize)));
        let said = notice(&window, cx).expect("the refusal was told");
        assert!(
            pill_drawn(cx, said),
            "the refusal was not shown on the library"
        );
        cx.executor()
            .advance_clock(NOTICE_FOR + Duration::from_secs(1));
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Home, cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Home);
        assert!(!pill_drawn(cx, said), "the Home shows an old refusal");
    }

    #[gpui_kit::test]
    fn on_the_home_a_refused_play_then_a_refused_change_show_one_pill(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, _) = poster(&window, cx, film_out_of_the_list(&server));
        click(cx, "card-play", &id);
        let refused_play = notice(&window, cx).expect("the refused play was told");
        assert!(pill_drawn(cx, refused_play));
        server.state.lock().unwrap().favourite_fails = true;
        click(cx, "card-my-list", &id);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        assert!(pill_drawn(cx, crate::favourite::FAILED));
        assert!(!pill_drawn(cx, refused_play), "two pills over each other");
    }

    #[gpui_kit::test]
    fn a_play_refused_from_a_poster_of_a_film_page_is_told_there(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let film = server.state.lock().unwrap().films[0].id.clone();
        window
            .update(cx, |shell, _, cx| shell.open_film(film, cx))
            .unwrap();
        cx.run_until_parked();
        let id = ElementId::from((SharedString::from("Similar titles-film"), 0_usize));
        click(cx, "card-play", &id);
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        let said = notice(&window, cx).expect("the refusal was told");
        assert!(
            pill_drawn(cx, said),
            "the refusal was not shown on the page"
        );
    }

    /// A film changed from a poster, then changed again elsewhere: the collection row of a film
    /// page, asked after, says what the server holds now, not what the old mark says.
    #[gpui_kit::test]
    fn the_collection_row_of_a_film_page_shows_what_the_server_says_over_an_old_mark(
        cx: &mut TestAppContext,
    ) {
        // Enough films for the saga to hold another than the one of the page.
        let (server, window, cx) = home_with(cx, |state| state.films = crate::fake::films(10));
        let saga = server.collection(&ItemId("saga".into())).unwrap();
        let watched = saga
            .films
            .iter()
            .find(|film| film.watched)
            .unwrap()
            .id
            .clone();
        window
            .update(cx, |shell, _, _| {
                shell.marks.follow(&watched, Action::Watched, false)
            })
            .unwrap();
        let film = server.state.lock().unwrap().films[0].id.clone();
        window
            .update(cx, |shell, _, cx| shell.open_film(film, cx))
            .unwrap();
        cx.run_until_parked();
        let shown = window
            .read_with(cx, |shell, _| {
                shell.marks.shown(&watched, Action::Watched, true)
            })
            .unwrap();
        assert!(shown, "the old mark still hides what the server says");
    }

    #[gpui_kit::test]
    fn a_series_of_a_person_page_has_my_list_and_watched(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let person = server.state.lock().unwrap().people[0].clone();
        window
            .update(cx, |shell, _, cx| shell.open_person(person, cx))
            .unwrap();
        cx.run_until_parked();
        let id = ElementId::from(("person-series", 0_usize));
        let series = quick(&window, cx, &id).item;
        assert!(!quick(&window, cx, &id).watched.now);
        click(cx, "card-watched", &id);
        assert!(quick(&window, cx, &id).watched.now);
        assert_eq!(
            server.state.lock().unwrap().record.watched,
            [(series, true)]
        );
        assert_eq!(screen(&window, cx), Screen::Person, "the page was opened");
    }

    /// A card whose place now holds another title is a new card: the fades of the old one's
    /// veil and buttons are not carried over to it.
    #[gpui_kit::test]
    fn a_card_whose_title_changes_starts_afresh(cx: &mut TestAppContext) {
        let (server, window, cx) = home(cx);
        let (id, first) = poster(&window, cx, |target| matches!(target, PlayTarget::Film(_)));
        hover(cx, &id);
        assert_eq!(veil(&window, cx, &id), 1.);
        let shell = window.root(cx).unwrap();
        let before = cx.read(|cx| shell.read(cx).cards.find(&id).unwrap().entity_id());

        // The library gained a film since: the row moves along, another film in this place.
        {
            let mut state = server.state.lock().unwrap();
            let at = state.films.iter().position(|f| f.id == first).unwrap();
            let gained = state.films.pop().unwrap();
            state.films.insert(at, gained);
        }
        window
            .update(cx, |shell, _, cx| shell.load_home(cx))
            .unwrap();
        cx.run_until_parked();
        assert_ne!(quick(&window, cx, &id).item, first);
        let after = cx.read(|cx| shell.read(cx).cards.find(&id).unwrap().entity_id());
        assert_ne!(before, after, "the old card was kept");
    }
}
