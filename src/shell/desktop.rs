//! The desktop's media controls' share of the shell: the title that plays shown to the bar
//! widget, its poster written where the desktop reads it, and the media keys heard back.

use std::time::Instant;

use gpui_kit::{Context, Window};

use super::Shell;
use crate::images;
use crate::mpris::{self, Command, State, Status, Title, Track};

impl Shell {
    /// The playback that just began shows `title` to the desktop (a trailer has none, and shows
    /// nothing). The poster is written to the cache off the window's thread, then shown too.
    pub(super) fn publish(&mut self, title: Option<Title>, cx: &mut Context<Self>) {
        let (Some(playback), Some(Title { track, poster })) = (&mut self.playback, title) else {
            return;
        };
        let item = track.item.clone();
        playback.track = Some(track);
        let (Some(poster), Some(dir)) = (poster, self.art_dir.clone()) else {
            return;
        };
        let asked = item.clone();
        self.ask(
            cx,
            move |server| {
                if let Some(path) = mpris::cached_art(&dir, &asked, &poster.tag) {
                    return Some(path);
                }
                let bytes = server.image(&poster, images::POSTER).ok()?;
                mpris::store_art(&dir, &asked, &poster.tag, &bytes).ok()
            },
            move |shell, path, _| {
                // The viewer may have moved on to another title meanwhile.
                let track = shell.playback.as_mut().and_then(|p| p.track.as_mut());
                if let (Some(path), Some(track)) = (path, track)
                    && track.item == item
                {
                    track.art = Some(path);
                }
            },
        );
    }

    /// What plays, told to the desktop: at every tick, the position with it.
    pub(super) fn show_to_desktop(&mut self) {
        let Some(playback) = &self.playback else {
            return;
        };
        let session = &playback.session;
        let state = match &playback.track {
            None => State::default(),
            Some(track) => State {
                status: if session.paused {
                    Status::Paused
                } else {
                    Status::Playing
                },
                track: Some(Track {
                    length: if session.duration > 0. {
                        session.duration
                    } else {
                        track.length
                    },
                    ..track.clone()
                }),
                can_go_next: playback.next.is_some(),
                rate: session.speed(),
            },
        };
        self.mpris.follow(state, session.position, Instant::now());
    }

    /// What the media keys and the bar widget asked since the last tick.
    pub(super) fn follow_desktop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for command in self.mpris.commands() {
            let Some(playback) = &mut self.playback else {
                return;
            };
            let session = &mut playback.session;
            // Told as a seek when mpv lands there, however small it was.
            match command {
                Command::SeekBy(seconds) => self.mpris.sought_by(seconds, Instant::now()),
                Command::SetPosition(seconds) => self.mpris.sought_to(seconds, Instant::now()),
                _ => {}
            }
            match command {
                Command::PlayPause => session.toggle_pause(),
                Command::Play if session.paused => session.toggle_pause(),
                Command::Pause if !session.paused => session.toggle_pause(),
                Command::Play | Command::Pause => {}
                Command::SeekBy(seconds) => session.skip(seconds),
                Command::SetPosition(seconds) => session.seek_to(seconds),
                Command::Stop => self.leave_player(false, window, cx),
                Command::Next => {
                    // The next episode at once, the one left told where it stopped: Next is
                    // not the end of the episode, which stays unwatched.
                    if let Some(next) = playback.next.take() {
                        self.leave_to(Some(next), window, cx);
                        self.play_up_next_now(window, cx);
                    }
                }
            }
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bobine::provider::ItemId;
    use gpui_kit::{TestAppContext, WindowHandle};

    use super::*;
    use crate::fake::{FILM, FakeServer, SERIES};
    use crate::mpris::fake::FakeControls;
    use crate::mpris::{Publisher, file_url};
    use crate::navigation::Screen;
    use crate::player::fake::{Call, FakePlayer};
    use crate::player::session::Session;
    use crate::player::{PlayerEvent, Seek};
    use crate::series_page::Next;
    use crate::video::fake::FakeDisplay;

    const PLANTED: &str = "PLANTED-TOKEN";

    /// A shell started on a kept session, the fake server behind it, its media controls a fake.
    fn signed_in_window(
        cx: &mut TestAppContext,
    ) -> (FakeServer, WindowHandle<Shell>, FakeControls) {
        let server = FakeServer::new(6, true);
        let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server.clone());
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
        let controls = FakeControls::default();
        let fake = controls.clone();
        window
            .update(cx, |shell, _, _| {
                shell.mpris = Publisher::new(Box::new(fake));
            })
            .unwrap();
        (server, window, controls)
    }

