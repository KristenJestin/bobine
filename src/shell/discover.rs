//! The request server (#63): how its client is made from the Settings (the address in the
//! settings file, the key in the keyring), and what the window keeps of it. Nothing of it shows
//! while it is not configured but the Integrations section of the Settings.

use std::sync::Arc;

use bobine::provider::{Token, TokenStore};
use bobine::requests::{RequestError, RequestServer, ServerCheck};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Entity, Subscription};

use super::{Loadable, Shell};
use crate::input::TextInput;
use crate::secret_field::{self, SecretField};

/// Where the keyring keeps the request server's API key, for the signed-in user.
pub const SEERR_KEY: &str = "seerr-api-key";

/// Makes a client of the request server at an address, with its key, for the Jellyfin user.
pub type OpenSeerr =
    Arc<dyn Fn(&str, Token, &str) -> Result<Arc<dyn RequestServer>, RequestError> + Send + Sync>;

/// The real client. A test never reaches a real server: it gives a fake, and this refuses.
pub fn real_seerr() -> OpenSeerr {
    #[cfg(not(test))]
    return Arc::new(|address, key, viewer| {
        let client = bobine::seerr::SeerrClient::new(address, key, viewer)?;
        Ok(Arc::new(client) as Arc<dyn RequestServer>)
    });
    #[cfg(test)]
    Arc::new(|_, _, _| Err(RequestError::Unreachable("no real server in a test".into())))
}

/// The request server, as the Settings and the pages see it.
#[derive(Default)]
pub struct Seerr {
    /// Made once the address and the key are both there: Discover shows from then on.
    pub client: Option<Arc<dyn RequestServer>>,
    /// The Settings' field of the address, and the key's.
    pub address: Option<Entity<TextInput>>,
    pub key: Option<Entity<SecretField>>,
    /// What the key field said at the last look: a change makes the client again.
    key_saved: bool,
    /// What the last test of the connection said: the version, or why it failed.
    pub check: Option<Loadable<String>>,
    /// Said under the fields when the requests go as the key's owner, not the viewer.
    pub owner_line: Option<String>,
    /// The number of the client being made: an older making that ends late is dropped.
    making: u64,
    _key_changed: Option<Subscription>,
}

impl Shell {
    /// The fields of the Integrations section, for the signed-in user: the address from the
    /// settings file, the key from the keyring. The client follows them.
    pub(super) fn set_up_seerr(&mut self, cx: &mut Context<Self>) {
        let address = cx.new(|cx| {
            let mut input = TextInput::new("https://seerr.example.org", false, cx);
            if let Some(kept) = &self.settings.seerr_address {
                input.set_text(kept, cx);
            }
            input
        });
        let Some(key) = self.add_secret_field("API key", SEERR_KEY, cx) else {
            return;
        };
        let changed = cx.observe(&key, |shell, key, cx| {
            let saved = key.read(cx).is_saved();
            if saved != shell.seerr.key_saved {
                shell.seerr.key_saved = saved;
                shell.connect_seerr(cx);
            }
        });
        self.seerr = Seerr {
            address: Some(address),
            key: Some(key),
            _key_changed: Some(changed),
            ..Seerr::default()
        };
    }

    /// The address typed is kept (an empty one forgets it) and the client made again.
    pub fn save_seerr_address(&mut self, cx: &mut Context<Self>) {
        let Some(input) = &self.seerr.address else {
            return;
        };
        let typed = input
            .read(cx)
            .text()
            .trim()
            .trim_end_matches('/')
            .to_string();
        let address = (!typed.is_empty()).then_some(typed);
        if address == self.settings.seerr_address {
            return;
        }
        self.settings.seerr_address = address;
        self.keep_settings();
        self.connect_seerr(cx);
        cx.notify();
    }

