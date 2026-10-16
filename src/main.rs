//! `bobine`: a native client for films and series, Jellyfin first.

mod chrome;
mod config;
mod discover;
#[cfg(any(test, feature = "harness"))]
mod fake;
#[cfg(test)]
mod fake_seerr;
mod fallback;
mod favourite;
mod film_page;
mod grid;
#[cfg(feature = "harness")]
mod harness;
mod hero;
mod home_page;
mod icons;
mod images;
mod input;
mod library_tools;
mod lists;
mod menu;
mod motion;
mod navigation;
mod player;
mod poster_actions;
mod reports;
mod screens;
mod scroll;
mod search;
mod secret_field;
mod series_page;
mod shell;
mod start;
mod theme;
mod video;

use std::sync::Arc;

use bobine::jellyfin::JellyfinConnector;
use bobine::provider::{Connector, ProviderError, SecretServiceStore, TokenStore};
use gpui_kit::prelude::*;
use gpui_kit::{App, Bounds, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size};

/// The Jellyfin connector over the account file and the system keyring.
fn open_jellyfin() -> Result<Arc<dyn Connector>, ProviderError> {
    let path = config::account_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .ok_or_else(|| ProviderError::Storage("no home directory".into()))?;
    let connector = JellyfinConnector::open(path, Arc::new(SecretServiceStore))?;
    Ok(Arc::new(connector))
}

fn main() {
    #[cfg(feature = "harness")]
    let harness = harness::Harness::from_args();
    gpui_kit::application()
        .with_assets(icons::Icons)
        .run(move |cx: &mut App| {
            input::bind_keys(cx);
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
            // The headless harness never touches the real account file nor the keyring.
            #[cfg(feature = "harness")]
            let open: shell::OpenConnector = match harness.connector() {
                Some(fake) => Box::new(move || Ok(fake)),
                None => Box::new(open_jellyfin),
            };
            #[cfg(not(feature = "harness"))]
            let open: shell::OpenConnector = Box::new(open_jellyfin);
            let settings = config::settings_path(
                std::env::var_os("XDG_CONFIG_HOME"),
                std::env::var_os("HOME"),
            );
            // Nor the real settings.
            #[cfg(feature = "harness")]
            let settings = settings.filter(|_| harness.connector().is_none());
            // Nor the real keyring.
            #[cfg(feature = "harness")]
            let secrets: Arc<dyn TokenStore> = if harness.connector().is_some() {
                Arc::new(fake::MemoryKeyring::default())
            } else {
                Arc::new(SecretServiceStore)
            };
            #[cfg(not(feature = "harness"))]
            let secrets: Arc<dyn TokenStore> = Arc::new(SecretServiceStore);
            let opened = cx.open_window(options, move |window, cx| {
                let shell = cx.new(|cx| shell::Shell::new(font, open, settings, secrets, cx));
                // The video surface and mpv go before the window's surface does.
                let closing = shell.downgrade();
                window.on_window_should_close(cx, move |_, cx| {
                    closing.update(cx, |shell, _| shell.close_video()).ok();
                    true
                });
                #[cfg(feature = "harness")]
                harness.start(&shell, window, cx);
                shell
            });
            if let Err(error) = opened {
                eprintln!("bobine: cannot open the window: {error}");
                std::process::exit(1);
            }
        });
}
