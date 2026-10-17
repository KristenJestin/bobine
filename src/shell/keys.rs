//! What the keys of `crate::keys` do on the window: the player's and the pages'.

use gpui_kit::prelude::*;
use gpui_kit::{Action, Context, Div, KeyContext, Stateful, Window};

use super::Shell;
use crate::keys::*;
use crate::navigation::{NavEntry, Screen};
use crate::player::controls::{KeyAction, SKIP_SECONDS};
use crate::player::session::VOLUME_STEP;

/// What a long seek skips.
const LONG_SKIP: f64 = 60.;

impl Shell {
    /// The key context of the window's root: the screen, and what over it takes the keys.
    pub(super) fn key_context(&self, screen: Screen) -> KeyContext {
        let mut context = KeyContext::default();
        context.add("Bobine");
        context.add(if screen == Screen::Player && self.playback.is_some() {
            "Player"
        } else {
            "Browse"
        });
        let over = if self.search_is_open() {
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
            .on_action(
                cx.listener(|shell, _: &MenuConfirm, _, cx| shell.playlist_menu_key("enter", cx)),
            )
    }

    /// Escape: what is over the page closes first, then the player or the page does its own.
    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_is_open() {
            self.search_key("escape", window, cx);
        } else if self.playlist_menu_is_open() {
            self.playlist_menu_key("escape", cx);
        } else if self.navigator.current() == Screen::Player && self.playback.is_some() {
            self.player_key(KeyAction::Escape, window, cx);
        } else {
            self.escape(cx);
        }
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

/// `A` is the palette's `key`.
fn palette<A: Action>(
    root: Stateful<Div>,
    key: &'static str,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    root.on_action(cx.listener(move |shell, _: &A, window, cx| shell.search_key(key, window, cx)))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bobine::provider::ItemId;
    use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle};

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
}
