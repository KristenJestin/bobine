//! The Stats page: what the viewer watched, read at sign-in, after a playback and when the page
//! opens, and offered in the account menu only while there is something to show.

use std::rc::Rc;
use std::sync::Arc;

use bobine::provider::LocalTime;
use gpui_kit::Context;

use super::Shell;
use crate::navigation::Screen;
use crate::stats;

impl Shell {
    /// Reads the watch history again, at sign-in, after a playback and when the page opens, not
    /// at each arrival on the Home: a long history takes a while. The stats before stay shown
    /// until it comes, or stay if it cannot be read. Nothing to show takes the page away.
    pub(super) fn load_stats(&mut self, cx: &mut Context<Self>) {
        if self.server.is_none() || self.stats_reading {
            return;
        }
        self.stats_reading = true;
        self.stats_asked += 1;
        let asked = self.stats_asked;
        let session = self.server.clone();
        self.ask(
            cx,
            // Counted here, off the window's thread: a long history takes a while.
            move |server| {
                server
                    .watch_history()
                    .map(|history| stats::of(&history, LocalTime::now()))
            },
            move |shell, stats, cx| {
                // An answer for a user who signed out meanwhile is not the next user's.
                let same = match (&session, &shell.server) {
                    (Some(asked), Some(now)) => Arc::ptr_eq(asked, now),
                    _ => false,
                };
                if !same || shell.stats_asked != asked {
                    return;
                }
                shell.stats_reading = false;
                // A read that failed leaves what was shown: the next one may well succeed.
                let Ok(stats) = stats else {
                    return;
                };
                shell.stats = stats.map(Rc::new);
                if shell.stats.is_none() {
                    shell.navigator.forget(|screen| screen == Screen::Stats);
                }
                cx.notify();
            },
        );
    }

