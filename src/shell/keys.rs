//! What the keys of `crate::keys` do on the window: the player's, the pages', the focus ring of
//! the cards and the help.

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    Action, Bounds, Context, Div, KeyContext, MouseMoveEvent, Pixels, Stateful, Window, canvas,
    point, px,
};

use super::Shell;
use crate::keys::*;
use crate::navigation::{NavEntry, Screen};
use crate::player::controls::{KeyAction, SKIP_SECONDS};
use crate::player::session::VOLUME_STEP;
use crate::ring::{self, Direction, Place};
use crate::screens::{Open, open_card, play_card};

/// What a long seek skips.
const LONG_SKIP: f64 = 60.;
/// The room a card scrolled into view keeps from the sides of its row.
const ROW_ROOM: f32 = 40.;
/// The room it keeps from the top of the window, under the pill bar, and from its bottom, for
/// the caption under the picture.
const BAR_ROOM: f32 = 104.;
const FOOT_ROOM: f32 = 64.;
/// The gap between two cards, added to a card's size when the page moves on by one.
const CARD_GAP: f32 = 16.;

impl Shell {
    /// The key context of the window's root: the screen, and what over it takes the keys.
    pub(super) fn key_context(&self, screen: Screen) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("Bobine");
        if screen == Screen::Player && self.playback.is_some() {
            context.add("Player");
        } else if self.server.is_some() && screen != Screen::Login {
            context.add("Browse");
        }
        // Without a server, no page has keys yet: the login keeps its own.
        let over = if self.help_open {
            Some("Help")
        } else if self.search_is_open() {
            Some("Palette")
        } else if self.playlist_menu_is_open() {
            Some("PlaylistMenu")
        } else {
            None
        };
        if let Some(over) = over {
            context.add("Modal");
            context.add(over);
        }
        context
    }

    /// The root of the window, answering every key of the keymap.
    pub(super) fn answer_keys(root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        use KeyAction as K;
        let root = root.on_mouse_move(cx.listener(|shell, _: &MouseMoveEvent, _, cx| {
            if shell.ring.hide() {
                shell.notify_ringed(cx);
                cx.notify();
            }
        }));
        let root = player::<PlayPause>(root, K::TogglePause, cx);
        let root = player::<SeekBack>(root, K::SeekBy(-SKIP_SECONDS), cx);
        let root = player::<SeekForward>(root, K::SeekBy(SKIP_SECONDS), cx);
        let root = player::<SeekBackLong>(root, K::SeekBy(-LONG_SKIP), cx);
        let root = player::<SeekForwardLong>(root, K::SeekBy(LONG_SKIP), cx);
        let root = player::<VolumeUp>(root, K::Volume(VOLUME_STEP), cx);
        let root = player::<VolumeDown>(root, K::Volume(-VOLUME_STEP), cx);
        let root = player::<Mute>(root, K::ToggleMute, cx);
        let root = player::<Fullscreen>(root, K::ToggleFullscreen, cx);
        let root = player::<CycleSubtitles>(root, K::CycleSubtitles, cx);
        let root = player::<CycleAudio>(root, K::CycleAudio, cx);
        let root = player::<PlayNext>(root, K::Next, cx);
        let root = player::<SkipSegment>(root, K::PressSkip, cx);
        let root = player::<SubtitlesEarlier>(root, K::ShiftSubtitles(-1), cx);
        let root = player::<SubtitlesLater>(root, K::ShiftSubtitles(1), cx);
        let root = player::<Slower>(root, K::Speed(-1), cx);
        let root = player::<Faster>(root, K::Speed(1), cx);
        let root = pill::<Pill1>(root, 0, cx);
        let root = pill::<Pill2>(root, 1, cx);
        let root = pill::<Pill3>(root, 2, cx);
        let root = pill::<Pill4>(root, 3, cx);
        let root = pill::<Pill5>(root, 4, cx);
        let root = pill::<Pill6>(root, 5, cx);
        let root = arrow::<FocusLeft>(root, Direction::Left, cx);
        let root = arrow::<FocusRight>(root, Direction::Right, cx);
        let root = arrow::<FocusUp>(root, Direction::Up, cx);
        let root = arrow::<FocusDown>(root, Direction::Down, cx);
        let root = palette::<PaletteUp>(root, "up", cx);
        let root = palette::<PaletteDown>(root, "down", cx);
        let root = palette::<PaletteOpen>(root, "enter", cx);
        root.on_action(cx.listener(|shell, _: &Dismiss, window, cx| shell.dismiss(window, cx)))
            .on_action(cx.listener(|shell, _: &Search, window, cx| {
                shell.open_search(window, cx);
            }))
            .on_action(cx.listener(|shell, _: &Back, _, cx| {
                shell.go_back(cx);
            }))
            .on_action(cx.listener(|shell, _: &OpenFocused, window, cx| {
                shell.press_ringed(false, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &PlayFocused, window, cx| {
                shell.press_ringed(true, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &Help, _, cx| shell.toggle_help(cx)))
            .on_action(
                cx.listener(|shell, _: &MenuConfirm, _, cx| shell.playlist_menu_key("enter", cx)),
            )
    }

    /// Escape: what is over the page closes first, then the player or the page does its own.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.help_open {
            self.close_help(cx);
        } else if self.search_is_open() {
            self.search_key("escape", window, cx);
        } else if self.playlist_menu_is_open() {
            self.playlist_menu_key("escape", cx);
        } else if self.navigator.current() == Screen::Player && self.playback.is_some() {
            self.player_key(KeyAction::Escape, window, cx);
        } else {
            self.escape(cx);
        }
    }

    /// `?`: the help over the screen, or away. Never over the palette or a menu.
    fn toggle_help(&mut self, cx: &mut Context<Self>) {
        if self.help_open {
            self.close_help(cx);
        } else if !self.search_is_open() && !self.playlist_menu_is_open() {
            self.help_open = true;
            self.help_fade.set(true, Instant::now());
            cx.notify();
        }
    }

    /// A grid of a uniform list drawn: the ring scrolls it too.
    pub fn scrolls_list(&mut self, list: &gpui_kit::UniformListScrollHandle) {
        self.scrolls.push(list.0.borrow().base_handle.clone());
    }

    /// Whether the help is open, and its fade.
    pub fn help(&self) -> (bool, crate::motion::Fade) {
        (self.help_open, self.help_fade)
    }

    pub fn close_help(&mut self, cx: &mut Context<Self>) {
        self.help_open = false;
        self.help_fade.set(false, Instant::now());
        cx.notify();
    }

    /// The card the ring and the card it leaves draw again.
    fn notify_ringed(&self, cx: &mut Context<Self>) {
        if let Some(id) = self.ring.card() {
            self.cards.notify(id, cx);
        }
    }

    /// An arrow over a page: the ring shows, or goes to the next card that way. With no card
    /// drawn that way, the row or the page moves on by one card and, if `then_again`, the arrow
    /// is tried again once it is drawn.
    pub(super) fn move_ring(
        &mut self,
        direction: Direction,
        then_again: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cards: Vec<_> = self
            .cards
            .places()
            .into_iter()
            .map(|(id, bounds)| (id, Place::of(bounds)))
            .collect();
        let from = self
            .ring
            .shown_on()
            .and_then(|id| cards.iter().find(|(card, _)| card == id))
            .map(|(_, place)| *place);
        let to = match from {
            Some(from) => ring::next(from, &cards, direction),
            None => {
                let size = window.viewport_size();
                let view = Place {
                    left: 0.,
                    top: 0.,
                    right: size.width.into(),
                    bottom: size.height.into(),
                };
                ring::first(&cards, view)
            }
        };
        self.notify_ringed(cx);
        match (to, from) {
            (Some(to), _) => {
                let place = cards
                    .iter()
                    .find(|(card, _)| *card == to)
                    .map(|(_, place)| *place);
                self.ring.show_on(to);
                if let Some(place) = place {
                    self.scroll_into_view(place);
                }
            }
            (None, Some(from)) if then_again => {
                if self.scroll_on(from, direction) {
                    self.ring.pending = Some(direction);
                }
            }
            (None, _) => {}
        }
        self.notify_ringed(cx);
        cx.notify();
    }

    /// The rows and the pages that hold `place` on the axis of `horizontal`: a row is across
    /// it, a page above and under it.
    fn scrolls_holding(
        &self,
        place: Place,
        horizontal: bool,
    ) -> impl Iterator<Item = (&gpui_kit::ScrollHandle, Place)> {
        let middle = (
            (place.left + place.right) / 2.,
            (place.top + place.bottom) / 2.,
        );
        self.scrolls.iter().filter_map(move |handle| {
            let area = Place::of(handle.bounds());
            let max = handle.max_offset();
            let holds = if horizontal {
                f32::from(max.x) > 0. && (area.top..area.bottom).contains(&middle.1)
            } else {
                f32::from(max.y) > 0. && (area.left..area.right).contains(&middle.0)
            };
            holds.then_some((handle, area))
        })
    }

    /// The row and the page move so that the card at `place` shows whole.
    fn scroll_into_view(&self, place: Place) {
        for horizontal in [true, false] {
            for (handle, area) in self.scrolls_holding(place, horizontal) {
                let (room, end_room) = if horizontal {
                    (ROW_ROOM, ROW_ROOM)
                } else {
                    ((BAR_ROOM - area.top).max(8.), FOOT_ROOM)
                };
                let by = ring::into_view(place, area, horizontal, room, end_room);
                move_by(handle, by, horizontal);
            }
        }
    }

    /// The row or the page under `place` moves on by one card `direction`; whether it moved.
    fn scroll_on(&self, place: Place, direction: Direction) -> bool {
        let horizontal = direction.horizontal();
        let size = if horizontal {
            place.right - place.left
        } else {
            place.bottom - place.top
        } + CARD_GAP;
        let by = if direction.forward() { -size } else { size };
        let mut moved = false;
        for (handle, _) in self.scrolls_holding(place, horizontal) {
            moved |= move_by(handle, by, horizontal);
        }
        moved
    }

    /// An arrow that found no card where they were drawn: drawn last over the window, once the
    /// cards the row or the page moved on are, it is tried again where they are now.
    pub(super) fn ring_again(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let direction = self.ring.pending.take()?;
        let shell = cx.weak_entity();
        let again = move |_: Bounds<Pixels>, _: (), window: &mut Window, cx: &mut gpui_kit::App| {
            window.defer(cx, move |window, cx| {
                let go = |shell: &mut Shell, cx: &mut Context<Shell>| {
                    shell.move_ring(direction, false, window, cx)
                };
                shell.update(cx, go).ok();
            });
        };
        Some(canvas(|_, _, _| {}, again).absolute().size_full())
    }

    /// Enter or P on the card the ring shows on: what a click on it, or on its play button,
    /// does. An episode's row plays either way.
    fn press_ringed(&mut self, play: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(offer) = self.ring.shown_on().and_then(|id| self.cards.offer(id)) else {
            return;
        };
        self.ring.hide();
        if play || matches!(offer.open, Open::Row(_)) {
            play_card(self, offer.play, window, cx);
        } else {
            open_card(self, offer.open, cx);
        }
        cx.notify();
    }
}

/// `A` does `action` in the player.
fn player<A: Action>(
    root: Stateful<Div>,
    action: KeyAction,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    root.on_action(
        cx.listener(move |shell, _: &A, window, cx| shell.player_key(action, window, cx)),
    )
}

/// `A` goes to the pill `at`.
fn pill<A: Action>(root: Stateful<Div>, at: usize, cx: &mut Context<Shell>) -> Stateful<Div> {
    root.on_action(cx.listener(move |shell, _: &A, _, cx| shell.select(NavEntry::ALL[at], cx)))
}

/// `A` moves the ring `direction`.
fn arrow<A: Action>(
    root: Stateful<Div>,
    direction: Direction,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    root.on_action(
        cx.listener(move |shell, _: &A, window, cx| shell.move_ring(direction, true, window, cx)),
    )
}

/// `A` is the palette's `key`.
fn palette<A: Action>(
    root: Stateful<Div>,
    key: &'static str,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    root.on_action(cx.listener(move |shell, _: &A, window, cx| shell.search_key(key, window, cx)))
}

/// Moves the scroll of `handle` by `by` on one axis, within its bounds; whether it moved.
fn move_by(handle: &gpui_kit::ScrollHandle, by: f32, horizontal: bool) -> bool {
    let (offset, max) = (handle.offset(), handle.max_offset());
    let to = if horizontal {
        let x = (f32::from(offset.x) + by).clamp(-f32::from(max.x), 0.);
        point(px(x), offset.y)
    } else {
        let y = (f32::from(offset.y) + by).clamp(-f32::from(max.y), 0.);
        point(offset.x, px(y))
    };
    if to == offset {
        return false;
    }
    handle.set_offset(to);
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bobine::provider::ItemId;
    use gpui_kit::{
        ElementId, Modifiers, TestAppContext, VisualTestContext, WindowHandle, point, px,
    };

    use super::*;
    use crate::fake::{FILM, FakeServer, SERIES};
    use crate::home_page::PlayTarget;
    use crate::player::fake::{Call, FakePlayer};
    use crate::player::{PlayerEvent, Seek, Track, TrackKind};

    fn signed_in(cx: &mut TestAppContext) -> WindowHandle<Shell> {
        signed_in_with(12, cx)
    }

    /// A shell signed in on a server of `films` films.
    fn signed_in_with(films: usize, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let connector: Arc<dyn bobine::provider::Connector> =
            Arc::new(FakeServer::new(films, true));
        let window = cx.add_window(|_, cx| {
            crate::input::bind_keys(cx);
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
        window
    }

    fn drawn(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> VisualTestContext {
        let visual = VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        visual
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .read_with(cx, |shell, _| shell.navigator.current())
            .unwrap()
    }

    fn ringed(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<ElementId> {
        window
            .read_with(cx, |shell, _| shell.ring.shown_on().cloned())
            .unwrap()
    }

    /// A film playing on a fake player; what it was asked goes in the calls returned.
    fn playing(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<Call>>> {
        let player = FakePlayer::default();
        let calls = player.calls.clone();
        player.queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::Duration(5400.),
            PlayerEvent::Tracks(vec![
                track(TrackKind::Audio, 1, true),
                track(TrackKind::Audio, 2, false),
                track(TrackKind::Subtitle, 3, false),
            ]),
        ]);
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(player);
                shell.play_from_home(PlayTarget::Film(ItemId(FILM.into())), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(super::super::PLAYER_TICK * 2);
        cx.run_until_parked();
        assert_eq!(screen(window, cx), Screen::Player);
        calls
    }

    fn track(kind: TrackKind, id: i64, selected: bool) -> Track {
        Track {
            id,
            kind,
            title: None,
            language: Some("fre".into()),
            codec: Some("aac".into()),
            channels: None,
            forced: false,
            external: false,
            selected,
        }
    }

    #[gpui_kit::test]
    fn each_player_key_acts_on_the_fake_player(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let calls = playing(&window, cx);
        let mut visual = drawn(&window, cx);
        let pressed = [
            ("space", Call::Paused(true)),
            ("k", Call::Paused(false)),
            ("left", Call::Seek(Seek::By(-10.))),
            ("j", Call::Seek(Seek::By(-10.))),
            ("right", Call::Seek(Seek::By(10.))),
            ("l", Call::Seek(Seek::By(10.))),
            ("shift-left", Call::Seek(Seek::By(-60.))),
            ("shift-right", Call::Seek(Seek::By(60.))),
            ("down", Call::Volume(95.)),
            ("up", Call::Volume(100.)),
            ("m", Call::Muted(true)),
            ("c", Call::Subtitle(Some(3))),
            ("a", Call::Audio(Some(2))),
            ("[", Call::SubtitleDelay(-0.1)),
            ("]", Call::SubtitleDelay(0.)),
            ("<", Call::Speed(0.75)),
            (">", Call::Speed(1.)),
        ];
        for (key, call) in pressed {
            let before = calls.borrow().len();
            visual.simulate_keystrokes(key);
            let asked: Vec<Call> = calls.borrow()[before..]
                .iter()
                .filter(|call| !matches!(call, Call::SubtitlePosition(_)))
                .cloned()
                .collect();
            assert_eq!(asked, [call], "{key}");
        }
        // Ctrl+F or Alt+Left belong to nobody in the player.
        let before = calls.borrow().len();
        visual.simulate_keystrokes("ctrl-f alt-left ctrl-k");
        assert_eq!(calls.borrow().len(), before);
        assert_eq!(screen(&window, cx), Screen::Player);
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn the_volume_and_the_mute_carry_to_the_next_film(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        playing(&window, cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("down m escape");
        assert_eq!(screen(&window, cx), Screen::Home);
        // A new mpv starts loud and unmuted: it is told what the one before was left at.
        let calls = playing(&window, cx);
        let asked = calls.borrow().clone();
        assert!(asked.contains(&Call::Volume(95.)), "{asked:?}");
        assert!(asked.contains(&Call::Muted(true)), "{asked:?}");
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("m");
        assert_eq!(calls.borrow().last(), Some(&Call::Muted(false)));
    }

    #[gpui_kit::test]
    fn n_plays_the_next_episode_at_once(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(SERIES.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
        let episodes = window
            .read_with(cx, |shell, _| {
                let page = shell.series_pages.last().unwrap();
                let season = &page.seasons[page.season].id;
                match &page.episodes[season] {
                    crate::shell::Loadable::Ready(episodes) => episodes.clone(),
                    _ => panic!("the episodes are shown"),
                }
            })
            .unwrap();
        window
            .update(cx, |shell, window, cx| {
                shell.fake_player = Some(FakePlayer::default());
                shell.play_episode(episodes[0].clone(), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Player);
        window
            .update(cx, |shell, _, _| {
                shell.fake_player = Some(FakePlayer::default())
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("n");
        visual.run_until_parked();
        let now = window
            .read_with(cx, |shell, _| shell.playback.as_ref()?.playing.clone())
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Player);
        assert_eq!(now, Some(episodes[1].item.id.clone()));
    }

    #[gpui_kit::test]
    fn arrow_focus_moves_through_a_row_and_a_grid_and_enter_opens(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        assert_eq!(ringed(&window, cx), None, "no ring before a key");
        // The first arrow shows the ring on the first card in view; the next ones move it.
        visual.simulate_keystrokes("right");
        let first = ringed(&window, cx).expect("the ring shows");
        visual.simulate_keystrokes("right");
        let second = ringed(&window, cx).unwrap();
        assert_ne!(first, second);
        visual.simulate_keystrokes("left");
        assert_eq!(ringed(&window, cx), Some(first.clone()));
        visual.simulate_keystrokes("down");
        let below = ringed(&window, cx).unwrap();
        assert_ne!(below, first, "the row below");
        // The mouse hides it again.
        visual.simulate_mouse_move(point(px(300.), px(300.)), None, Modifiers::none());
        assert_eq!(ringed(&window, cx), None);

        // A grid: Movies.
        visual.simulate_keystrokes("alt-2");
        assert_eq!(screen(&window, cx), Screen::Library);
        let mut visual = drawn(&window, cx);
        let columns = window
            .update(cx, |_, window, _| {
                crate::grid::columns(window.viewport_size().width.into())
            })
            .unwrap();
        visual.simulate_keystrokes("right");
        assert_eq!(ringed(&window, cx), Some(("film", 0usize).into()));
        visual.simulate_keystrokes("right");
        assert_eq!(ringed(&window, cx), Some(("film", 1usize).into()));
        visual.simulate_keystrokes("down");
        assert_eq!(ringed(&window, cx), Some(("film", columns + 1).into()));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
    }

    #[gpui_kit::test]
    fn the_ring_scrolls_its_card_into_view_down_a_grid(cx: &mut TestAppContext) {
        let window = signed_in_with(200, cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("alt-2");
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("right");
        let lines = 6;
        for _ in 0..lines {
            visual.simulate_keystrokes("down");
            visual.run_until_parked();
        }
        let columns = window
            .update(cx, |_, window, _| {
                crate::grid::columns(window.viewport_size().width.into())
            })
            .unwrap();
        assert_eq!(ringed(&window, cx), Some(("film", lines * columns).into()));
        let selector = format!("card-ring film-{}", lines * columns);
        let bounds = visual
            .debug_bounds(Box::leak(selector.into_boxed_str()))
            .expect("the ring is drawn");
        let height = window
            .update(cx, |_, window, _| f32::from(window.viewport_size().height))
            .unwrap();
        assert!(
            f32::from(bounds.bottom()) <= height,
            "{bounds:?} in {height}"
        );
    }

    #[gpui_kit::test]
    fn p_plays_the_title_the_ring_is_on(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("alt-2");
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("right");
        window
            .update(cx, |shell, _, _| {
                shell.fake_player = Some(FakePlayer::default())
            })
            .unwrap();
        visual.simulate_keystrokes("p");
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Player);
    }

    fn help_lines(visual: &mut VisualTestContext) -> (bool, bool) {
        (
            visual.debug_bounds("help-line Search").is_some(),
            visual.debug_bounds("help-line Play or pause").is_some(),
        )
    }

    #[gpui_kit::test]
    fn question_mark_shows_the_help_with_the_screens_keys(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("?");
        cx.executor().advance_clock(Duration::from_secs(1));
        let mut visual = drawn(&window, cx);
        assert_eq!(help_lines(&mut visual), (true, false), "the pages' keys");
        assert!(
            visual.debug_bounds("help-line Discover").is_none(),
            "no Discover pill, no line for it"
        );
        // Under the help, the page's keys do nothing; `?` closes it.
        visual.simulate_keystrokes("alt-2 right");
        assert_eq!(screen(&window, cx), Screen::Home);
        assert_eq!(ringed(&window, cx), None);
        visual.simulate_keystrokes("?");
        assert!(!window.read_with(cx, |shell, _| shell.help_open).unwrap());
        // Escape closes it too, and a click outside its panel.
        visual.simulate_keystrokes("? escape");
        assert!(!window.read_with(cx, |shell, _| shell.help_open).unwrap());
        assert_eq!(screen(&window, cx), Screen::Home);
        visual.simulate_keystrokes("?");
        visual.simulate_click(point(px(5.), px(5.)), Modifiers::none());
        assert!(!window.read_with(cx, |shell, _| shell.help_open).unwrap());

        // The player's own keys.
        playing(&window, cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("?");
        cx.executor().advance_clock(Duration::from_secs(1));
        let mut visual = drawn(&window, cx);
        assert_eq!(help_lines(&mut visual), (false, true), "the player's keys");
        visual.simulate_keystrokes("escape");
        assert_eq!(
            screen(&window, cx),
            Screen::Player,
            "Escape closed the help only"
        );
    }

    #[gpui_kit::test]
    fn typing_in_a_field_does_not_trigger_keys(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("ctrl-k");
        assert!(
            window
                .read_with(cx, |shell, _| shell.search_is_open())
                .unwrap()
        );
        visual.simulate_input("?pk");
        visual.simulate_keystrokes("alt-2 backspace");
        visual.run_until_parked();
        window
            .read_with(cx, |shell, cx| {
                assert!(!shell.help_open);
                assert_eq!(shell.navigator.current(), Screen::Home);
                let typed = shell.search.as_ref().unwrap().input.read(cx).text();
                assert_eq!(typed, "?p", "Backspace edits the field");
            })
            .unwrap();
        // Escape still closes the palette, and the page's keys come back.
        visual.simulate_keystrokes("escape");
        assert!(
            !window
                .read_with(cx, |shell, _| shell.search_is_open())
                .unwrap()
        );
        visual.simulate_keystrokes("alt-2");
        assert_eq!(screen(&window, cx), Screen::Library);
    }

    #[gpui_kit::test]
    fn ctrl_k_alone_opens_and_closes_the_palette(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("ctrl-shift-k");
        assert!(
            !window
                .read_with(cx, |shell, _| shell.search_is_open())
                .unwrap()
        );
        visual.simulate_keystrokes("ctrl-k");
        assert!(
            window
                .read_with(cx, |shell, _| shell.search_is_open())
                .unwrap()
        );
        visual.simulate_keystrokes("ctrl-k");
        assert!(
            !window
                .read_with(cx, |shell, _| shell.search_is_open())
                .unwrap()
        );
    }

    #[gpui_kit::test]
    fn alt_and_a_number_go_to_the_pills_and_alt_left_comes_back(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        for (keys, screen_reached) in [
            ("alt-2", Screen::Library),
            ("alt-3", Screen::Series),
            ("alt-4", Screen::Collections),
            ("alt-5", Screen::MyList),
            ("alt-1", Screen::Home),
        ] {
            visual.simulate_keystrokes(keys);
            visual.run_until_parked();
            assert_eq!(screen(&window, cx), screen_reached, "{keys}");
        }
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("alt-left");
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn alt_and_a_number_on_the_login_stays_on_the_login(cx: &mut TestAppContext) {
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(FakeServer::new(12, false));
        let window = cx.add_window(|_, cx| {
            crate::input::bind_keys(cx);
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(crate::fake::MemoryKeyring::default()),
                cx,
            )
        });
        let mut visual = drawn(&window, cx);
        assert_eq!(screen(&window, cx), Screen::Login);
        // Not in a field: the root has the keys.
        window
            .update(cx, |shell, window, cx| window.focus(&shell.focus, cx))
            .unwrap();
        visual.simulate_keystrokes("alt-1 alt-2 ?");
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Login);
        assert!(
            !window.read_with(cx, |shell, _| shell.help_open).unwrap(),
            "no keys to show yet"
        );
    }

    #[gpui_kit::test]
    fn on_azerty_alt_and_the_unshifted_number_row_go_to_the_pills(cx: &mut TestAppContext) {
        let window = signed_in(cx);
        let mut visual = drawn(&window, cx);
        // What the number row produces on AZERTY, without Shift.
        for (keys, screen_reached) in [
            ("alt-eacute", Screen::Library),
            ("alt-\"", Screen::Series),
            ("alt-'", Screen::Collections),
            ("alt-(", Screen::MyList),
            ("alt-&", Screen::Home),
        ] {
            visual.simulate_keystrokes(keys);
            visual.run_until_parked();
            assert_eq!(screen(&window, cx), screen_reached, "{keys}");
        }
    }

    #[gpui_kit::test]
    fn alt_6_goes_to_discover_when_a_request_server_is_configured(cx: &mut TestAppContext) {
        let seerr = crate::fake_seerr::FakeSeerr::default();
        let window = super::super::discover::tests::configured(&seerr, cx);
        cx.update(crate::keys::bind);
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("alt-6");
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Discover);
        visual.simulate_keystrokes("alt-1");
        visual.run_until_parked();
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("alt--");
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::Discover, "AZERTY's 6");
        visual.simulate_keystrokes("?");
        cx.executor().advance_clock(Duration::from_secs(1));
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("help-line Discover").is_some());
    }
}
