//! `bobine`: a native client for films and series, Jellyfin first.

mod brand;
mod chrome;
mod config;
mod curtain;
mod discover;
#[cfg(any(test, feature = "harness"))]
mod fake;
#[cfg(any(test, feature = "harness"))]
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
mod instance;
mod keys;
mod launcher;
mod library_tools;
mod lists;
mod menu;
mod motion;
mod mpris;
mod navigation;
mod notify;
mod player;
mod playlists;
mod poster_actions;
mod reports;
mod ring;
mod screens;
mod scroll;
mod search;
mod secret_field;
mod series_page;
mod shell;
mod start;
mod stats;
mod theme;
mod trailer;
mod video;

use std::sync::Arc;

use bobine::jellyfin::JellyfinConnector;
use bobine::provider::{Connector, ProviderError, SecretServiceStore, TokenStore};
use gpui_kit::prelude::*;
use gpui_kit::{App, Bounds, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size};

/// The window's Jellyfin connector: the keyring may ask the user to unlock it.
fn open_jellyfin() -> Result<Arc<dyn Connector>, ProviderError> {
    open_jellyfin_with(SecretServiceStore::UNLOCKING)
}

/// The Jellyfin connector over the account file and the system keyring.
fn open_jellyfin_with(store: SecretServiceStore) -> Result<Arc<dyn Connector>, ProviderError> {
    let path = config::account_path(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
    .ok_or_else(|| ProviderError::Storage("no home directory".into()))?;
    let connector = JellyfinConnector::open(path, Arc::new(store))?;
    Ok(Arc::new(connector))
}

/// What a command of the launcher prints, and its exit code.
fn exit_with(outcome: launcher::Outcome) -> ! {
    println!("{}", outcome.json());
    std::process::exit(outcome.code);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--version") {
        println!("bobine {}", bobine::VERSION);
        return;
    }
    let socket = instance::socket_path(std::env::var_os("XDG_RUNTIME_DIR"));
    // `open` and `play` go to the window running, else open one for themselves.
    let first = match launcher::parse(&args) {
        None => None,
        Some(Err(said)) => exit_with(launcher::Outcome::usage(&said)),
        Some(Ok(launcher::Command::Ask(request))) => {
            if socket
                .as_deref()
                .is_some_and(|path| instance::hand_off(path, &request))
            {
                return;
            }
            Some(request)
        }
        // A locked keyring is no dialog for the launcher: not signed in.
        Some(Ok(command)) => {
            let posters =
                launcher::poster_dir(std::env::var_os("XDG_CACHE_HOME"), std::env::var_os("HOME"))
                    .map(launcher::Posters::new);
            exit_with(launcher::run(
                &command,
                || open_jellyfin_with(SecretServiceStore::WITHOUT_PROMPT),
                posters.as_ref(),
            ))
        }
    };
    #[cfg(feature = "harness")]
    let harness = harness::Harness::from_args();
    // The headless harness on its fake server never takes the commands of the real one.
    #[cfg(feature = "harness")]
    let socket = socket.filter(|_| harness.connector().is_none());
    let commands = socket.as_deref().and_then(instance::listen);
    gpui_kit::application()
        .with_assets(icons::Icons)
        .run(move |cx: &mut App| {
            input::bind_keys(cx);
            keys::bind(cx);
            let bounds = Bounds::centered(None, size(px(1440.), px(810.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                app_id: Some(bobine::APP_ID.into()),
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
                Arc::new(SecretServiceStore::UNLOCKING)
            };
            #[cfg(not(feature = "harness"))]
            let secrets: Arc<dyn TokenStore> = Arc::new(SecretServiceStore::UNLOCKING);
            let opened = cx.open_window(options, move |window, cx| {
                let shell = cx.new(|cx| {
                    let mut shell = shell::Shell::new(font, open, settings, secrets, cx);
                    shell.hear_commands(commands, first);
                    shell
                });
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