    /// The account menu's "Stats": over the screen the viewer is on, which Escape returns to.
    pub fn open_stats(&mut self, cx: &mut Context<Self>) {
        self.account_menu.close();
        if self.stats.is_none() || self.navigator.current() == Screen::Stats {
            cx.notify();
            return;
        }
        self.forward.clear();
        self.navigator.open(Screen::Stats);
        self.load_stats(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bobine::provider::{Connector, HistorySource, ProviderError, WatchHistory};
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::fake::{self, FakeServer, MemoryKeyring};

    /// Saturday 17 October 2026, at noon UTC.
    const NOW: i64 = 1_792_238_400;

    /// A shell on a kept session of a server that holds `history`.
    fn start(history: WatchHistory, cx: &mut TestAppContext) -> (FakeServer, WindowHandle<Shell>) {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().history = history;
        let window = start_on(&server, cx);
        (server, window)
    }

    fn start_on(server: &FakeServer, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(MemoryKeyring::default()),
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

    fn click(visual: &mut VisualTestContext, selector: &'static str) {
        let at = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();
    }

    fn screen(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Screen {
        window
            .update(cx, |shell, _, _| shell.navigator.current())
            .unwrap()
    }

    fn source(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Option<HistorySource> {
        window
            .update(cx, |shell, _, _| {
                shell.stats.as_ref().map(|stats| stats.source)
            })
            .unwrap()
    }

    fn reload(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| shell.load_stats(cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn reads(server: &FakeServer) -> u32 {
        server.state.lock().unwrap().record.histories
    }

    #[gpui_kit::test]
    fn with_the_plugin_its_numbers_are_shown(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlaybackReporting, NOW);
        let (_server, window) = start(history, cx);
        assert_eq!(source(&window, cx), Some(HistorySource::PlaybackReporting));
        let mut visual = drawn(&window, cx);
        click(&mut visual, "nav-account");
        click(&mut visual, "account-stats");
        assert_eq!(screen(&window, cx), Screen::Stats);
        assert!(visual.debug_bounds("stats-page").is_some());
        assert!(
            visual.debug_bounds("stats-days").is_some(),
            "by day of the week"
        );
        assert!(visual.debug_bounds("stats-hours").is_some(), "by hour");
        assert!(visual.debug_bounds("stats-no-activity").is_none());
        // Escape leaves it for where it was reached from.
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn without_the_plugin_the_played_items_are_used_and_the_page_says_so(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlayedItems, NOW);
        let (_server, window) = start(history, cx);
        assert_eq!(source(&window, cx), Some(HistorySource::PlayedItems));
        window
            .update(cx, |shell, _, cx| shell.open_stats(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("stats-source").is_some());
        assert!(
            visual.debug_bounds("stats-no-activity").is_some(),
            "no empty charts: the page says why there are none"
        );
        assert!(visual.debug_bounds("stats-days").is_none());
        assert!(crate::screens::stats::source_note(HistorySource::PlayedItems).contains("played"));
    }

    #[gpui_kit::test]
    fn with_no_data_at_all_there_is_no_page(cx: &mut TestAppContext) {
        let nothing = WatchHistory {
            source: HistorySource::PlayedItems,
            plays: Vec::new(),
        };
        let (server, window) = start(nothing, cx);
        assert_eq!(
            server.state.lock().unwrap().record.histories,
            1,
            "it was asked"
        );
        assert_eq!(source(&window, cx), None);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "nav-account");
        assert!(visual.debug_bounds("account-settings").is_some());
        assert!(
            visual.debug_bounds("account-stats").is_none(),
            "no way to reach it"
        );
        window
            .update(cx, |shell, _, cx| shell.open_stats(cx))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn the_page_goes_when_the_history_is_found_empty_again(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlaybackReporting, NOW);
        let (server, window) = start(history, cx);
        window
            .update(cx, |shell, _, cx| shell.open_stats(cx))
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Stats);
        server.state.lock().unwrap().history.plays.clear();
        reload(&window, cx);
        assert_eq!(source(&window, cx), None);
        assert_ne!(screen(&window, cx), Screen::Stats);
    }

    #[gpui_kit::test]
    fn a_history_that_cannot_be_read_keeps_the_stats_and_the_page(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlaybackReporting, NOW);
        let (server, window) = start(history, cx);
        window
            .update(cx, |shell, _, cx| shell.open_stats(cx))
            .unwrap();
        cx.run_until_parked();
        server.state.lock().unwrap().history_error = Some(ProviderError::Server(500));
        reload(&window, cx);
        assert_eq!(source(&window, cx), Some(HistorySource::PlaybackReporting));
        assert_eq!(screen(&window, cx), Screen::Stats);
    }

    #[gpui_kit::test]
    fn a_first_read_that_fails_shows_no_page(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        server.state.lock().unwrap().history_error = Some(ProviderError::Server(500));
        let window = start_on(&server, cx);
        assert_eq!(source(&window, cx), None);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "nav-account");
        assert!(visual.debug_bounds("account-stats").is_none());
    }

    #[gpui_kit::test]
    fn home_arrivals_do_not_read_the_history_again_opening_the_page_does(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlaybackReporting, NOW);
        let (server, window) = start(history, cx);
        assert_eq!(reads(&server), 1, "read at sign-in");
        let mut visual = drawn(&window, cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Home);
        assert_eq!(reads(&server), 1, "back on the Home");
        window
            .update(cx, |shell, _, cx| shell.open_stats(cx))
            .unwrap();
        cx.run_until_parked();
        assert_eq!(reads(&server), 2, "the page opened");
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Home);
        assert_eq!(reads(&server), 2, "back on the Home again");
    }

    #[gpui_kit::test]
    fn a_playback_ended_on_the_home_reads_the_history_again(cx: &mut TestAppContext) {
        let (server, window) = start(fake::watch_history(HistorySource::PlayedItems, NOW), cx);
        window
            .update(cx, |shell, _, cx| {
                shell.after_playback(std::thread::spawn(|| {}), cx)
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(reads(&server), 2);
    }

    #[gpui_kit::test]
    fn no_second_read_while_one_runs(cx: &mut TestAppContext) {
        let (server, window) = start(fake::watch_history(HistorySource::PlayedItems, NOW), cx);
        window
            .update(cx, |shell, _, cx| {
                shell.load_stats(cx);
                shell.load_stats(cx);
            })
            .unwrap();
        cx.run_until_parked();
        assert_eq!(reads(&server), 2, "one more read, not two");
        reload(&window, cx);
        assert_eq!(reads(&server), 3, "once it landed, the next one is read");
    }

    #[gpui_kit::test]
    fn the_arrows_put_the_ring_on_a_poster_and_enter_opens_it(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlaybackReporting, NOW);
        let (_server, window) = start(history, cx);
        window
            .update(cx, |shell, _, cx| shell.open_stats(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        visual.simulate_keystrokes("right");
        let ringed = window
            .read_with(cx, |shell, _| shell.ring.shown_on().cloned())
            .unwrap();
        assert_eq!(ringed, Some(("stats-movies", 0usize).into()));
        // The row below, further down the page: the page scrolls to it.
        visual.simulate_keystrokes("down");
        visual.run_until_parked();
        let ringed = window
            .read_with(cx, |shell, _| shell.ring.shown_on().cloned())
            .unwrap();
        assert_eq!(ringed, Some(("stats-series", 0usize).into()));
        let bounds = visual
            .debug_bounds("stats-series")
            .expect("the row is drawn");
        let height = window
            .update(cx, |_, window, _| f32::from(window.viewport_size().height))
            .unwrap();
        assert!(
            f32::from(bounds.bottom()) <= height,
            "{bounds:?} in {height}"
        );
        visual.simulate_keystrokes("up");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(screen(&window, cx), Screen::FilmDetail);
    }
}
