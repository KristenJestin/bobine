//! The way into the player and back out of it, on the window: see `crate::curtain`.

use std::time::{Duration, Instant};

use bobine::provider::ItemId;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, ElementId, ObjectFit, Pixels, Window, div, img, point, px,
};

use super::Shell;
use crate::curtain::{Curtain, PAGE, Picture, Way, back, between, in_sight};
use crate::navigation::Screen;
use crate::{screens, theme};

/// While the way in waits for the video's first frame, the player is asked this often: the cover
/// goes the frame after the video's first.
pub(super) const FIRST_FRAME_TICK: Duration = Duration::from_millis(16);

/// What the way in or out draws over the window this frame.
pub(super) struct Cover {
    pub element: AnyElement,
    /// How much of the player's controls show.
    pub player: f32,
    /// How much of the page the player opens over still shows.
    pub page: f32,
}

impl Shell {
    /// The player opens over the page `from`: the way in starts, unless this playback follows
    /// the one that just ended.
    pub(super) fn open_curtain(&mut self, from: Screen, seamless: bool) {
        self.curtain = (!seamless).then(|| Curtain::opening(None, Instant::now()));
        if !seamless {
            self.note_curtain("opening");
        }
        self.curtain_from = from;
        self.curtain_back = self.navigator.back_label();
        self.opened_from = None;
    }

    /// Play was pressed on the picture drawn at `at`: the playback it starts grows from it.
    pub fn clicked_on(&mut self, at: impl Into<ElementId>) {
        self.clicked = self.pictures.borrow().get(&at.into()).cloned();
    }

    /// The way in grows from the picture whose Play was pressed, or else from the picture of
    /// `item` the page drew, when it is in sight; the way out goes back to it.
    pub(super) fn grow_from(&mut self, item: Option<&ItemId>, window: &Window) {
        let drawn = self.pictures.borrow();
        let still_drawn = |picture: &Picture| {
            (drawn.get(&picture.at)).is_some_and(|drawn| drawn.item == picture.item)
        };
        // A press whose playback never started is left behind with the page it was on.
        let clicked = self.clicked.take().filter(still_drawn);
        let page = || {
            (drawn.get(&PAGE.into()))
                .filter(|picture| Some(&picture.item) == item)
                .cloned()
        };
        let picture = (clicked.or_else(page))
            .filter(|picture| in_sight(picture.bounds, window.viewport_size()));
        drop(drawn);
        if let Some(curtain) = &mut self.curtain
            && curtain.way() == Way::Opening
        {
            curtain.picture = picture.clone();
        }
        self.opened_from = picture;
    }

    /// A playback asked did not start: the picture pressed grows into no other.
    pub(super) fn forget_click(&mut self) {
        self.clicked = None;
    }

    /// Where the page draws `picture` now, if it shows it in sight: the way out shrinks back
    /// there, and fades out otherwise.
    pub(super) fn back_to(&self, picture: &Picture, window: &Window) -> Option<Bounds<Pixels>> {
        let drawn = self.pictures.borrow();
        (drawn.get(&picture.at))
            .filter(|drawn| drawn.item == picture.item)
            .map(|drawn| drawn.bounds)
            .filter(|bounds| in_sight(*bounds, window.viewport_size()))
    }

    /// While the player opens over it, the page it opens over, and the label of that page's
    /// back link.
    pub(super) fn leaving_page(&self, screen: Screen) -> Option<(Screen, Option<&'static str>)> {
        let curtain = self.curtain.as_ref()?;
        let opening = curtain.way() == Way::Opening && screen == Screen::Player;
        (opening && !curtain.over(Instant::now())).then_some((self.curtain_from, self.curtain_back))
    }

    /// The player is left: the way out, unless the card of the next episode is offered there.
    pub(super) fn close_curtain(&mut self, next_offered: bool) {
        let picture = self.opened_from.take();
        // With no picture to shrink back, there is nothing of the way out to draw.
        let closes = !next_offered && picture.is_some();
        self.curtain = closes.then(|| Curtain::closing(picture, Instant::now()));
        if closes {
            self.note_curtain("closing");
        }
    }

    /// The harness reads the frame times of each way in and out from these.
    pub(super) fn note_curtain(&self, what: &'static str) {
        #[cfg(feature = "harness")]
        self.curtain_marks.borrow_mut().push((what, Instant::now()));
        #[cfg(not(feature = "harness"))]
        let _ = what;
    }

