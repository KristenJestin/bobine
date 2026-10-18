//! What the launcher's commands ask the window (see `instance`): a page, or a title to play.

use bobine::provider::{ItemId, Title};
use gpui_kit::{Context, Window};

use super::Shell;
use crate::film_page::From;
use crate::home_page::{self, PlayTarget};
use crate::instance::{Inbox, Kind, Request};
use crate::navigation::Screen;

impl Shell {
    /// The window hears the commands on `inbox` (`None`: they cannot reach it), and does
    /// `first`, the command it was opened for, once a session is there.
    pub fn hear_commands(&mut self, inbox: Option<Inbox>, first: Option<Request>) {
        self.commands = inbox;
        self.request_waiting = first;
    }

    /// From the first draw on, as the window is needed to play; and the request kept for the
    /// session, once it is there.
    pub(super) fn listen_to_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commands_heard.is_none()
            && let Some(inbox) = self.commands.clone()
        {
            self.commands_heard = Some(cx.spawn_in(window, async move |this, cx| {
                loop {
                    let request = inbox.next().await;
                    if this
                        .update_in(cx, |shell, window, cx| shell.asked(request, window, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
        }
        if self.session_ready()
            && let Some(request) = self.request_waiting.take()
        {
            cx.spawn_in(window, async move |this, cx| {
                this.update_in(cx, |shell, window, cx| shell.asked(request, window, cx))
                    .ok();
            })
            .detach();
        }
    }

    fn session_ready(&self) -> bool {
        self.server.is_some() && self.navigator.current() != Screen::Login
    }

    /// The window is raised, and shows the page or plays the title. Before a session is there
    /// (the kept one being restored, or the login), the request waits for it; while a film
    /// plays, it goes on, and the request is dropped.
    pub fn asked(&mut self, request: Request, window: &mut Window, cx: &mut Context<Self>) {
        // On Wayland the compositor decides: it may only mark the window as asking for attention.
        window.activate_window();
        if !self.session_ready() {
            self.request_waiting = Some(request);
            return;
        }
        if self.playback.is_some() {
            return;
        }
        self.search = None;
        match request {
            Request::Open(Kind::Movie, id) => self.open_film(id, cx),
            Request::Open(Kind::Series, id) => self.open_series(id, cx),
            Request::Open(Kind::Episode, id) => self.open_episode_page(id, cx),
            Request::Play(id, from) => self.play_asked(id, from, window, cx),
        }
        cx.notify();
    }

    /// An episode has no page: its series has, on its season, the episode in view.
    fn open_episode_page(&mut self, id: ItemId, cx: &mut Context<Self>) {
        self.ask(
            cx,
            move |server| server.title(&id),
            |shell, title, cx| match title {
                Ok(Title::Episode(episode)) => {
                    shell.open_series_on(episode.series, Some(episode.season), cx);
                    if let Some(page) = shell.series_pages.last_mut() {
                        page.in_view.replace(Some((episode.item.id, 0)));
                    }
                }
                Ok(title) => shell.open_title(&title, cx),
                Err(error) => {
                    let said = shell.failed(error, cx);
                    shell.show_notice(said, cx);
                }
            },
        );
    }

    /// Like a "Play" of the Home: the server says what the id is, then opens its playback.
    fn play_asked(&mut self, id: ItemId, from: From, window: &mut Window, cx: &mut Context<Self>) {
        if self.home.busy {
            return;
        }
        let Some(server) = self.server.clone() else {
            return;
        };
        self.home.busy = true;
        self.notify_hero(cx);
        let task = cx.background_executor().spawn(async move {
            let title = server.title(&id)?;
            home_page::start_from(&*server, &PlayTarget::of(&title), from)
        });
        cx.spawn_in(window, async move |this, cx| {
            let started = task.await;
            this.update_in(cx, |shell, window, cx| {
                shell.home_started(started, window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use bobine::provider::{Connector, ItemId};
    use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::fake::{FakeServer, MemoryKeyring};

    /// A window on `server`, hearing the commands, opened for `first`.
    fn start(
        server: &FakeServer,
        first: Option<Request>,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Shell>, Inbox, VisualTestContext) {
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let inbox = Inbox::default();
        let heard = inbox.clone();
        let window = cx.add_window(|_, cx| {
            let mut shell = Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(MemoryKeyring::default()),
                cx,
            );
            shell.hear_commands(Some(heard), first);
            shell
        });
        let visual = VisualTestContext::from_window(*window, cx);
        visual.run_until_parked();
        (window, inbox, visual)
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    fn planned(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Vec<(ItemId, Duration)> {
        window
            .update(cx, |shell, _, _| shell.planned.clone())
            .unwrap()
    }

    #[gpui_kit::test]
    fn open_movie_with_a_window_running_shows_that_films_page_in_it(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, inbox, visual) = start(&server, None, cx);
        assert_eq!(screen(&window, cx), Screen::Home);
        inbox.push(Request::Open(Kind::Movie, ItemId("film-4".into())));
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.details.last().unwrap().id, ItemId("film-4".into()));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn open_series_and_episode_show_the_series_page_on_the_episode(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, inbox, visual) = start(&server, None, cx);
        inbox.push(Request::Open(Kind::Series, ItemId("series-3".into())));
        visual.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.navigator.current(), Screen::SeriesDetail);
                assert_eq!(
                    shell.series_pages.last().unwrap().id,
                    ItemId("series-3".into())
                );
            })
            .unwrap();
        inbox.push(Request::Open(
            Kind::Episode,
            ItemId("series-0-s3-e2".into()),
        ));
        visual.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                let page = shell.series_pages.last().unwrap();
                assert_eq!(page.id, ItemId("series-0".into()));
                assert_eq!(page.seasons[page.season].id, ItemId("series-0-s3".into()));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn play_with_no_window_running_opens_one_that_plays_it(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        // No player starts in a test: the server refuses, and what was asked is kept.
        server.state.lock().unwrap().play_error =
            Some(bobine::provider::ProviderError::Unreachable("test".into()));
        let first = Request::Play(ItemId("film-0".into()), From::Resume);
        let (window, _inbox, _visual) = start(&server, Some(first), cx);
        assert_eq!(
            planned(&window, cx),
            [(ItemId("film-0".into()), Duration::from_secs(72 * 60))]
        );
    }

    #[gpui_kit::test]
    fn play_from_the_start_and_play_a_series_start_where_asked(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().play_error =
            Some(bobine::provider::ProviderError::Unreachable("test".into()));
        let (window, inbox, visual) = start(&server, None, cx);
        inbox.push(Request::Play(ItemId("film-0".into()), From::Beginning));
        visual.run_until_parked();
        // The series' next episode: the one stopped at 30 min.
        inbox.push(Request::Play(ItemId("series-0".into()), From::Resume));
        visual.run_until_parked();
        assert_eq!(
            planned(&window, cx),
            [
                (ItemId("film-0".into()), Duration::ZERO),
                (
                    ItemId("series-0-s2-e4".into()),
                    Duration::from_secs(30 * 60)
                ),
            ]
        );
    }

    #[gpui_kit::test]
    fn a_request_before_the_sign_in_waits_for_it(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, false);
        let first = Request::Open(Kind::Movie, ItemId("film-4".into()));
        let (window, _inbox, visual) = start(&server, Some(first), cx);
        assert_eq!(screen(&window, cx), Screen::Login);
        let session = server.login(
            "https://media.example.org",
            crate::fake::USER,
            crate::fake::PASSWORD,
        );
        window
            .update(cx, |shell, _, cx| shell.signed_in(session.unwrap(), cx))
            .unwrap();
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
    }
}
