//! `bobine`: a native client for films and series, Jellyfin first.

mod chrome;
#[cfg(feature = "harness")]
mod harness;
mod navigation;
mod player;
mod screens;
mod shell;
mod theme;
mod video;

use gpui_kit::prelude::*;
use gpui_kit::{App, Bounds, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size};

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1440.), px(810.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("bobine".into()),
            // The video is under the window: the player leaves a hole for it, every other
            // screen paints its own background.
            window_background: WindowBackgroundAppearance::Transparent,
            // A film plays on a window that may not have the focus.
            inactive_frame_interval: None,
            ..Default::default()
        };
        let font = theme::font_family(cx);
        let opened = cx.open_window(options, |window, cx| {
            let shell = cx.new(|cx| shell::Shell::new(font, cx));
            // The video surface and mpv go before the window's surface does.
            let closing = shell.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                closing.update(cx, |shell, _| shell.stop_playback()).ok();
                true
            });
            #[cfg(feature = "harness")]
            harness::start(&shell, window, cx);
            shell
        });
        if let Err(error) = opened {
            eprintln!("bobine: cannot open the window: {error}");
            std::process::exit(1);
        }
    });
}
