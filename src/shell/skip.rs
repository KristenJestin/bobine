//! The skip button's share of the shell: the segments the server marked, asked once when a
//! playback starts, and the press of the button.

use bobine::provider::ItemId;
use gpui_kit::{Context, Window};

use super::Shell;
use crate::series_page::Next;

impl Shell {
    /// The player started: the episode after it is known, and the server is asked for the
    /// segments of what plays, off the window's thread. A server that has none, or that fails,
    /// leaves the player without the button, and says nothing.
    pub(super) fn playback_started(
        &mut self,
        item: &ItemId,
        next: Option<Next>,
        cx: &mut Context<Self>,
    ) {
        let generation = self.next_generation();
        let Some(playback) = &mut self.playback else {
            return;
        };
        playback.next = next;
        // The credits offer "Next episode" for an episode only: a playlist's next title waits
        // for the card.
        playback
            .session
            .set_next_episode(matches!(playback.next, Some(Next::Episode(_))));
        playback.generation = generation;
        let item = item.clone();
        self.ask(
            cx,
            move |server| server.media_segments(&item),
            move |shell, segments, _| {
                // The viewer may have left, or started another film, while the server answered.
                if let (Some(playback), Ok(segments)) = (&mut shell.playback, segments)
                    && playback.generation == generation
                {
                    playback.session.set_segments(segments);
                }
            },
        );
    }

    /// The skip button was clicked.
    pub fn press_skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let request = playback.session.press_skip();
        self.player_request(request, window, cx);
        cx.notify();
    }
}
