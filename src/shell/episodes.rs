//! The new episodes' share of the shell: the server checked at sign-in and every quarter of an
//! hour, what is new told to the desktop, and a click on it opening the episode.

use std::path::PathBuf;

use bobine::provider::MediaServer;
use gpui_kit::{Context, Window};

use super::Shell;
use crate::images;
use crate::mpris;
use crate::navigation::Screen;
use crate::notify::{self, Gained, Notification, Open};

/// Whose checks these are: the server and the user, by their ids.
fn account(server: &dyn MediaServer) -> String {
    format!("{}/{}", server.server().id, server.user_id())
}

impl Shell {
    /// The account signed in now, if any.
    fn account(&self) -> Option<String> {
        self.server.as_deref().map(account)
    }

    /// Checks now, then every quarter of an hour, for the account signed in, while the switch is
    /// on and the desktop is told anything. Any check going on is dropped first.
    pub(super) fn start_episode_checks(&mut self, cx: &mut Context<Self>) {
        self.episode_checks = None;
        if self.notifier.is_none() || !self.settings.notify_new_episodes || self.server.is_none() {
            return;
        }
        self.episode_checks = Some(cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |shell, cx| shell.check_new_episodes(cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(notify::EVERY).await;
            }
        }));
    }

    /// Asks the server what came in since the last check, and the posters of the series that
    /// gained an episode, written to the cache.
    fn check_new_episodes(&mut self, cx: &mut Context<Self>) {
        let Some(asked) = self.account() else {
            return;
        };
        let seen = self.settings.new_episodes_seen.get(&asked).cloned();
        let dir = self.art_dir.clone();
        let account = asked.clone();
        self.ask(
            cx,
            move |server| {
                let checked = notify::check(server, seen.as_ref())?;
                let told = checked
                    .gained
                    .into_iter()
                    .map(|gained| {
                        let icon = dir.as_deref().and_then(|dir| poster(server, dir, &gained));
                        notification(gained, icon, &account)
                    })
                    .collect::<Vec<_>>();
                Ok::<_, bobine::provider::ProviderError>((checked.seen, told))
            },
            move |shell, checked, _| {
                // A failed check says nothing: the next one, a quarter of an hour later, asks
                // again from the same place.
                let Ok((seen, told)) = checked else {
                    return;
                };
                // Signed out, or someone else signed in, or the switch turned off meanwhile.
                if shell.account().as_ref() != Some(&asked)
                    || shell.episode_checks.is_none()
                    || !shell.settings.notify_new_episodes
                {
                    return;
                }
                if shell.settings.new_episodes_seen.get(&asked) != Some(&seen) {
                    shell.settings.new_episodes_seen.insert(asked, seen);
                    shell.keep_settings();
                }
                if let Some(notifier) = &mut shell.notifier {
                    for notification in &told {
                        notifier.notify(notification);
                    }
                }
            },
        );
    }

    /// The Settings' switch: off, nothing is asked nor told; on again, the checks start afresh
    /// from what the server holds now, so that what came in meanwhile does not flood the desktop.
    pub fn toggle_new_episodes(&mut self, cx: &mut Context<Self>) {
        self.settings.notify_new_episodes = !self.settings.notify_new_episodes;
        if self.settings.notify_new_episodes {
            if let Some(account) = self.account() {
                self.settings.new_episodes_seen.remove(&account);
            }
            self.start_episode_checks(cx);
        } else {
            self.episode_checks = None;
        }
        self.keep_settings();
        cx.notify();
    }

    /// The clicks on the notifications, heard from the first draw on: the window is needed to
    /// raise it.
    pub(super) fn listen_to_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notification_clicks.is_some() {
            return;
        }
        let Some(notifier) = &self.notifier else {
            return;
        };
        let clicks = notifier.clicks();
        self.notification_clicks = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                let open = clicks.next().await;
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.open_notified(open, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// A click on a notification: the window is raised, on the episode's series page, the
    /// episode in view. Nothing starts playing; a film playing goes on, the window only raised.
    pub fn open_notified(&mut self, open: Open, window: &mut Window, cx: &mut Context<Self>) {
        // On Wayland the compositor decides: it may only mark the window as asking for attention.
        window.activate_window();
        if self.account().as_ref() != Some(&open.account)
            || self.playback.is_some()
            || self.navigator.current() == Screen::Login
        {
            return;
        }
        self.search = None;
        self.open_series_on(open.series, Some(open.season), cx);
        if let Some(page) = self.series_pages.last_mut() {
            page.in_view.replace(Some(open.episode));
        }
    }
}

