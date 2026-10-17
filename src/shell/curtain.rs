//! The way into the player and back out of it, on the window: see `crate::curtain`.

use std::time::{Duration, Instant};

use bobine::provider::ItemId;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Bounds, Context, ObjectFit, Window, div, img, point, px};

use super::Shell;
use crate::curtain::{Curtain, Picture, Way, between};
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

    /// The way in grows from the picture of `item`, when the page drew it in sight; the way out
    /// goes back to it.
    pub(super) fn grow_from(&mut self, item: Option<&ItemId>, window: &Window) {
        let height = window.viewport_size().height;
        let picture = self.pictures.borrow().clone().filter(|picture| {
            Some(&picture.item) == item
                && picture.bounds.bottom() > px(0.)
                && picture.bounds.top() < height
        });
        if let Some(curtain) = &mut self.curtain
            && curtain.way() == Way::Opening
        {
            curtain.picture = picture.clone();
        }
        self.opened_from = picture;
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
        self.curtain = (!next_offered).then(|| Curtain::closing(picture, Instant::now()));
        if !next_offered {
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
                let picture = curtain.picture.as_ref()?;
                // Back to where the page draws it now: the window may have changed meanwhile.
                let place = (self.pictures.borrow().as_ref())
                    .filter(|drawn| drawn.item == picture.item)
                    .map_or(picture.bounds, |drawn| drawn.bounds);
                let element = drawn_at(picture, between(place, whole, frame.grown));
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
    use gpui_kit::{TestAppContext, WindowHandle};

    use crate::curtain::Way;
    use crate::fake::{FILM, FakeServer, SERIES};
    use crate::film_page::From;
    use crate::navigation::Screen;
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

    /// The film's page open, then its Play pressed, on `player`.
    fn play_from_its_page(
        window: &WindowHandle<Shell>,
        player: FakePlayer,
        cx: &mut TestAppContext,
    ) {
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(window, cx), Screen::FilmDetail);
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(player);
                shell.play_film(From::Resume, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
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
    fn leaving_the_player_returns_to_the_page_with_the_reverse_transition(cx: &mut TestAppContext) {
        let (_server, window) = signed_in(cx);
        let player = FakePlayer::default();
        player
            .queued
            .borrow_mut()
            .extend([PlayerEvent::Loaded, PlayerEvent::FirstFrame]);
        play_from_its_page(&window, player, cx);
        tick(cx);
        window
            .update(cx, |shell, window, cx| shell.player_back(window, cx))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        assert_eq!(way(&window, cx), Some(Way::Closing));
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
