//! `bobine`: a native client for films and series, Jellyfin first.

mod chrome;
mod navigation;
mod screens;
mod shell;
mod theme;

use gpui_kit::prelude::*;
use gpui_kit::{App, Bounds, WindowBounds, WindowOptions, px, size};

fn main() {
    gpui_kit::application().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1440.), px(810.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("bobine".into()),
            // A film plays on a window that may not have the focus.
            inactive_frame_interval: None,
            ..Default::default()
        };
        let font = theme::font_family(cx);
        if let Err(error) =
            cx.open_window(options, |_, cx| cx.new(|cx| shell::Shell::new(font, cx)))
        {
            eprintln!("bobine: cannot open the window: {error}");
            std::process::exit(1);
        }
    });
}