    /// Makes the client from the address and the key, or drops it when one of them is missing:
    /// Discover goes with it. The key is read from the keyring off the window's thread, and goes
    /// from there to the client only.
    fn connect_seerr(&mut self, cx: &mut Context<Self>) {
        self.seerr.making += 1;
        let making = self.seerr.making;
        self.seerr.check = None;
        self.seerr.owner_line = None;
        let configured = self
            .settings
            .seerr_address
            .clone()
            .filter(|_| self.seerr.key_saved);
        let (Some(address), Some(server)) = (configured, self.server.clone()) else {
            self.seerr_gone(cx);
            return;
        };
        let account = secret_field::account(&server.server().id, server.user_id(), SEERR_KEY);
        let (store, open): (Arc<dyn TokenStore>, _) =
            (self.secrets.clone(), self.open_seerr.clone());
        let viewer = server.user_id().to_string();
        let task = cx.background_executor().spawn(async move {
            let key = store.load(&account).ok().flatten()?;
            let client = open(&address, key, &viewer).ok()?;
            // Whom the requests go as, said in the Settings when it is not the viewer.
            let check = client.check();
            Some((client, check))
        });
        cx.spawn(async move |this, cx| {
            let made = task.await;
            this.update(cx, |shell, cx| {
                if shell.seerr.making != making {
                    return;
                }
                match made {
                    Some((client, check)) => {
                        shell.seerr.client = Some(client);
                        shell.seerr_checked(check);
                        shell.seerr_connected(cx);
                    }
                    None => shell.seerr_gone(cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The client is there: what needs it can be read.
    fn seerr_connected(&mut self, _cx: &mut Context<Self>) {}

    /// No client any more: nothing of it shows.
    fn seerr_gone(&mut self, cx: &mut Context<Self>) {
        self.seerr.client = None;
        cx.notify();
    }

    fn seerr_checked(&mut self, check: Result<ServerCheck, RequestError>) {
        match check {
            Ok(check) => {
                let (version, owner) = crate::discover::check_lines(&check);
                self.seerr.check = Some(Loadable::Ready(version));
                self.seerr.owner_line = owner;
            }
            Err(error) => {
                self.seerr.check = Some(Loadable::Failed(crate::discover::error_line(&error)));
                self.seerr.owner_line = None;
            }
        }
    }

    /// The Settings' "Test": the version, or why the connection failed.
    pub fn test_seerr(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.seerr.client.clone() else {
            self.seerr.check = Some(Loadable::Failed("Enter the address and the API key first."));
            cx.notify();
            return;
        };
        self.seerr.check = Some(Loadable::Loading);
        let making = self.seerr.making;
        let task = cx
            .background_executor()
            .spawn(async move { client.check() });
        cx.spawn(async move |this, cx| {
            let check = task.await;
            this.update(cx, |shell, cx| {
                if shell.seerr.making == making {
                    shell.seerr_checked(check);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

#[cfg(test)]
pub(super) mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use bobine::provider::{Connector, TokenStore};
    use bobine::requests::RequestError;
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::config::Settings;
    use crate::fake::{FakeServer, MemoryKeyring};
    use crate::fake_seerr::FakeSeerr;

    pub const ADDRESS: &str = "https://seerr.example.org";
    pub const KEY: &str = "k3y-seerr-0123456789abcdef";
    pub const ACCOUNT: &str = "setting/server-1/user-1/seerr-api-key";

    /// A shell on a kept session, its settings at `path`, with the fake request server behind
    /// whatever is configured.
    pub fn start(
        path: Option<PathBuf>,
        keyring: &Arc<MemoryKeyring>,
        seerr: &FakeSeerr,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Shell>, FakeServer) {
        let server = FakeServer::new(6, true);
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let secrets: Arc<dyn TokenStore> = keyring.clone();
        let window = cx.add_window(|_, cx| {
            Shell::new(None, Box::new(move || Ok(connector)), path, secrets, cx)
        });
        let fake = seerr.clone();
        window
            .update(cx, |shell, _, _| {
                shell.open_seerr = Arc::new(move |address, key, viewer| {
                    assert_eq!(address, ADDRESS);
                    assert_eq!(key.expose_secret(), KEY);
                    assert_eq!(viewer, crate::fake::USER_ID);
                    Ok(Arc::new(fake.clone()) as Arc<dyn RequestServer>)
                })
            })
            .unwrap();
        cx.run_until_parked();
        (window, server)
    }

    /// A shell where the request server is configured: its address in the settings, its key in
    /// the keyring.
    pub fn configured(seerr: &FakeSeerr, cx: &mut TestAppContext) -> WindowHandle<Shell> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.keep().join("settings.toml");
        Settings {
            seerr_address: Some(ADDRESS.into()),
            ..Settings::default()
        }
        .save(&path)
        .unwrap();
        let keyring = Arc::new(MemoryKeyring::default());
        keyring.save(ACCOUNT, &Token::new(KEY.into())).unwrap();
        start(Some(path), &keyring, seerr, cx).0
    }

    pub fn drawn(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> VisualTestContext {
        let visual = VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        visual
    }

    pub fn click(visual: &mut VisualTestContext, selector: &'static str) {
        let at = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();
    }

    fn connected(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> bool {
        window
            .update(cx, |shell, _, _| shell.seerr.client.is_some())
            .unwrap()
    }

    #[gpui_kit::test]
    fn not_configured_only_the_integrations_section_shows_with_its_empty_fields(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        let (window, _) = start(None, &Arc::new(MemoryKeyring::default()), &seerr, cx);
        assert!(!connected(&window, cx));
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.seerr.address.as_ref().unwrap().read(cx).text(), "");
                assert!(!shell.seerr.key.as_ref().unwrap().read(cx).is_saved());
                shell.open_settings(cx);
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("seerr-address").is_some());
        assert!(visual.debug_bounds("seerr-owner").is_none());
        assert_eq!(seerr.state.lock().unwrap().record.checks, 0);
    }

    #[gpui_kit::test]
    fn the_address_is_kept_in_the_settings_and_the_key_in_the_keyring_only(
        cx: &mut TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let keyring = Arc::new(MemoryKeyring::default());
        let seerr = FakeSeerr::default();
        let (window, _) = start(Some(path.clone()), &keyring, &seerr, cx);
        let key = window
            .update(cx, |shell, _, cx| {
                let address = shell.seerr.address.clone().unwrap();
                address.update(cx, |field, cx| field.set_text(ADDRESS, cx));
                shell.save_seerr_address(cx);
                shell.seerr.key.clone().unwrap()
            })
            .unwrap();
        assert!(!connected(&window, cx), "no key yet");
        key.update(cx, |field, cx| {
            field.type_in(KEY, cx);
            field.save(cx);
        });
        cx.run_until_parked();
        assert!(connected(&window, cx));
        assert_eq!(keyring.secret(ACCOUNT).as_deref(), Some(KEY));
        assert_eq!(
            Settings::read(&path).seerr_address.as_deref(),
            Some(ADDRESS)
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains(KEY), "{text}");
        // Clearing the key takes the client away.
        key.update(cx, |field, cx| field.clear(cx));
        cx.run_until_parked();
        assert!(!connected(&window, cx));
    }

    #[gpui_kit::test]
    fn a_configured_connection_is_tested_and_shows_its_version(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        let window = configured(&seerr, cx);
        assert!(connected(&window, cx));
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "seerr-test");
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    &shell.seerr.check,
                    Some(Loadable::Ready(line)) if line == "Connected to Seerr 2.7.3."
                ));
            })
            .unwrap();
        assert!(visual.debug_bounds("seerr-check").is_some());
    }

    #[gpui_kit::test]
    fn a_wrong_key_says_so(cx: &mut TestAppContext) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().failure = Some(RequestError::KeyRefused);
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.test_seerr(cx))
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, _| {
                assert!(matches!(
                    shell.seerr.check,
                    Some(Loadable::Failed("Seerr refused the API key."))
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn without_a_seerr_user_for_the_viewer_the_settings_say_requests_go_as_the_owner(
        cx: &mut TestAppContext,
    ) {
        let seerr = FakeSeerr::default();
        seerr.state.lock().unwrap().no_viewer = true;
        let window = configured(&seerr, cx);
        window
            .update(cx, |shell, _, cx| shell.open_settings(cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("seerr-owner").is_some());
        window
            .update(cx, |shell, _, _| {
                assert_eq!(
                    shell.seerr.owner_line.as_deref(),
                    Some("No Seerr user has your Jellyfin account: requests go as Owner.")
                );
            })
            .unwrap();
    }
}