/// The series' poster in the cache, written there first when it is not yet.
fn poster(server: &dyn MediaServer, dir: &std::path::Path, gained: &Gained) -> Option<PathBuf> {
    let poster = gained.poster.as_ref()?;
    let item = &poster.item.0;
    if let Some(path) = mpris::cached_art(dir, item, &poster.tag) {
        return Some(path);
    }
    let bytes = server.image(poster, images::POSTER).ok()?;
    mpris::store_art(dir, item, &poster.tag, &bytes).ok()
}

fn notification(gained: Gained, icon: Option<PathBuf>, account: &str) -> Notification {
    Notification {
        summary: gained.series_title,
        body: gained.body,
        icon,
        open: Open {
            account: account.to_string(),
            series: gained.series,
            season: gained.season,
            episode: gained.episode,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use bobine::provider::{Connector, ItemId, TokenStore};
    use gpui_kit::{TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::config::Settings;
    use crate::fake::{FakeServer, MemoryKeyring, PASSWORD, USER};
    use crate::notify::fake::FakeNotifier;

    const ACCOUNT: &str = "server-1/user-1";

    /// A shell on a kept session of `server`, its settings at `path`, telling a fake notifier.
    fn start(
        server: &FakeServer,
        path: Option<PathBuf>,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Shell>, FakeNotifier) {
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let secrets: Arc<dyn TokenStore> = Arc::new(MemoryKeyring::default());
        let notifier = FakeNotifier::default();
        let told = notifier.clone();
        let window = cx.add_window(|_, cx| {
            let mut shell = Shell::new(None, Box::new(move || Ok(connector)), path, secrets, cx);
            shell.notifier = Some(Box::new(told));
            shell
        });
        cx.run_until_parked();
        // Off the Home, whose header turns every 33 ms: a quarter of an hour of it is long.
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        cx.run_until_parked();
        (window, notifier)
    }

    /// The server adds `episode` at `at`, ticks since 1970.
    fn add(server: &FakeServer, episode: &str, at: i64) {
        let mut state = server.state.lock().unwrap();
        state.added.push((ItemId(episode.into()), at));
    }

    /// A quarter of an hour goes by.
    fn later(cx: &mut TestAppContext) {
        cx.executor().advance_clock(notify::EVERY);
        cx.run_until_parked();
    }

    fn asks(server: &FakeServer) -> u32 {
        server.state.lock().unwrap().record.new_episode_asks
    }

    fn told(notifier: &FakeNotifier) -> Vec<(String, String)> {
        notifier
            .shown
            .borrow()
            .iter()
            .map(|n| (n.summary.clone(), n.body.clone()))
            .collect()
    }

    /// The mark the last check of `account` kept in the settings at `path`.
    fn mark(path: &Path, account: &str) -> Option<i64> {
        let seen = Settings::read(path).new_episodes_seen;
        seen.get(account).map(|seen| seen.mark)
    }

    fn settings_in(dir: &Path) -> PathBuf {
        dir.join("bobine/settings.toml")
    }

    #[gpui_kit::test]
    fn a_new_episode_of_a_followed_series_notifies_once(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (_window, notifier) = start(&server, None, cx);
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        assert_eq!(
            told(&notifier),
            [("Les Cartographes".into(), "S1 · E4 is available".into())]
        );
        later(cx);
        later(cx);
        assert_eq!(told(&notifier).len(), 1, "told once");
    }

    #[gpui_kit::test]
    fn a_series_that_is_not_followed_does_not_notify(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (_window, notifier) = start(&server, None, cx);
        // series-3 is neither in Favorites nor begun.
        add(&server, "series-3-s1-e2", 150);
        later(cx);
        assert_eq!(told(&notifier), []);
    }

    #[gpui_kit::test]
    fn several_episodes_are_grouped(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (_window, notifier) = start(&server, None, cx);
        for (episode, at) in [
            ("series-1-s1-e2", 150),
            ("series-1-s1-e3", 151),
            ("series-1-s1-e4", 152),
            ("series-0-s3-e1", 153),
        ] {
            add(&server, episode, at);
        }
        later(cx);
        let mut told = told(&notifier);
        told.sort();
        assert_eq!(
            told,
            [
                ("Les Cartographes".into(), "3 new episodes".into()),
                ("Station Nord".into(), "S3 · E1 is available".into()),
            ],
            "one per series"
        );
    }

    #[gpui_kit::test]
    fn a_restart_does_not_notify_again(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_in(dir.path());
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, Some(path.clone()), cx);
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        assert_eq!(told(&notifier).len(), 1);
        assert_eq!(mark(&path, ACCOUNT), Some(150));
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        // Bobine starts again: what was told is not told again; what came in since is.
        let (_window, again) = start(&server, Some(path.clone()), cx);
        assert_eq!(told(&again), []);
        add(&server, "series-4-s1-e1", 160);
        later(cx);
        assert_eq!(told(&again).len(), 1);
        assert_eq!(told(&again)[0].1, "S1 · E1 is available");
    }

    #[gpui_kit::test]
    fn the_switch_off_stops_notifications(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_in(dir.path());
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, Some(path.clone()), cx);
        window
            .update(cx, |shell, _, cx| shell.toggle_new_episodes(cx))
            .unwrap();
        let before = asks(&server);
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        assert_eq!(told(&notifier), []);
        assert_eq!(asks(&server), before, "nothing asked while it is off");
        // Off is kept: a restart asks nothing either.
        let (_window, again) = start(&server, Some(path), cx);
        later(cx);
        assert_eq!(told(&again), []);
        assert_eq!(asks(&server), before);
    }

    #[gpui_kit::test]
    fn switched_on_again_the_checks_start_afresh(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, None, cx);
        let toggle = |cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, cx| shell.toggle_new_episodes(cx))
                .unwrap();
            cx.run_until_parked();
        };
        toggle(cx);
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        toggle(cx);
        later(cx);
        assert_eq!(told(&notifier), [], "what came in meanwhile is not told");
        add(&server, "series-4-s1-e1", 160);
        later(cx);
        assert_eq!(told(&notifier).len(), 1);
    }

    #[gpui_kit::test]
    fn the_first_check_ever_notifies_nothing(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_in(dir.path());
        let server = FakeServer::new(6, true);
        add(&server, "series-1-s1-e4", 150);
        let (_window, notifier) = start(&server, Some(path.clone()), cx);
        later(cx);
        assert_eq!(told(&notifier), []);
        assert_eq!(mark(&path, ACCOUNT), Some(150));
    }

    #[gpui_kit::test]
    fn the_server_is_checked_at_the_start_and_every_quarter_of_an_hour(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (_window, _notifier) = start(&server, None, cx);
        assert_eq!(asks(&server), 1, "at the start");
        cx.executor()
            .advance_clock(notify::EVERY - std::time::Duration::from_secs(60));
        cx.run_until_parked();
        assert_eq!(asks(&server), 1);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(60));
        cx.run_until_parked();
        assert_eq!(asks(&server), 2);
    }

    /// Signs out, then in again through the form as `user_id`.
    fn sign_out_and_in_as(
        window: &WindowHandle<Shell>,
        user_id: &str,
        server: &FakeServer,
        cx: &mut TestAppContext,
    ) {
        window
            .update(cx, |shell, window, cx| shell.sign_out(window, cx))
            .unwrap();
        cx.run_until_parked();
        server.state.lock().unwrap().user_id = user_id.into();
        window
            .update(cx, |shell, _, cx| {
                let form = shell.login.as_ref().expect("the login shows");
                form.user.update(cx, |field, cx| field.set_text(USER, cx));
                form.password
                    .update(cx, |field, cx| field.set_text(PASSWORD, cx));
                shell.submit_login(cx);
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn signing_out_stops_the_checks_and_a_sign_in_starts_them_for_that_user(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = settings_in(dir.path());
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, Some(path.clone()), cx);
        add(&server, "series-1-s1-e4", 150);
        window
            .update(cx, |shell, window, cx| shell.sign_out(window, cx))
            .unwrap();
        cx.run_until_parked();
        let before = asks(&server);
        later(cx);
        assert_eq!(asks(&server), before, "nothing asked once signed out");
        assert_eq!(told(&notifier), []);
        // Back in as another user: their own first check, which tells nothing.
        sign_out_and_in_as(&window, "user-2", &server, cx);
        assert_eq!(asks(&server), before + 1, "checked at the sign-in");
        assert_eq!(told(&notifier), []);
        assert_eq!(mark(&path, "server-1/user-2"), Some(150));
        assert_eq!(mark(&path, ACCOUNT), Some(0), "the first user's own");
    }

    #[gpui_kit::test]
    fn the_poster_is_a_local_file_and_holds_no_token(cx: &mut TestAppContext) {
        let cache = tempfile::tempdir().unwrap();
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, None, cx);
        window
            .update(cx, |shell, _, _| {
                shell.art_dir = Some(cache.path().join("bobine").join("mpris"));
            })
            .unwrap();
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        let shown = notifier.shown.borrow()[0].clone();
        let icon = shown.icon.clone().expect("the series' poster");
        assert!(icon.starts_with(cache.path()), "{icon:?}");
        assert!(icon.exists());
        let printed = format!("{shown:?}");
        assert!(!printed.contains("http"), "{printed}");
    }

    #[gpui_kit::test]
    fn the_harness_keeps_notifications_off(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, None, cx);
        window
            .update(cx, |shell, _, _| shell.without_desktop())
            .unwrap();
        let before = asks(&server);
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        assert_eq!(asks(&server), before);
        assert_eq!(told(&notifier), []);
    }

    fn drawn(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> VisualTestContext {
        let visual = VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        visual
    }

    #[gpui_kit::test]
    fn a_click_opens_the_episode_on_its_series_page(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, None, cx);
        let mut visual = drawn(&window, cx);
        add(&server, "series-1-s1-e4", 150);
        later(cx);
        let open = notifier.shown.borrow()[0].open.clone();
        notifier.clicks.push(open);
        visual.run_until_parked();
        // Drawn twice: the page scrolls to the episode once it is laid out.
        visual.update(|window, _| window.refresh());
        visual.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.navigator.current(), Screen::SeriesDetail);
                let page = shell.series_pages.last().unwrap();
                assert_eq!(page.id, ItemId("series-1".into()));
                assert_eq!(page.seasons[page.season].id, ItemId("series-1-s1".into()));
                assert_eq!(*page.in_view.borrow(), None, "scrolled to it");
                assert!(page.scroll.offset().y < gpui_kit::px(0.), "scrolled down");
                assert!(
                    shell.playback.is_none() && shell.planned.is_empty(),
                    "not played"
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_click_for_another_account_opens_nothing(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, notifier) = start(&server, None, cx);
        let visual = drawn(&window, cx);
        notifier.clicks.push(Open {
            account: "server-1/user-2".into(),
            series: ItemId("series-1".into()),
            season: ItemId("series-1-s1".into()),
            episode: ItemId("series-1-s1-e4".into()),
        });
        visual.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.navigator.current(), Screen::Settings);
            })
            .unwrap();
    }
}
