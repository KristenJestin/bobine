//! The Settings screen: what it keeps beside the settings themselves (the open quality menu, the
//! integrations' secret fields) and how a change is made and kept.

use std::sync::Arc;

use bobine::provider::{ServerInfo, TokenStore};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Pixels, Point};

use super::Shell;
use crate::chrome::ButtonBounds;
use crate::config::Quality;
use crate::menu::Menus;
use crate::motion::{self, Presence};
use crate::navigation::Screen;
use crate::secret_field::{self, SecretField};

/// The one menu of the screen: the choices of the default quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QualityMenu;

pub struct SettingsPage {
    /// The keys the integrations ask for, one field each. There are none until #63.
    pub secrets: Vec<Entity<SecretField>>,
    pub menus: Menus<QualityMenu>,
    /// The menu fades in and out; the button it opens from is where this says it was drawn.
    pub menu_fade: Presence<QualityMenu>,
    pub quality_button: ButtonBounds,
}

impl Default for SettingsPage {
    fn default() -> Self {
        Self {
            secrets: Vec::new(),
            menus: Menus::default(),
            menu_fade: Presence::new(motion::quick()),
            quality_button: Default::default(),
        }
    }
}

/// What the About section says: the program, and the server it is signed in to.
pub fn about(server: &ServerInfo) -> [(&'static str, String); 3] {
    [
        ("Bobine", env!("CARGO_PKG_VERSION").to_string()),
        ("Server", server.name.clone()),
        ("Server version", server.version.clone()),
    ]
}

impl Shell {
    /// The account menu's "Settings": over the screen the viewer is on, which Escape returns to.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.account_menu.close();
        if self.navigator.current() == Screen::Settings {
            cx.notify();
            return;
        }
        self.forward.clear();
        self.navigator.open(Screen::Settings);
        cx.notify();
    }

    /// What the About section says, once signed in.
    pub fn about(&self) -> Vec<(&'static str, String)> {
        match &self.server {
            Some(server) => about(server.server()).into(),
            None => Vec::new(),
        }
    }

    /// The compositor was asked at the first film and cannot cover a monitor: the switch does
    /// nothing there.
    pub fn blackout_unavailable(&self) -> bool {
        self.screens_tried && self.screens.is_none()
    }

    /// Writes the settings. A file that cannot be written leaves them holding until Bobine quits.
    pub(super) fn keep_settings(&self) {
        if let Some(path) = &self.settings_path {
            let _ = self.settings.save(path);
        }
    }

    pub fn set_quality(&mut self, quality: Quality, cx: &mut Context<Self>) {
        self.settings.default_quality = quality;
        self.settings_page.menus.close();
        self.keep_settings();
        cx.notify();
    }

    pub fn toggle_quality_menu(&mut self, cx: &mut Context<Self>) {
        self.settings_page.menus.toggle(QualityMenu);
        cx.notify();
    }

    /// The mouse went down outside the open quality menu, maybe on its button.
    pub fn pressed_outside_quality_menu(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let page = &mut self.settings_page;
        let on = page
            .quality_button
            .get()
            .is_some_and(|bounds| bounds.contains(&position))
            .then_some(QualityMenu);
        page.menus.pressed_outside(on);
        cx.notify();
    }

    /// A field for a secret the settings keep in the keyring under `key`, for the signed-in user
    /// of this server, at the end of the Integrations section. None without a session: the
    /// secret belongs to someone.
    #[cfg_attr(not(any(test, feature = "harness")), allow(dead_code))]
    pub fn add_secret_field(
        &mut self,
        label: &'static str,
        key: &str,
        cx: &mut Context<Self>,
    ) -> Option<Entity<SecretField>> {
        let server = self.server.as_ref()?;
        let account = secret_field::account(&server.server().id, server.user_id(), key);
        let store: Arc<dyn TokenStore> = self.secrets.clone();
        let field = cx.new(|cx| SecretField::new(label, account, store, cx));
        self.settings_page.secrets.push(field.clone());
        cx.notify();
        Some(field)
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use bobine::provider::{Connector, TokenStore};
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::config::{Quality, Settings};
    use crate::fake::{FakeServer, MemoryKeyring, PASSWORD, USER};
    use crate::navigation::Screen;

    /// A shell on a kept session, its settings at `path`, its keyring in memory.
    fn start(
        path: Option<PathBuf>,
        keyring: &Arc<MemoryKeyring>,
        cx: &mut TestAppContext,
    ) -> WindowHandle<Shell> {
        let connector: Arc<dyn Connector> = Arc::new(FakeServer::new(6, true));
        let secrets: Arc<dyn TokenStore> = keyring.clone();
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(None, Box::new(move || Ok(connector)), path, secrets, cx)
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

    fn files_in(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(files_in(&path));
            } else {
                found.push(path);
            }
        }
        found
    }

    #[gpui_kit::test]
    fn the_settings_screen_opens_from_the_account_menu(cx: &mut TestAppContext) {
        let window = start(None, &Arc::new(MemoryKeyring::default()), cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "nav-account");
        click(&mut visual, "account-settings");
        assert_eq!(screen(&window, cx), Screen::Settings);
        assert!(visual.debug_bounds("settings-page").is_some());
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.account_menu.open(), None, "the menu stays open");
            })
            .unwrap();
        // Escape leaves them for where they were reached from.
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn the_settings_have_a_section_each_for_playback_screens_integrations_and_about(
        cx: &mut TestAppContext,
    ) {
        let window = start(None, &Arc::new(MemoryKeyring::default()), cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        for section in [
            "settings-playback",
            "settings-screens",
            "settings-integrations",
            "settings-about",
        ] {
            assert!(visual.debug_bounds(section).is_some(), "{section}");
        }
    }

    #[gpui_kit::test]
    fn a_change_is_saved_and_read_back_at_the_next_start(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine/settings.toml");
        let keyring = Arc::new(MemoryKeyring::default());
        let window = start(Some(path.clone()), &keyring, cx);
        window
            .update(cx, |shell, window, cx| {
                shell.open_settings(cx);
                shell.set_quality(Quality::Cap(8), cx);
                shell.toggle_blackout(window, cx);
            })
            .unwrap();
        let again = start(Some(path.clone()), &keyring, cx);
        again
            .update(cx, |shell, _, _| {
                assert_eq!(shell.settings.default_quality, Quality::Cap(8));
                assert!(shell.settings.black_out_other_monitors);
            })
            .unwrap();
        assert_eq!(
            Settings::read(&path),
            Settings {
                black_out_other_monitors: true,
                default_quality: Quality::Cap(8),
            }
        );
    }

    #[gpui_kit::test]
    fn an_unreadable_settings_file_gives_the_defaults(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "default_quality = [1, 2\n\u{0}\u{0}").unwrap();
        let window = start(Some(path), &Arc::new(MemoryKeyring::default()), cx);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.settings, Settings::default());
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_secret_entered_in_the_settings_is_in_the_keyring_and_never_in_a_file(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine/settings.toml");
        let keyring = Arc::new(MemoryKeyring::default());
        let window = start(Some(path), &keyring, cx);
        let secret = "k3y-0123456789abcdef";
        let field = window
            .update(cx, |shell, _, cx| {
                shell.open_settings(cx);
                shell.add_secret_field("Demo key", "demo-key", cx)
            })
            .unwrap()
            .expect("signed in");
        // The keyring is asked first whether it already holds one.
        cx.run_until_parked();
        field.update(cx, |field, cx| {
            field.type_in(secret, cx);
            field.save(cx);
        });
        cx.run_until_parked();
        // Something else is changed, so that the settings file is written after the secret.
        window
            .update(cx, |shell, _, cx| shell.set_quality(Quality::Cap(4), cx))
            .unwrap();
        assert_eq!(
            keyring
                .secret("setting/server-1/user-1/demo-key")
                .as_deref(),
            Some(secret)
        );
        let files = files_in(dir.path());
        assert!(!files.is_empty(), "the settings were not written");
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(!text.contains(secret), "{}", file.display());
        }
    }

    /// A secret field of the signed-in session, with `secret` saved in it.
    fn save_a_secret(
        window: &WindowHandle<Shell>,
        secret: &str,
        cx: &mut TestAppContext,
    ) -> Entity<SecretField> {
        let field = add_field(window, cx);
        field.update(cx, |field, cx| {
            field.type_in(secret, cx);
            field.save(cx);
        });
        cx.run_until_parked();
        field
    }

    fn add_field(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Entity<SecretField> {
        let field = window
            .update(cx, |shell, _, cx| {
                shell.add_secret_field("Demo key", "demo-key", cx)
            })
            .unwrap()
            .expect("signed in");
        cx.run_until_parked();
        field
    }

    /// Signs out and signs in again through the form, as `user`.
    fn sign_out_and_in_as(
        window: &WindowHandle<Shell>,
        user_id: &str,
        server: &FakeServer,
        cx: &mut TestAppContext,
    ) {
        server.state.lock().unwrap().user_id = user_id.into();
        window
            .update(cx, |shell, window, cx| shell.sign_out(window, cx))
            .unwrap();
        cx.run_until_parked();
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
        window
            .update(cx, |shell, _, _| {
                assert!(shell.server.is_some(), "signed in")
            })
            .unwrap();
    }

    fn start_on(
        server: &FakeServer,
        keyring: &Arc<MemoryKeyring>,
        cx: &mut TestAppContext,
    ) -> WindowHandle<Shell> {
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let secrets: Arc<dyn TokenStore> = keyring.clone();
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            Shell::new(None, Box::new(move || Ok(connector)), None, secrets, cx)
        });
        cx.run_until_parked();
        window
    }

    #[gpui_kit::test]
    fn a_secret_is_not_shown_to_another_user_of_the_server(cx: &mut TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let server = FakeServer::new(6, true);
        let window = start_on(&server, &keyring, cx);
        save_a_secret(&window, "k3y-A", cx);
        sign_out_and_in_as(&window, "user-2", &server, cx);
        let field = add_field(&window, cx);
        field.read_with(cx, |field, _| assert!(!field.is_saved()));
        assert_eq!(keyring.secret("setting/server-1/user-2/demo-key"), None);
    }

    #[gpui_kit::test]
    fn a_secret_is_not_shown_on_another_server(cx: &mut TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let server = FakeServer::new(6, true);
        let window = start_on(&server, &keyring, cx);
        save_a_secret(&window, "k3y-A", cx);
        let other = FakeServer::new(6, true).on_server("server-2");
        let window = start_on(&other, &keyring, cx);
        let field = add_field(&window, cx);
        field.read_with(cx, |field, _| assert!(!field.is_saved()));
    }

    #[gpui_kit::test]
    fn signing_out_keeps_the_secret_for_the_user_who_signs_back_in(cx: &mut TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let server = FakeServer::new(6, true);
        let window = start_on(&server, &keyring, cx);
        save_a_secret(&window, "k3y-A", cx);
        sign_out_and_in_as(&window, "user-1", &server, cx);
        assert_eq!(
            keyring
                .secret("setting/server-1/user-1/demo-key")
                .as_deref(),
            Some("k3y-A")
        );
        let field = add_field(&window, cx);
        field.read_with(cx, |field, _| assert!(field.is_saved()));
    }

    #[gpui_kit::test]
    fn signing_out_drops_the_secret_fields_of_the_settings_page(cx: &mut TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let server = FakeServer::new(6, true);
        let window = start_on(&server, &keyring, cx);
        save_a_secret(&window, "k3y-A", cx);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.settings_page.secrets.len(), 1)
            })
            .unwrap();
        sign_out_and_in_as(&window, "user-2", &server, cx);
        window
            .update(cx, |shell, _, _| {
                assert!(shell.settings_page.secrets.is_empty())
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn a_secret_field_cannot_be_added_without_a_session(cx: &mut TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let server = FakeServer::new(6, false);
        let window = start_on(&server, &keyring, cx);
        let added = window
            .update(cx, |shell, _, cx| {
                shell.add_secret_field("Demo key", "demo-key", cx)
            })
            .unwrap();
        assert!(added.is_none());
    }

    #[gpui_kit::test]
    fn the_quality_menu_closes_on_escape_before_the_settings_do(cx: &mut TestAppContext) {
        let window = start(None, &Arc::new(MemoryKeyring::default()), cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "settings-quality");
        assert!(visual.debug_bounds("quality-menu").is_some());
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Settings);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.settings_page.menus.open(), None)
            })
            .unwrap();
        visual.simulate_keystrokes("escape");
        assert_eq!(screen(&window, cx), Screen::Home);
    }

    #[gpui_kit::test]
    fn a_click_outside_closes_the_quality_menu(cx: &mut TestAppContext) {
        let window = start(None, &Arc::new(MemoryKeyring::default()), cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "settings-quality");
        click(&mut visual, "settings-about");
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.settings_page.menus.open(), None)
            })
            .unwrap();
        assert_eq!(screen(&window, cx), Screen::Settings);
    }

    #[gpui_kit::test]
    fn choosing_a_quality_in_the_menu_keeps_it_and_closes_the_menu(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let window = start(Some(path.clone()), &Arc::new(MemoryKeyring::default()), cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "settings-quality");
        click(&mut visual, "quality-4mbps");
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.settings.default_quality, Quality::Cap(4));
                assert_eq!(shell.settings_page.menus.open(), None);
            })
            .unwrap();
        assert_eq!(Settings::read(&path).default_quality, Quality::Cap(4));
    }

    #[gpui_kit::test]
    fn the_switch_of_the_settings_blacks_out_the_other_screens(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let window = start(Some(path.clone()), &Arc::new(MemoryKeyring::default()), cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "settings-blackout");
        assert!(Settings::read(&path).black_out_other_monitors);
        click(&mut visual, "settings-blackout");
        assert!(!Settings::read(&path).black_out_other_monitors);
    }

    #[test]
    fn the_about_says_the_app_version_and_the_server_and_its_version() {
        let server = bobine::provider::ServerInfo {
            id: "server-1".into(),
            name: "Cinéma du coin".into(),
            version: "10.10.7".into(),
        };
        assert_eq!(
            about(&server),
            [
                ("Bobine", env!("CARGO_PKG_VERSION").to_string()),
                ("Server", "Cinéma du coin".to_string()),
                ("Server version", "10.10.7".to_string()),
            ]
        );
    }
}
