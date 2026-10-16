//! A field for a secret (an API key): what is typed goes to the system keyring under a key name
//! and nowhere else, then the field shows dots, with a way to clear it. The value never lives in
//! the settings file, in this view's state once saved, or in a message: the field is emptied
//! before the keyring is asked, and a failure says nothing of what was typed.
//!
//! The first integration (#63) draws one; until then only the tests and the harness do.

use std::sync::Arc;

use bobine::provider::{ProviderError, Token, TokenStore};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, Context, ElementId, Entity, IntoElement, KeyDownEvent, Render, Window,
    div, px,
};

use crate::input::TextInput;
use crate::motion;
use crate::start;
use crate::theme;

/// What a saved secret shows instead of itself.
const DOTS: &str = "••••";

/// Where the keyring keeps the secret of `key`: apart from the accounts' tokens, and for one
/// user of one server, so that another user, or the same name on another server, never reads it.
/// It outlives a sign-out: the same user who signs back in finds it.
pub fn account(server_id: &str, user_id: &str, key: &str) -> String {
    format!("setting/{server_id}/{user_id}/{key}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// The keyring is being asked whether it holds the secret.
    Checking,
    Empty,
    Saved,
    /// The keyring is saving or clearing.
    Busy,
}

pub struct SecretField {
    label: &'static str,
    account: String,
    store: Arc<dyn TokenStore>,
    input: Entity<TextInput>,
    state: State,
    error: Option<&'static str>,
    /// How many times the field changed state: each change fades the new content in.
    changes: usize,
}

impl SecretField {
    /// A field for the secret kept under `account` (see `account()`). It looks in the keyring,
    /// off the window's thread, whether one is already there.
    pub fn new(
        label: &'static str,
        account: String,
        store: Arc<dyn TokenStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new("Paste it here", true, cx));
        let (asked, name) = (store.clone(), account.clone());
        let task = cx
            .background_executor()
            .spawn(async move { asked.load(&name) });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            this.update(cx, |field, cx| {
                match found {
                    Ok(found) => field.settle(found.is_some()),
                    Err(error) => {
                        field.error = Some(start::error_message(&error));
                        field.settle(false);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            label,
            account,
            store,
            input,
            state: State::Checking,
            error: None,
            changes: 0,
        }
    }

    fn settle(&mut self, saved: bool) {
        self.state = if saved { State::Saved } else { State::Empty };
        self.changes += 1;
    }

    pub fn is_saved(&self) -> bool {
        self.state == State::Saved
    }

    /// The dots of a saved secret, never the secret. #63 reads it; until then only tests do.
    #[cfg_attr(not(any(test, feature = "harness")), allow(dead_code))]
    pub fn shown(&self) -> Option<&'static str> {
        self.is_saved().then_some(DOTS)
    }

    #[cfg_attr(not(any(test, feature = "harness")), allow(dead_code))]
    pub fn error(&self) -> Option<&'static str> {
        self.error
    }

    /// Text entered as the viewer would type it.
    #[cfg(any(test, feature = "harness"))]
    pub fn type_in(&mut self, text: &str, cx: &mut Context<Self>) {
        self.input.update(cx, |input, cx| input.set_text(text, cx));
    }

    /// Keeps what was typed in the keyring. The field is emptied first: from here the value
    /// lives only in the call.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        if self.state != State::Empty {
            return;
        }
        let typed = self.input.update(cx, |input, cx| input.take(cx));
        let typed = typed.trim().to_string();
        if typed.is_empty() {
            return;
        }
        self.error = None;
        let token = Token::new(typed);
        self.work(cx, true, move |store, account| store.save(account, &token));
    }

    /// Takes the secret out of the keyring.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.state != State::Saved {
            return;
        }
        self.error = None;
        self.work(cx, false, |store, account| store.delete(account));
    }

    /// Runs `call` on the keyring off the window's thread; `saved` is what the field holds if it
    /// worked.
    fn work(
        &mut self,
        cx: &mut Context<Self>,
        saved: bool,
        call: impl FnOnce(&dyn TokenStore, &str) -> Result<(), ProviderError> + Send + 'static,
    ) {
        let before = self.state;
        self.state = State::Busy;
        let (store, account) = (self.store.clone(), self.account.clone());
        let task = cx
            .background_executor()
            .spawn(async move { call(&*store, &account) });
        cx.spawn(async move |this, cx| {
            let done = task.await;
            this.update(cx, |field, cx| {
                match done {
                    Ok(()) => field.settle(saved),
                    Err(error) => {
                        field.error = Some(start::error_message(&error));
                        field.state = before;
                        field.changes += 1;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }
}

impl Render for SecretField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = matches!(self.state, State::Busy | State::Checking);
        let control = if self.is_saved() {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme::muted())
                        .child(DOTS),
                )
                .child(
                    theme::eased(
                        ElementId::from((self.label, 1usize)),
                        theme::Shades::line(),
                        window,
                        cx,
                    )
                    .px(px(12.))
                    .py(px(7.))
                    .rounded(px(theme::RADIUS))
                    .border_1()
                    .border_color(theme::line())
                    .text_size(px(13.))
                    .when(!busy, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|field, _, _, cx| field.clear(cx)))
                    })
                    .child("Clear"),
                )
        } else {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .on_key_down(cx.listener(|field, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "enter" {
                        field.save(cx);
                        cx.stop_propagation();
                    }
                }))
                .child(div().w(px(240.)).child(self.input.clone()))
                .child(
                    theme::eased(
                        ElementId::from((self.label, 2usize)),
                        theme::Shades::line(),
                        window,
                        cx,
                    )
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(theme::RADIUS))
                    .border_1()
                    .border_color(theme::line())
                    .text_size(px(13.))
                    .when(!busy, |button| {
                        button
                            .cursor_pointer()
                            .on_click(cx.listener(|field, _, _, cx| field.save(cx)))
                    })
                    .child("Save"),
                )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .text_size(px(14.))
                    .child(self.label)
                    .child(
                        div()
                            .when(busy, |control| control.opacity(0.6))
                            .child(control)
                            .with_animation(
                                ElementId::from((self.label, self.changes)),
                                Animation::new(motion::quick()).with_easing(motion::ease_out),
                                |control, t| control.opacity(t),
                            ),
                    ),
            )
            .children(self.error.map(|why| {
                div()
                    .text_size(px(12.))
                    .text_color(theme::danger())
                    .child(why)
            }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use gpui_kit::Entity;

    use super::*;
    use crate::fake::MemoryKeyring;

    fn key() -> String {
        account("server-1", "user-1", "demo-key")
    }

    fn field(
        keyring: &Arc<MemoryKeyring>,
        cx: &mut gpui_kit::TestAppContext,
    ) -> Entity<SecretField> {
        let store: Arc<dyn bobine::provider::TokenStore> = keyring.clone();
        let field = cx.new(|cx| SecretField::new("Demo key", key(), store, cx));
        cx.run_until_parked();
        field
    }

    fn type_in(field: &Entity<SecretField>, text: &str, cx: &mut gpui_kit::TestAppContext) {
        field.update(cx, |field, cx| field.type_in(text, cx));
    }

    fn save(field: &Entity<SecretField>, cx: &mut gpui_kit::TestAppContext) {
        field.update(cx, |field, cx| field.save(cx));
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn a_secret_entered_goes_to_the_keyring_and_leaves_the_field(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let keyring = Arc::new(MemoryKeyring::default());
        let field = field(&keyring, cx);
        type_in(&field, "abc-123", cx);
        save(&field, cx);
        assert_eq!(keyring.secret(&key()).as_deref(), Some("abc-123"));
        field.read_with(cx, |field, cx| {
            assert!(field.is_saved());
            assert_eq!(field.input.read(cx).text(), "", "the field keeps nothing");
        });
    }

    #[gpui_kit::test]
    fn a_saved_secret_shows_as_dots_and_never_as_itself(cx: &mut gpui_kit::TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let field = field(&keyring, cx);
        field.read_with(cx, |field, _| assert_eq!(field.shown(), None));
        type_in(&field, "abc-123", cx);
        save(&field, cx);
        field.read_with(cx, |field, _| assert_eq!(field.shown(), Some("••••")));
    }

    #[gpui_kit::test]
    fn a_secret_kept_by_a_start_before_shows_as_saved(cx: &mut gpui_kit::TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let first = field(&keyring, cx);
        type_in(&first, "abc-123", cx);
        save(&first, cx);
        let second = field(&keyring, cx);
        second.read_with(cx, |field, _| assert!(field.is_saved()));
    }

    #[gpui_kit::test]
    fn clearing_takes_the_secret_out_of_the_keyring(cx: &mut gpui_kit::TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let field = field(&keyring, cx);
        type_in(&field, "abc-123", cx);
        save(&field, cx);
        field.update(cx, |field, cx| field.clear(cx));
        cx.run_until_parked();
        assert_eq!(keyring.secret(&key()), None);
        field.read_with(cx, |field, _| assert!(!field.is_saved()));
    }

    #[gpui_kit::test]
    fn an_empty_entry_saves_nothing(cx: &mut gpui_kit::TestAppContext) {
        let keyring = Arc::new(MemoryKeyring::default());
        let field = field(&keyring, cx);
        type_in(&field, "   ", cx);
        save(&field, cx);
        assert_eq!(keyring.secret(&key()), None);
        field.read_with(cx, |field, _| assert!(!field.is_saved()));
    }

    #[gpui_kit::test]
    fn a_keyring_that_cannot_be_reached_is_said_and_keeps_nothing(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let keyring = Arc::new(MemoryKeyring::default());
        let field = field(&keyring, cx);
        keyring.unreachable.store(true, Ordering::SeqCst);
        type_in(&field, "abc-123", cx);
        save(&field, cx);
        field.read_with(cx, |field, cx| {
            assert!(!field.is_saved());
            assert_eq!(field.error(), Some("The system keyring cannot be reached."));
            assert_eq!(
                field.input.read(cx).text(),
                "",
                "the secret did not stay in the field"
            );
        });
    }

    #[test]
    fn the_secret_is_kept_apart_from_the_account_tokens_and_per_server_and_user() {
        assert_eq!(
            account("server-1", "user-1", "demo-key"),
            "setting/server-1/user-1/demo-key"
        );
    }
}