    fn film() -> Title {
        let film = crate::fake::films(1).remove(0);
        Title::film(&film)
    }

    /// `title` playing on the shell with a fake player, `next` after it.
    fn play(
        window: &WindowHandle<Shell>,
        cx: &mut TestAppContext,
        title: Option<Title>,
        next: Option<Next>,
    ) -> FakePlayer {
        let player = FakePlayer::default();
        player.queued.borrow_mut().extend([
            PlayerEvent::Loaded,
            PlayerEvent::Duration(7860.),
            PlayerEvent::Position(10.),
        ]);
        let handles = FakePlayer {
            calls: player.calls.clone(),
            queued: player.queued.clone(),
            ..Default::default()
        };
        window
            .update(cx, |shell, window, cx| {
                let session = Session::new(
                    Box::new(handles),
                    Box::new(FakeDisplay::default()),
                    "Le Dernier Hiver".into(),
                    None,
                );
                shell.begin_playback(session, None, false, window, cx);
                shell.playback.as_mut().unwrap().next = next;
                shell.publish(title, cx);
            })
            .unwrap();
        cx.run_until_parked();
        player
    }

    fn tick(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, window, cx| shell.tick(window, cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn ask(controls: &FakeControls, command: Command) {
        controls.queued.borrow_mut().push(command);
    }

    fn current(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    #[gpui_kit::test]
    fn a_playback_publishes_its_metadata_and_its_status(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        let player = play(&window, cx, Some(film()), None);
        tick(&window, cx);
        let shown = controls.last();
        assert_eq!(shown.status, Status::Playing);
        let track = shown.track.expect("a track");
        assert_eq!(track.title, film().track.title);
        assert_eq!(track.item, FILM);
        assert_eq!(track.length, 7860., "the player's duration");
        assert!(!shown.can_go_next, "no episode after a film");
        assert_eq!(controls.positions.borrow().last(), Some(&10.));
        player.queued.borrow_mut().push(PlayerEvent::Paused(true));
        tick(&window, cx);
        assert_eq!(controls.last().status, Status::Paused);
    }

    #[gpui_kit::test]
    fn pause_from_mpris_pauses_the_player_and_the_status_follows(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        let player = play(&window, cx, Some(film()), None);
        tick(&window, cx);
        ask(&controls, Command::Pause);
        tick(&window, cx);
        assert_eq!(player.calls.borrow().last(), Some(&Call::Paused(true)));
        assert_eq!(controls.last().status, Status::Paused);
        // Pause again changes nothing; PlayPause plays.
        ask(&controls, Command::Pause);
        ask(&controls, Command::PlayPause);
        tick(&window, cx);
        let pauses: Vec<Call> = player
            .calls
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Paused(_)))
            .cloned()
            .collect();
        assert_eq!(pauses, [Call::Paused(true), Call::Paused(false)]);
        assert_eq!(controls.last().status, Status::Playing);
    }

    #[gpui_kit::test]
    fn seek_and_set_position_move_the_player(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        let player = play(&window, cx, Some(film()), None);
        tick(&window, cx);
        ask(&controls, Command::SeekBy(-10.));
        ask(&controls, Command::SetPosition(600.));
        tick(&window, cx);
        let seeks: Vec<Call> = player
            .calls
            .borrow()
            .iter()
            .filter(|call| matches!(call, Call::Seek(_)))
            .cloned()
            .collect();
        assert_eq!(
            seeks,
            [Call::Seek(Seek::By(-10.)), Call::Seek(Seek::To(600.))]
        );
        // mpv says where the film went: the desktop is told it jumped.
        player.queued.borrow_mut().push(PlayerEvent::Position(600.));
        tick(&window, cx);
        assert_eq!(*controls.seeked.borrow(), [600.]);
    }

    #[gpui_kit::test]
    fn a_small_seek_from_mpris_is_followed_by_a_seeked_signal(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        let player = play(&window, cx, Some(film()), None);
        tick(&window, cx);
        ask(&controls, Command::SeekBy(0.5));
        tick(&window, cx);
        player.queued.borrow_mut().push(PlayerEvent::Position(10.5));
        tick(&window, cx);
        assert_eq!(*controls.seeked.borrow(), [10.5]);
    }

    #[gpui_kit::test]
    fn next_plays_the_next_episode(cx: &mut TestAppContext) {
        let (server, window, controls) = signed_in_window(cx);
        let series = ItemId(SERIES.into());
        window
            .update(cx, |shell, _, cx| shell.open_series(series, cx))
            .unwrap();
        cx.run_until_parked();
        // No player starts in a test: the server refuses, and what was asked is kept.
        server.state.lock().unwrap().play_error =
            Some(bobine::provider::ProviderError::Unreachable("test".into()));
        let mut episodes = crate::fake::episodes();
        let (first, second) = (episodes.remove(0), episodes.remove(0));
        play(
            &window,
            cx,
            Some(Title::episode(&first)),
            Some(Next::Episode(second.clone())),
        );
        tick(&window, cx);
        assert!(controls.last().can_go_next);
        ask(&controls, Command::Next);
        tick(&window, cx);
        let planned = window
            .update(cx, |shell, _, _| shell.planned.clone())
            .unwrap();
        assert_eq!(
            planned.last().map(|(id, _)| id),
            Some(&second.item.id),
            "the next episode was asked for"
        );
        assert_eq!(current(&window, cx), Screen::SeriesDetail);
    }

    #[gpui_kit::test]
    fn next_does_nothing_without_a_next_episode(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        ask(&controls, Command::Next);
        tick(&window, cx);
        assert_eq!(current(&window, cx), Screen::Player);
        assert_eq!(controls.last().status, Status::Playing);
    }

    #[gpui_kit::test]
    fn the_art_url_is_a_local_file_and_holds_no_token(cx: &mut TestAppContext) {
        let (server, window, controls) = signed_in_window(cx);
        {
            let mut state = server.state.lock().unwrap();
            state.stream =
                format!("https://media.example.org/Videos/{FILM}/stream?api_key={PLANTED}");
            state.converted =
                format!("https://media.example.org/videos/{FILM}/master.m3u8?ApiKey={PLANTED}");
            state.trailer_stream = format!("https://media.example.org/trailer?api_key={PLANTED}");
        }
        let cache = tempfile::tempdir().unwrap();
        window
            .update(cx, |shell, _, _| {
                shell.art_dir = Some(cache.path().join("bobine").join("mpris"));
            })
            .unwrap();
        let title = film();
        let poster = title.poster.clone().expect("the film has a poster");
        play(&window, cx, Some(title), None);
        tick(&window, cx);
        let art = controls
            .last()
            .track
            .and_then(|track| track.art)
            .expect("the poster is shown");
        assert_eq!(art, cache.path().join("bobine/mpris/film-0.t1.bmp"));
        use bobine::provider::MediaServer;
        assert_eq!(
            std::fs::read(&art).unwrap(),
            server.image(&poster, images::POSTER).unwrap(),
            "the server's poster, as it is"
        );
        assert!(file_url(&art).starts_with("file:///"));
        for state in controls.shown.borrow().iter() {
            let printed = format!("{state:?}");
            assert!(!printed.contains(PLANTED), "{printed}");
            assert!(!printed.contains("http"), "{printed}");
        }
    }

    #[gpui_kit::test]
    fn the_harness_keeps_the_desktop_off(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        let cache = tempfile::tempdir().unwrap();
        window
            .update(cx, |shell, _, _| {
                shell.art_dir = Some(cache.path().join("bobine").join("mpris"));
                shell.without_desktop();
            })
            .unwrap();
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        assert!(
            controls.shown.borrow().is_empty(),
            "nothing is shown to the desktop"
        );
        assert!(
            !cache.path().join("bobine").exists(),
            "no poster is written to the cache"
        );
    }

    /// The server's requests for `title`'s poster so far.
    fn poster_requests(server: &FakeServer, title: &Title) -> usize {
        let poster = title.poster.clone().expect("the film has a poster");
        let state = server.state.lock().unwrap();
        state
            .record
            .images
            .iter()
            .filter(|(image, width)| *image == poster && *width == images::POSTER)
            .count()
    }

    #[gpui_kit::test]
    fn the_poster_already_on_disk_is_not_fetched_again(cx: &mut TestAppContext) {
        let (server, window, controls) = signed_in_window(cx);
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join("bobine").join("mpris");
        window
            .update(cx, |shell, _, _| shell.art_dir = Some(dir.clone()))
            .unwrap();
        // The home page may ask for the same poster: only what playing asks is counted.
        let before = poster_requests(&server, &film());
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        assert_eq!(poster_requests(&server, &film()), before + 1);
        let first = controls.last().track.and_then(|t| t.art).expect("a poster");
        // The same film, played again.
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        assert_eq!(
            poster_requests(&server, &film()),
            before + 1,
            "no second request"
        );
        let again = controls.last().track.and_then(|t| t.art).expect("a poster");
        assert_eq!(again, first);
    }

    #[gpui_kit::test]
    fn a_new_poster_of_the_item_is_fetched_and_replaces_the_old_file(cx: &mut TestAppContext) {
        let (server, window, controls) = signed_in_window(cx);
        let cache = tempfile::tempdir().unwrap();
        let dir = cache.path().join("bobine").join("mpris");
        window
            .update(cx, |shell, _, _| shell.art_dir = Some(dir.clone()))
            .unwrap();
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        let old = controls.last().track.and_then(|t| t.art).expect("a poster");
        let mut renewed = film();
        renewed.poster.as_mut().unwrap().tag = "t2".into();
        play(&window, cx, Some(renewed.clone()), None);
        tick(&window, cx);
        assert_eq!(
            poster_requests(&server, &renewed),
            1,
            "the new tag is asked for"
        );
        let new = controls.last().track.and_then(|t| t.art).expect("a poster");
        assert_ne!(new, old);
        assert!(new.to_string_lossy().contains(".t2."), "{new:?}");
        assert!(new.exists());
        assert!(!old.exists(), "the item's older poster is replaced");
    }

    #[gpui_kit::test]
    fn stopping_the_film_sets_it_to_stopped(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        assert_eq!(controls.last().status, Status::Playing);
        window
            .update(cx, |shell, window, cx| shell.player_back(window, cx))
            .unwrap();
        assert_eq!(controls.last(), State::default());
    }

    #[gpui_kit::test]
    fn stop_from_mpris_leaves_the_player(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        play(&window, cx, Some(film()), None);
        tick(&window, cx);
        ask(&controls, Command::Stop);
        tick(&window, cx);
        assert_ne!(current(&window, cx), Screen::Player);
        assert_eq!(controls.last().status, Status::Stopped);
    }

    #[gpui_kit::test]
    fn the_end_of_the_film_sets_it_to_stopped(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        let player = play(&window, cx, Some(film()), None);
        tick(&window, cx);
        assert_eq!(controls.last().status, Status::Playing);
        player.queued.borrow_mut().push(PlayerEvent::EndOfFile);
        tick(&window, cx);
        assert_eq!(controls.last().status, Status::Stopped);
    }

    #[gpui_kit::test]
    fn a_command_heard_while_nothing_played_is_not_applied_to_the_next_film(
        cx: &mut TestAppContext,
    ) {
        let (_server, window, controls) = signed_in_window(cx);
        ask(&controls, Command::Pause);
        let player = play(&window, cx, Some(film()), None);
        tick(&window, cx);
        assert!(!player.calls.borrow().contains(&Call::Paused(true)));
    }

    #[gpui_kit::test]
    fn closing_the_window_releases_the_controls(cx: &mut TestAppContext) {
        let (_server, window, controls) = signed_in_window(cx);
        play(&window, cx, Some(film()), None);
        window
            .update(cx, |shell, _, _| shell.close_video())
            .unwrap();
        assert!(*controls.closed.borrow());
    }
}
