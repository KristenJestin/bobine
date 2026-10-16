//! The trailer button's share of the shell: the page on top (a film or a series) plays its
//! trailer in the player. A trailer is no watching: nothing is reported to the server, and no
//! resume point, up-next card, quality cap or remembered language is touched.

use bobine::provider::{ProviderError, StreamUrl};
use gpui_kit::{Context, Window};

use super::pages::{Loadable, Origin};
use super::{Film as Playing, Shell};
use crate::navigation::Screen;
use crate::trailer::{self, Trailer};

impl Shell {
    /// "Trailer": plays the trailer of the page on top. A trailer file of the server is asked
    /// for off the window's thread; one hosted elsewhere goes straight to the player.
    pub fn play_trailer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.server.clone() else {
            return;
        };
        let ytdlp = self.ytdlp;
        let (origin, item, title, trailer) = if self.navigator.current() == Screen::SeriesDetail {
            let depth = self.series_pages.len().wrapping_sub(1);
            let Some(page) = self.series_pages.last_mut() else {
                return;
            };
            let Loadable::Ready(detail) = &page.detail else {
                return;
            };
            let Some(trailer) = trailer::choose(&detail.trailers, ytdlp) else {
                return;
            };
            if page.busy {
                return;
            }
            page.busy = true;
            page.error = None;
            let title = detail.series.title.clone();
            (
                Origin::Series(depth, page.id.clone()),
                page.id.clone(),
                title,
                trailer,
            )
        } else {
            let depth = self.details.len().wrapping_sub(1);
            let Some(page) = self.details.last_mut() else {
                return;
            };
            let Loadable::Ready(detail) = &page.detail else {
                return;
            };
            let Some(trailer) = trailer::choose(&detail.trailers, ytdlp) else {
                return;
            };
            if page.busy {
                return;
            }
            page.busy = true;
            page.error = None;
            let title = detail.film.title.clone();
            (
                Origin::Film(depth, page.id.clone()),
                page.id.clone(),
                title,
                trailer,
            )
        };
        let playing = Playing {
            title,
            subtitle: Some("Trailer".into()),
            // Not a title the viewer can put in their list from here.
            list: None,
        };
        match trailer {
            Trailer::Remote(address) => {
                self.start_trailer(
                    origin,
                    Ok(StreamUrl::new(address)),
                    true,
                    playing,
                    window,
                    cx,
                );
            }
            Trailer::Local => {
                let task = cx
                    .background_executor()
                    .spawn(async move { server.local_trailer(&item) });
                cx.spawn_in(window, async move |this, cx| {
                    let stream = task.await;
                    this.update_in(cx, |shell, window, cx| {
                        shell.start_trailer(origin, stream, false, playing, window, cx);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// The trailer's address is known: the player starts, with no reports and no film around
    /// it. `ytdl`: the address is a page for the `yt-dlp` hook to resolve.
    fn start_trailer(
        &mut self,
        origin: Origin,
        stream: Result<StreamUrl, ProviderError>,
        ytdl: bool,
        playing: Playing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // An answer that took its time: the viewer has moved on, maybe to a film, which a
        // trailer must not replace.
        if self.playback.is_some() || !self.on_top(&origin) {
            self.tried(&origin, None, cx);
            return;
        }
        let said = match stream {
            Ok(stream) => {
                self.tried(&origin, None, cx);
                match self.open_trailer(&stream, ytdl, &playing, window, cx) {
                    Ok(()) => return,
                    Err(error) => {
                        // The words of the video surface or of mpv's error code: no address in
                        // them.
                        eprintln!("bobine: the trailer does not play: {error}");
                        "Cannot play the trailer."
                    }
                }
            }
            Err(error) => self.failed(error, cx),
        };
        self.tried(&origin, Some(said), cx);
    }

    /// The page a trailer was asked from is still the one shown.
    fn on_top(&self, origin: &Origin) -> bool {
        match origin {
            Origin::Film(depth, id) => {
                self.navigator.current() == Screen::FilmDetail
                    && self.details.len() == depth + 1
                    && self.details.get(*depth).is_some_and(|page| &page.id == id)
            }
            Origin::Series(depth, id) => {
                self.navigator.current() == Screen::SeriesDetail
                    && self.series_pages.len() == depth + 1
                    && self
                        .series_pages
                        .get(*depth)
                        .is_some_and(|page| &page.id == id)
            }
            Origin::Home => false,
        }
    }

    /// What makes a trailer no watching, for the player and for the tests alike: its own mpv
    /// options, and a playback with nothing to report to, no conversion or quality to fall
    /// back on, and no next episode.
    fn open_trailer(
        &mut self,
        stream: &StreamUrl,
        ytdl: bool,
        playing: &Playing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let options = trailer::mpv_options(&self.mpv_options, ytdl);
        let session = self.trailer_session(stream, options, playing, window)?;
        self.begin_playback(session, None, window, cx);
        Ok(())
    }

    #[cfg(not(test))]
    fn trailer_session(
        &mut self,
        stream: &StreamUrl,
        options: Vec<(&'static str, &'static str)>,
        playing: &Playing,
        window: &mut Window,
    ) -> Result<crate::player::session::Session, String> {
        use crate::player::libmpv::Libmpv;
        use crate::player::{FirstSubtitle, Media, Stream};

        let mpv = Libmpv::new(&options).map_err(|error| error.to_string())?;
        let media = Media {
            stream: Stream {
                url: stream.expose_for_player(),
                headers: &[],
            },
            start: 0.,
            audio: None,
            subtitle: FirstSubtitle::Default,
            external_subtitles: &[],
        };
        let playing = Playing {
            title: playing.title.clone(),
            subtitle: playing.subtitle.clone(),
            list: None,
        };
        self.open_session(std::sync::Arc::new(mpv), media, playing, window)
    }

    /// No player starts in a test: what it would have been given is kept, and a fake player
    /// stands in for it.
    #[cfg(test)]
    fn trailer_session(
        &mut self,
        stream: &StreamUrl,
        options: Vec<(&'static str, &'static str)>,
        playing: &Playing,
        _window: &mut Window,
    ) -> Result<crate::player::session::Session, String> {
        use crate::player::fake::FakePlayer;
        use crate::player::session::Session;
        use crate::video::fake::FakeDisplay;

        self.trailers_opened
            .push((stream.expose_for_player().to_string(), options));
        Ok(Session::new(
            Box::new(FakePlayer::default()),
            Box::new(FakeDisplay::default()),
            playing.title.clone(),
            playing.subtitle.clone(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bobine::provider::{ItemId, Trailers};
    use gpui_kit::TestAppContext;

    use super::*;
    use crate::fake::{FILM, FakeServer, SERIES};
    use crate::navigation::Screen;
    use crate::screens::{FILM_TRAILER, SERIES_TRAILER};

    const ADDRESS: &str = "https://video.example/watch?v=abc";

    /// A shell started on a kept session, the fake server behind it.
    fn signed_in_window(
        trailers: Trailers,
        ytdlp: bool,
        cx: &mut TestAppContext,
    ) -> (FakeServer, gpui_kit::WindowHandle<Shell>) {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().trailers = trailers;
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
        window
            .update(cx, |shell, _, _| shell.ytdlp = ytdlp)
            .unwrap();
        (server, window)
    }

    fn local() -> Trailers {
        Trailers {
            local: true,
            remote: None,
        }
    }

    fn remote() -> Trailers {
        Trailers {
            local: false,
            remote: Some(ADDRESS.into()),
        }
    }

    fn on_film(window: &gpui_kit::WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| shell.open_film(ItemId(FILM.into()), cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn on_series(window: &gpui_kit::WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| {
                shell.open_series(ItemId(SERIES.into()), cx)
            })
            .unwrap();
        cx.run_until_parked();
    }

    fn press_trailer(window: &gpui_kit::WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, window, cx| shell.play_trailer(window, cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn current(window: &gpui_kit::WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    /// Each address the player was given, with whether its mpv had the `yt-dlp` hook on.
    fn opened(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> Vec<(String, bool)> {
        window
            .update(cx, |shell, _, _| {
                shell
                    .trailers_opened
                    .iter()
                    .map(|(address, options)| (address.clone(), options.contains(&("ytdl", "yes"))))
                    .collect()
            })
            .unwrap()
    }

    #[gpui_kit::test]
    fn a_local_trailer_plays(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        server.state.lock().unwrap().trailer_stream = "file:///nowhere-trailer.mp4".into();
        on_film(&window, cx);
        press_trailer(&window, cx);
        assert_eq!(
            server.state.lock().unwrap().record.trailer_asks,
            [ItemId(FILM.into())]
        );
        assert_eq!(
            opened(&window, cx),
            [("file:///nowhere-trailer.mp4".to_string(), false)],
            "the server's file, without the yt-dlp hook"
        );
        assert_eq!(current(&window, cx), Screen::Player);
    }

    #[gpui_kit::test]
    fn a_remote_trailer_plays_when_ytdlp_is_present(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(remote(), true, cx);
        on_film(&window, cx);
        press_trailer(&window, cx);
        assert_eq!(opened(&window, cx), [(ADDRESS.to_string(), true)]);
        assert!(server.state.lock().unwrap().record.trailer_asks.is_empty());
        assert_eq!(current(&window, cx), Screen::Player);
    }

    #[gpui_kit::test]
    fn a_remote_trailer_does_not_play_when_ytdlp_is_missing(cx: &mut TestAppContext) {
        let (_server, window) = signed_in_window(remote(), false, cx);
        on_film(&window, cx);
        press_trailer(&window, cx);
        assert!(opened(&window, cx).is_empty());
        assert_eq!(current(&window, cx), Screen::FilmDetail);
    }

    #[gpui_kit::test]
    fn a_title_without_a_trailer_plays_none(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(Trailers::default(), true, cx);
        on_film(&window, cx);
        press_trailer(&window, cx);
        assert!(opened(&window, cx).is_empty());
        assert!(server.state.lock().unwrap().record.trailer_asks.is_empty());
        assert_eq!(current(&window, cx), Screen::FilmDetail);
    }

    #[gpui_kit::test]
    fn the_local_trailer_plays_before_a_remote_one(cx: &mut TestAppContext) {
        let both = Trailers {
            local: true,
            remote: Some(ADDRESS.into()),
        };
        let (_server, window) = signed_in_window(both, true, cx);
        on_film(&window, cx);
        press_trailer(&window, cx);
        assert_eq!(
            opened(&window, cx),
            [("file:///nowhere-trailer.mp4".to_string(), false)]
        );
    }

    #[gpui_kit::test]
    fn a_series_plays_its_trailer_too(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        on_series(&window, cx);
        press_trailer(&window, cx);
        assert_eq!(
            server.state.lock().unwrap().record.trailer_asks,
            [ItemId(SERIES.into())]
        );
        assert_eq!(opened(&window, cx).len(), 1);
        assert_eq!(current(&window, cx), Screen::Player);
    }

    #[gpui_kit::test]
    fn no_progress_is_reported_for_a_trailer(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        press_trailer(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                let playback = shell.playback.as_ref().expect("the trailer plays");
                assert!(playback.reporting.is_none(), "nothing to report to");
                assert!(playback.fallback.is_none(), "no conversion to fall back on");
                assert!(
                    !playback.session.quality_offered(),
                    "and no default quality applied"
                );
                // Long enough for any progress to be due.
                for _ in 0..3 {
                    shell.tick(window, cx);
                }
                shell.leave_player(false, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let state = server.state.lock().unwrap();
        assert!(state.record.reports.is_empty());
        assert!(
            state.record.plays.is_empty(),
            "no playback session was opened"
        );
        assert!(
            state.record.watched.is_empty(),
            "and nothing is marked watched"
        );
    }

    #[gpui_kit::test]
    fn a_trailer_leaves_the_resume_the_up_next_and_the_quality_alone(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        let pages = server.state.lock().unwrap().record.film_pages.len();
        press_trailer(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                let playback = shell.playback.as_ref().expect("the trailer plays");
                assert!(playback.fallback.is_none(), "no conversion, no quality");
                assert!(playback.next.is_none(), "no next episode");
                assert!(playback.list.is_none(), "no My list button on a trailer");
                shell.leave_player(true, window, cx);
                assert!(shell.up_next.episode().is_none(), "no card at the end");
            })
            .unwrap();
        cx.run_until_parked();
        let state = server.state.lock().unwrap();
        assert!(state.record.transcodes.is_empty());
        assert_eq!(
            state.record.segment_asks,
            [],
            "no skip button for a trailer"
        );
        assert!(state.record.tiles.is_empty(), "no trickplay either");
        assert_eq!(
            state.record.film_pages.len(),
            pages,
            "the page is not read again as after a film"
        );
    }

    #[gpui_kit::test]
    fn leaving_a_trailer_returns_to_the_film_page(cx: &mut TestAppContext) {
        let (_server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        press_trailer(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                shell.leave_player(false, window, cx)
            })
            .unwrap();
        assert_eq!(current(&window, cx), Screen::FilmDetail);
        window
            .update(cx, |shell, _, _| assert!(shell.playback.is_none()))
            .unwrap();
    }

    #[gpui_kit::test]
    fn escape_in_the_player_returns_to_the_series_page(cx: &mut TestAppContext) {
        let (_server, window) = signed_in_window(local(), false, cx);
        on_series(&window, cx);
        press_trailer(&window, cx);
        let mut visual = gpui_kit::VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        assert_eq!(current(&window, cx), Screen::SeriesDetail);
    }

    #[gpui_kit::test]
    fn a_trailer_the_server_cannot_give_says_so_and_stays_on_the_page(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        server.state.lock().unwrap().trailer_error =
            Some(ProviderError::Unreachable("down".into()));
        on_film(&window, cx);
        press_trailer(&window, cx);
        assert_eq!(current(&window, cx), Screen::FilmDetail);
        window
            .update(cx, |shell, _, _| {
                let page = shell.details.last().unwrap();
                assert!(page.error.is_some(), "the page says what went wrong");
                assert!(!page.busy, "and can be pressed again");
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn pressing_trailer_twice_asks_the_server_once(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                shell.play_trailer(window, cx);
                shell.play_trailer(window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(server.state.lock().unwrap().record.trailer_asks.len(), 1);
    }

    /// The page opened and drawn, tall enough to show all of it.
    fn drawn_page(
        window: &gpui_kit::WindowHandle<Shell>,
        cx: &mut TestAppContext,
    ) -> gpui_kit::VisualTestContext {
        let visual = gpui_kit::VisualTestContext::from_window((*window).into(), cx);
        visual.simulate_resize(gpui_kit::size(gpui_kit::px(1600.), gpui_kit::px(3000.)));
        visual.run_until_parked();
        visual
    }

    fn click_on(visual: &mut gpui_kit::VisualTestContext, what: &'static str) {
        let at = visual
            .debug_bounds(what)
            .unwrap_or_else(|| panic!("{what} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, gpui_kit::Modifiers::none());
        visual.simulate_click(at, gpui_kit::Modifiers::none());
        visual.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_film_with_a_trailer_has_a_button_that_plays_it(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        let mut visual = drawn_page(&window, cx);
        click_on(&mut visual, FILM_TRAILER);
        assert_eq!(
            server.state.lock().unwrap().record.trailer_asks,
            [ItemId(FILM.into())]
        );
        assert_eq!(current(&window, cx), Screen::Player);
    }

    #[gpui_kit::test]
    fn a_series_with_a_trailer_has_a_button_that_plays_it(cx: &mut TestAppContext) {
        let (server, window) = signed_in_window(remote(), true, cx);
        on_series(&window, cx);
        let mut visual = drawn_page(&window, cx);
        click_on(&mut visual, SERIES_TRAILER);
        assert_eq!(opened(&window, cx), [(ADDRESS.to_string(), true)]);
        assert!(server.state.lock().unwrap().record.trailer_asks.is_empty());
        assert_eq!(current(&window, cx), Screen::Player);
    }

    #[gpui_kit::test]
    fn there_is_no_button_when_there_is_no_trailer(cx: &mut TestAppContext) {
        let (_server, window) = signed_in_window(Trailers::default(), true, cx);
        on_film(&window, cx);
        let mut visual = drawn_page(&window, cx);
        assert!(
            visual.debug_bounds("cast-member 0").is_some(),
            "the page is drawn"
        );
        assert!(visual.debug_bounds(FILM_TRAILER).is_none());
        window
            .update(cx, |shell, _, cx| {
                shell.navigator.back();
                shell.open_series(ItemId(SERIES.into()), cx);
            })
            .unwrap();
        let mut visual = drawn_page(&window, cx);
        assert!(
            visual.debug_bounds("cast-member 0").is_some(),
            "the page is drawn"
        );
        assert!(visual.debug_bounds(SERIES_TRAILER).is_none());
    }

    #[gpui_kit::test]
    fn there_is_no_button_when_only_a_remote_trailer_exists_and_ytdlp_is_missing(
        cx: &mut TestAppContext,
    ) {
        let (_server, window) = signed_in_window(remote(), false, cx);
        on_film(&window, cx);
        let mut visual = drawn_page(&window, cx);
        assert!(
            visual.debug_bounds("cast-member 0").is_some(),
            "the page is drawn"
        );
        assert!(visual.debug_bounds(FILM_TRAILER).is_none());
        window
            .update(cx, |shell, _, cx| {
                shell.navigator.back();
                shell.open_series(ItemId(SERIES.into()), cx);
            })
            .unwrap();
        let mut visual = drawn_page(&window, cx);
        assert!(visual.debug_bounds(SERIES_TRAILER).is_none());
    }

    #[gpui_kit::test]
    fn a_late_trailer_answer_does_not_replace_the_film_that_plays(cx: &mut TestAppContext) {
        use crate::player::PlayerEvent;
        use crate::player::fake::FakePlayer;
        use crate::player::session::Session;
        use crate::reports::Reporting;
        use crate::video::fake::FakeDisplay;
        use bobine::provider::{MediaSourceId, PlayMethod, PlaySessionId, PlaybackReport};

        use crate::fake::Report;

        let (server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        // The server's answer arrives only once the window parks: everything below happens
        // while it is on its way.
        window
            .update(cx, |shell, window, cx| {
                shell.play_trailer(window, cx);
                shell.navigator.back();
                let player = FakePlayer::default();
                player.queued.borrow_mut().push(PlayerEvent::Loaded);
                let session = Session::new(
                    Box::new(player),
                    Box::new(FakeDisplay::default()),
                    "Le Dernier Hiver".into(),
                    None,
                );
                let reporting = Reporting::start(
                    Arc::new(server.clone()),
                    PlaybackReport {
                        session: PlaySessionId("session-1".into()),
                        film: ItemId(FILM.into()),
                        source: MediaSourceId("source-4k".into()),
                        position: std::time::Duration::ZERO,
                        paused: false,
                        audio_track: None,
                        subtitle_track: None,
                        method: PlayMethod::DirectPlay,
                    },
                );
                shell.begin_playback(session, Some(reporting), window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(
            server.state.lock().unwrap().record.trailer_asks,
            [ItemId(FILM.into())],
            "the answer came"
        );
        assert!(opened(&window, cx).is_empty(), "and opened nothing");
        window
            .update(cx, |shell, window, cx| {
                let playback = shell.playback.as_ref().expect("the film still plays");
                assert_eq!(playback.session.title, "Le Dernier Hiver");
                assert!(playback.reporting.is_some(), "the film's reports go on");
                shell.tick(window, cx);
                shell.leave_player(false, window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        let reports = server.reports();
        assert!(
            matches!(reports.last(), Some(Report::Stopped(report)) if report.film.0 == FILM),
            "the film was told stopped: {reports:?}"
        );
    }

    #[gpui_kit::test]
    fn a_trailer_answer_for_a_page_left_opens_nothing(cx: &mut TestAppContext) {
        let (_server, window) = signed_in_window(local(), false, cx);
        on_film(&window, cx);
        window
            .update(cx, |shell, window, cx| {
                shell.play_trailer(window, cx);
                shell.navigator.back();
            })
            .unwrap();
        cx.run_until_parked();
        assert!(opened(&window, cx).is_empty());
        assert_ne!(current(&window, cx), Screen::Player);
        window
            .update(cx, |shell, _, _| assert!(shell.playback.is_none()))
            .unwrap();
    }
}