    /// The player's ticker does not ask now: mpv opening a file answers slowly, and asking it
    /// while the way in moves would cost that move its frames. What the viewer presses is not
    /// held back.
    pub(super) fn holds_the_player_back(&self) -> bool {
        let slow = (self.playback.as_ref()).is_some_and(|p| p.session.answers_slowly());
        slow && (self.curtain.as_ref()).is_some_and(|c| c.animating(Instant::now()))
    }

    /// How long the player's ticker waits before it asks again.
    pub(super) fn tick_every(&self) -> Duration {
        if self.curtain.as_ref().is_some_and(Curtain::waiting) {
            FIRST_FRAME_TICK
        } else {
            super::PLAYER_TICK
        }
    }

    /// What the way in or out draws this frame, if it is still on its way. `playing`: the
    /// player is the screen.
    pub(super) fn curtain_cover(
        &mut self,
        playing: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Cover> {
        let now = Instant::now();
        let shown =
            (self.playback.as_ref()).is_some_and(|playback| playback.session.picture_shown());
        let curtain = self.curtain.as_mut()?;
        if shown && curtain.waiting() {
            curtain.first_frame(now);
            self.note_curtain("first frame");
        }
        let curtain = self.curtain.as_mut()?;
        // Its first frame starts it: what held the window before is not taken from it.
        let frame = curtain.draw(now);
        // A way in to a player gone, or a way out to a player come back, is nothing to draw.
        if curtain.over(now) || (curtain.way() == Way::Opening) != playing {
            self.curtain = None;
            self.note_curtain("over");
            return None;
        }
        if curtain.moving(now) {
            window.request_animation_frame();
        }
        let whole = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
        match curtain.way() {
            Way::Opening => {
                let picture = curtain.picture.as_ref().map(|picture| {
                    let at = between(picture.bounds, whole, frame.grown);
                    drawn_at(picture, at)
                });
                // The page stays where it was, fading out under the player coming in.
                let from = self.curtain_from;
                let page = (frame.page > 0. && from != Screen::Login)
                    .then(|| screens::render(self, from, window, cx));
                let element = div()
                    .id("curtain")
                    .absolute()
                    .inset_0()
                    .bg(theme::background())
                    .opacity(frame.cover)
                    .children(picture)
                    .children(
                        page.map(|page| div().absolute().inset_0().opacity(frame.page).child(page)),
                    )
                    // What is left of the page answers nothing.
                    .child(div().absolute().inset_0().occlude())
                    .into_any_element();
                Some(Cover {
                    element,
                    player: frame.player,
                    page: frame.page,
                })
            }
            Way::Closing => {
                let picture = curtain.picture.clone()?;
                // Back to where the page draws it now: the window may have changed meanwhile.
                let (at, shows) = back(self.back_to(&picture, window), whole, frame.grown);
                let element = div()
                    .absolute()
                    .inset_0()
                    .opacity(shows)
                    .child(drawn_at(&picture, at));
                Some(Cover {
                    element: element.into_any_element(),
                    player: 0.,
                    page: frame.page,
                })
            }
        }
    }
}

/// `picture` filling `at`, cut as the page cuts it.
fn drawn_at(picture: &Picture, at: Bounds<gpui_kit::Pixels>) -> AnyElement {
    img(picture.image.clone())
        .absolute()
        .left(at.origin.x)
        .top(at.origin.y)
        .w(at.size.width)
        .h(at.size.height)
        .object_fit(ObjectFit::Cover)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use bobine::provider::{Episode, ItemId, ProviderError};
    use gpui_kit::{
        Bounds, ElementId, Modifiers, Pixels, TestAppContext, VisualTestContext, WindowHandle, px,
        size,
    };

    use crate::curtain::Way;
    use crate::fake::{FILM, FakeServer, SERIES};
    use crate::film_page::From;
    use crate::home_page::PlayTarget;
    use crate::navigation::{NavEntry, Screen};
    use crate::player::PlayerEvent;
    use crate::player::fake::FakePlayer;
    use crate::player::session::WindowRequest;
    use crate::series_page::Next;
    use crate::shell::{Loadable, Shell};

    fn signed_in(cx: &mut TestAppContext) -> (FakeServer, WindowHandle<Shell>) {
        let server = FakeServer::new(6, true);
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        (server, window)
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .read_with(cx, |shell, _| shell.navigator.current())
            .unwrap()
    }

    fn way(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<Way> {
        window
            .read_with(cx, |shell, _| shell.curtain.as_ref().map(|c| c.way()))
            .unwrap()
    }

    fn waiting(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> bool {
        window
            .read_with(cx, |shell, _| shell.curtain.as_ref().unwrap().waiting())
            .unwrap()
    }

    fn tick(cx: &mut TestAppContext) {
        cx.executor().advance_clock(super::super::PLAYER_TICK * 2);
        cx.run_until_parked();
    }

    /// The series page open on the episodes of its first season.
    fn series_episodes(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<Episode> {
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(SERIES.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
        window
            .read_with(cx, |shell, _| {
                let page = shell.series_pages.last().unwrap();
                match &page.episodes[&page.seasons[page.season].id] {
                    Loadable::Ready(episodes) => episodes.clone(),
                    _ => panic!("the episodes are shown"),
                }
            })
            .unwrap()
    }

    fn open_film_page(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(window, cx), Screen::FilmDetail);
    }

    /// The page is drawn, its backdrop asked, loaded, and drawn again.
    fn draw_page(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
        let visual = gpui_kit::VisualTestContext::from_window((*window).into(), cx);
        for _ in 0..3 {
            visual.run_until_parked();
        }
    }

    fn press_play(window: &WindowHandle<Shell>, player: FakePlayer, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(player);
                shell.play_film(From::Resume, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
    }

    /// The film's page open, then its Play pressed, on `player`: the page was never drawn.
    fn play_from_its_page(
        window: &WindowHandle<Shell>,
        player: FakePlayer,
        cx: &mut TestAppContext,
    ) {
        open_film_page(window, cx);
        press_play(window, player, cx);
    }

    /// The window, large enough for the rows, drawn until the pictures it asked are in.
    fn drawn(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> &'static mut VisualTestContext {
        let visual = VisualTestContext::from_window((*window).into(), cx).into_mut();
        visual.simulate_resize(size(px(1600.), px(1200.)));
        for _ in 0..3 {
            visual.run_until_parked();
        }
        // The page slides in on the real clock: a picture is measured once it is in place.
        std::thread::sleep(crate::motion::page());
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        visual
    }

    fn bounds(visual: &mut VisualTestContext, selector: String) -> Bounds<Pixels> {
        let selector: &'static str = Box::leak(selector.into_boxed_str());
        (visual.debug_bounds(selector)).unwrap_or_else(|| panic!("{selector} was not drawn"))
    }

    /// A click on the Play button `button`, the pointer over it first: where the picture
    /// `picture` was drawn when it was clicked.
    fn play_at(visual: &mut VisualTestContext, button: String, picture: String) -> Bounds<Pixels> {
        let at = bounds(visual, button).center();
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.run_until_parked();
        let picture = bounds(visual, picture);
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();
        picture
    }

    /// The next playback starts on a fake player that opens and shows its first frame.
    fn arm(window: &WindowHandle<Shell>, cx: &mut VisualTestContext) {
        let player = FakePlayer::default();
        (player.queued.borrow_mut()).extend([PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        (window.update(cx, |shell, _, _| shell.fake_player = Some(player))).unwrap();
    }

    /// Where the picture the way in grows from starts, if it grows one.
    fn grown_from(
        window: &WindowHandle<Shell>,
        cx: &mut VisualTestContext,
    ) -> Option<Bounds<Pixels>> {
        window
            .read_with(cx, |shell, _| {
                let curtain = shell.curtain.as_ref()?;
                assert_eq!(curtain.way(), Way::Opening);
                Some(curtain.picture.as_ref()?.bounds)
            })
            .unwrap()
    }

    /// The card drawn highest in the window among those `named`.
    fn a_card(window: &WindowHandle<Shell>, cx: &mut VisualTestContext, named: &str) -> ElementId {
        let ids = window
            .read_with(cx, |shell, _| {
                (shell.cards.ids().into_iter())
                    .filter(|id| id.to_string().contains(named))
                    .collect::<Vec<_>>()
            })
            .unwrap();
        (ids.into_iter())
            .filter_map(|id| {
                let selector = format!("card-hover {id}");
                let drawn = cx.debug_bounds(Box::leak(selector.into_boxed_str()))?;
                let window = size(px(1600.), px(1200.));
                crate::curtain::in_sight(drawn, window).then(|| (f32::from(drawn.top()), id))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id)| id)
            .unwrap_or_else(|| panic!("no card {named}"))
    }

    /// Play on the card `id`: where its picture was.
    fn play_on_card(
        window: &WindowHandle<Shell>,
        cx: &mut VisualTestContext,
        id: &ElementId,
    ) -> Bounds<Pixels> {
        arm(window, cx);
        // The pointer over the card brings its Play button.
        let card = bounds(cx, format!("card-hover {id}"));
        cx.simulate_mouse_move(card.center(), None, Modifiers::none());
        cx.run_until_parked();
        let picture = play_at(cx, format!("card-play {id}"), format!("card-hover {id}"));
        assert_eq!(screen(window, cx), Screen::Player);
        picture
    }

    /// Back from the player, onto the page drawn again: where the way out takes the picture
    /// back, if the page shows it.
    fn back_onto(
        window: &WindowHandle<Shell>,
        cx: &mut VisualTestContext,
    ) -> Option<Bounds<Pixels>> {
        tick(cx);
        let picture = window
            .update(cx, |shell, window, cx| {
                shell.player_back(window, cx);
                assert_eq!(shell.curtain.as_ref().map(|c| c.way()), Some(Way::Closing));
                shell.curtain.as_ref().unwrap().picture.clone()
            })
            .unwrap()
            .expect("a picture to take back");
        for _ in 0..3 {
            cx.run_until_parked();
        }
        (window.update(cx, |shell, window, _| shell.back_to(&picture, window))).unwrap()
    }

    #[gpui_kit::test]
    fn play_in_the_home_header_grows_its_backdrop_into_the_player(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let visual = drawn(&window, cx);
        arm(&window, visual);
        let header = play_at(visual, "hero-play".into(), "hero".into());
        assert_eq!(screen(&window, visual), Screen::Player);
        assert_eq!(grown_from(&window, visual), Some(header));
    }

    #[gpui_kit::test]
    fn play_on_a_card_of_a_home_row_grows_its_picture_into_the_player(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let visual = drawn(&window, cx);
        let card = a_card(&window, visual, "home-");
        let picture = play_on_card(&window, visual, &card);
        assert_eq!(grown_from(&window, visual), Some(picture));
    }

    #[gpui_kit::test]
    fn play_on_a_poster_of_a_library_grows_it_into_the_player(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        (window.update(cx, |shell, _, cx| shell.select(NavEntry::Films, cx))).unwrap();
        let visual = drawn(&window, cx);
        assert_eq!(screen(&window, visual), Screen::Library);
        let card = a_card(&window, visual, "film");
        let picture = play_on_card(&window, visual, &card);
        assert_eq!(grown_from(&window, visual), Some(picture));
    }

    #[gpui_kit::test]
    fn play_on_a_playlist_entry_grows_its_picture_into_the_player(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.select(NavEntry::MyList, cx);
                shell.open_playlist(ItemId("playlist-watchlist".into()), cx);
            })
            .unwrap();
        let visual = drawn(&window, cx);
        arm(&window, visual);
        let play = "playlist-entry-play 0".into();
        let still = play_at(visual, play, "playlist-still 0".into());
        assert_eq!(screen(&window, visual), Screen::Player);
        assert_eq!(grown_from(&window, visual), Some(still));
    }

    #[gpui_kit::test]
    fn leaving_the_player_shrinks_the_picture_back_to_its_card_still_in_sight(
        cx: &mut TestAppContext,
    ) {
        let (_server, window) = signed_in(cx);
        let visual = drawn(&window, cx);
        let card = a_card(&window, visual, "home-");
        let picture = play_on_card(&window, visual, &card);
        let back = back_onto(&window, visual).expect("back to the card");
        // Where the card is now: the page may be sliding in.
        assert_eq!(back, bounds(visual, format!("card-hover {card}")));
        assert_eq!(back.size, picture.size);
    }

    #[gpui_kit::test]
    fn leaving_the_player_fades_the_picture_out_when_its_card_is_out_of_sight(
        cx: &mut TestAppContext,
    ) {
        let (_server, window) = signed_in(cx);
        let visual = drawn(&window, cx);
        let card = a_card(&window, visual, "home-");
        play_on_card(&window, visual, &card);
        // The window got lower meanwhile: the card is under its bottom.
        visual.simulate_resize(size(px(1600.), px(500.)));
        assert_eq!(back_onto(&window, visual), None);
    }

    #[gpui_kit::test]
    fn starting_a_film_from_its_page_plays_the_transition_then_the_video(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let player = FakePlayer::default();
        let queued = player.queued.clone();
        queued.borrow_mut().push(PlayerEvent::Loaded);
        play_from_its_page(&window, player, cx);
        assert_eq!(screen(&window, cx), Screen::Player);
        assert_eq!(way(&window, cx), Some(Way::Opening));
        tick(cx);
        assert!(
            waiting(&window, cx),
            "the video is covered until its first frame"
        );
        queued.borrow_mut().push(PlayerEvent::FirstFrame);
        tick(cx);
        assert!(
            !waiting(&window, cx),
            "the first frame takes the cover away"
        );
        let over = window
            .read_with(cx, |shell, _| {
                let later = Instant::now() + Duration::from_secs(1);
                shell.curtain.as_ref().is_none_or(|c| c.over(later))
            })
            .unwrap();
        assert!(over, "then the video, and nothing over it");
    }

    #[gpui_kit::test]
    fn a_player_slow_to_answer_is_not_asked_while_the_way_in_moves(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let player = FakePlayer {
            slow: true,
            ..Default::default()
        };
        let queued = player.queued.clone();
        queued.borrow_mut().push(PlayerEvent::Loaded);
        play_from_its_page(&window, player, cx);
        let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        // The clock of the move is the real one: the test says where it stands.
        let starts = |at: Instant, cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, _| {
                    shell.curtain.as_mut().unwrap().started_at(at)
                })
                .unwrap()
        };
        starts(Instant::now() + Duration::from_secs(3600), cx);
        queued.borrow_mut().push(PlayerEvent::EndOfFile);
        tick(cx);
        assert_eq!(
            screen(&window, cx),
            Screen::Player,
            "not asked while it moves"
        );
        // Grown, the cover waits without moving: the player is asked again.
        starts(Instant::now() - Duration::from_secs(60), cx);
        tick(cx);
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
    }

    #[gpui_kit::test]
    fn an_episode_with_nothing_to_play_leaves_the_next_playback_its_transition(
        cx: &mut TestAppContext,
    ) {
        let (_server, window) = signed_in(cx);
        let mut episode = series_episodes(&window, cx).remove(0);
        episode.versions.clear();
        // The card of the next episode plays it, and there is no version to play.
        window
            .update(cx, |shell, window, cx| {
                shell.play_next(Next::Episode(episode), window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::SeriesDetail);
        window
            .read_with(cx, |shell, _| {
                assert!(shell.series_pages.last().unwrap().error.is_some());
            })
            .unwrap();
        // Later, a film from its page: that is a new way into the player.
        let player = FakePlayer::default();
        player.queued.borrow_mut().push(PlayerEvent::Loaded);
        play_from_its_page(&window, player, cx);
        assert_eq!(screen(&window, cx), Screen::Player);
        assert_eq!(way(&window, cx), Some(Way::Opening));
    }

    #[gpui_kit::test]
    fn next_pressed_while_the_player_opens_goes_to_the_next_episode_at_once(
        cx: &mut TestAppContext,
    ) {
        let (_server, window) = signed_in(cx);
        let episodes = series_episodes(&window, cx);
        let player = FakePlayer {
            slow: true,
            ..Default::default()
        };
        let queued = player.queued.clone();
        queued
            .borrow_mut()
            .extend([PlayerEvent::Loaded, PlayerEvent::Duration(1800.)]);
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(player);
                shell.play_episode(episodes[0].clone(), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        // The way in is drawn and moving.
        let visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        queued
            .borrow_mut()
            .extend([PlayerEvent::Position(1799.), PlayerEvent::EndOfFile]);
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(FakePlayer::default());
                shell.player_request(Some(WindowRequest::NextEpisode), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let playing = window
            .read_with(cx, |shell, _| shell.playback.as_ref()?.playing.clone())
            .unwrap();
        assert_eq!(playing, Some(episodes[1].item.id.clone()));
    }

    #[gpui_kit::test]
    fn the_picture_of_the_films_page_grows_into_the_player_and_back(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        open_film_page(&window, cx);
        draw_page(&window, cx);
        let drawn = window
            .read_with(cx, |shell, _| {
                shell
                    .pictures
                    .borrow()
                    .contains_key(&crate::curtain::PAGE.into())
            })
            .unwrap();
        assert!(drawn, "the page drew its backdrop, in sight");
        let player = FakePlayer::default();
        player
            .queued
            .borrow_mut()
            .extend([PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        press_play(&window, player, cx);
        let grows = window
            .read_with(cx, |shell, _| {
                shell.curtain.as_ref().is_some_and(|c| c.picture.is_some())
            })
            .unwrap();
        assert!(grows, "the way in grows from the backdrop");
        tick(cx);
        window
            .update(cx, |shell, window, cx| shell.player_back(window, cx))
            .unwrap();
        assert_eq!(way(&window, cx), Some(Way::Closing));
        let shrinks = window
            .read_with(cx, |shell, _| {
                shell.curtain.as_ref().is_some_and(|c| c.picture.is_some())
            })
            .unwrap();
        assert!(shrinks, "the way out goes back to it");
    }

    #[gpui_kit::test]
    fn leaving_the_player_returns_to_the_page_with_the_reverse_transition(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let player = FakePlayer::default();
        player
            .queued
            .borrow_mut()
            .extend([PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        open_film_page(&window, cx);
        draw_page(&window, cx);
        press_play(&window, player, cx);
        tick(cx);
        window
            .update(cx, |shell, window, cx| shell.player_back(window, cx))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        assert_eq!(way(&window, cx), Some(Way::Closing));
    }

    #[gpui_kit::test]
    fn leaving_a_player_opened_from_home_has_no_way_out_to_draw(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let player = FakePlayer::default();
        player
            .queued
            .borrow_mut()
            .extend([PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        // Played with no picture pressed: the way in only fades.
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(player);
                shell.play_from_home(PlayTarget::Film(ItemId(FILM.into())), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Player);
        tick(cx);
        window
            .update(cx, |shell, window, cx| shell.player_back(window, cx))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Home);
        assert_eq!(way(&window, cx), None, "nothing to shrink back");
    }

    #[gpui_kit::test]
    fn an_error_while_opening_ends_on_the_page_with_no_half_drawn_player(cx: &mut TestAppContext) {
        let (server, window) = signed_in(cx);
        server.state.lock().unwrap().play_error =
            Some(ProviderError::Unreachable("invented".into()));
        play_from_its_page(&window, FakePlayer::default(), cx);
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        window
            .read_with(cx, |shell, _| {
                assert!(shell.playback.is_none());
                assert!(
                    shell.curtain.is_none(),
                    "no way into a player that is not there"
                );
                assert!(
                    shell.details.last().unwrap().error.is_some(),
                    "the page says why"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn next_up_autoplay_goes_episode_to_episode_without_replaying_the_page_transition(
        cx: &mut TestAppContext,
    ) {
        let (_server, window) = signed_in(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(SERIES.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
        let episodes = window
            .read_with(cx, |shell, _| {
                let page = shell.series_pages.last().unwrap();
                match &page.episodes[&page.seasons[page.season].id] {
                    Loadable::Ready(episodes) => episodes.clone(),
                    _ => panic!("the episodes are shown"),
                }
            })
            .unwrap();
        let player = FakePlayer::default();
        let queued = player.queued.clone();
        queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::FirstFrame,
            PlayerEvent::Duration(1800.),
        ]);
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(player);
                shell.play_episode(episodes[0].clone(), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(way(&window, cx), Some(Way::Opening), "the first one opens");
        tick(cx);
        // The episode ends: the card counts down over the series page, then the next plays.
        queued
            .borrow_mut()
            .extend([PlayerEvent::Position(1799.), PlayerEvent::EndOfFile]);
        tick(cx);
        assert_eq!(screen(&window, cx), Screen::SeriesDetail);
        assert_eq!(way(&window, cx), None, "no way out towards the card");
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(FakePlayer::default());
                shell.play_up_next_now(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Player);
        let playing = window
            .read_with(cx, |shell, _| shell.playback.as_ref()?.playing.clone())
            .unwrap();
        assert_eq!(playing, Some(episodes[1].item.id.clone()));
        assert_eq!(way(&window, cx), None, "the next episode opens without it");
    }
}
