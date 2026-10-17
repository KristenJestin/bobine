//! The Stats page: what the viewer watched, read with the Home, and offered in the account menu
//! only while there is something to show.

use std::rc::Rc;
use std::sync::Arc;

use bobine::provider::LocalTime;
use gpui_kit::Context;

use super::Shell;
use crate::navigation::Screen;
use crate::stats;

impl Shell {
    /// Reads the watch history again; the stats before stay shown until it comes. Nothing to
    /// show, or a history that cannot be read, takes the page away.
    pub(super) fn load_stats(&mut self, cx: &mut Context<Self>) {
        let session = self.server.clone();
        self.ask(
            cx,
            |server| server.watch_history(),
            move |shell, history, cx| {
                // An answer for a user who signed out meanwhile is not the next user's.
                let same = match (&session, &shell.server) {
                    (Some(asked), Some(now)) => Arc::ptr_eq(asked, now),
                    _ => false,
                };
                if !same {
                    return;
                }
                shell.stats = history
                    .ok()
                    .and_then(|history| stats::of(&history, LocalTime::now()))
                    .map(Rc::new);
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
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(MemoryKeyring::default()),
                cx,
            )
        });
        cx.run_until_parked();
        (server, window)
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
            .update(cx, |shell, _, cx| shell.load_home(cx))
            .unwrap();
        cx.run_until_parked();
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
    fn a_history_that_cannot_be_read_shows_no_page(cx: &mut TestAppContext) {
        let history = fake::watch_history(HistorySource::PlaybackReporting, NOW);
        let (server, window) = start(history, cx);
        assert!(source(&window, cx).is_some());
        server.state.lock().unwrap().history_error = Some(ProviderError::Server(500));
        reload(&window, cx);
        assert_eq!(source(&window, cx), None);
    }
}
