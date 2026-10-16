//! The login: the server's address (Bobine checks it is a Jellyfin server and says its name and
//! version), the user, the password. Enter signs in; Tab goes to the next field.

use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, Focusable, FontWeight, IntoElement, KeyDownEvent, Window, div, px};

use super::centered;
use crate::chrome::logo;
use crate::shell::Shell;
use crate::theme;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(form) = &shell.login else {
        return centered();
    };
    let server_line = match &form.server {
        Some(Ok(line)) => Some((line.clone(), theme::accent(), theme::muted())),
        Some(Err(why)) => Some((why.to_string(), theme::danger(), theme::danger())),
        None => None,
    };
    let fields = [
        form.address.clone(),
        form.user.clone(),
        form.password.clone(),
    ];
    let label = if form.busy {
        "Signing in…"
    } else {
        "Sign in"
    };

    centered().bg(theme::background()).child(
        div()
            .w(px(400.))
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(32.))
            .rounded(px(theme::RADIUS_LARGE))
            .border_1()
            .border_color(theme::line())
            .bg(theme::panel())
            .on_key_down(cx.listener(move |shell, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => shell.submit_login(cx),
                    "tab" => {
                        let step = if event.keystroke.modifiers.shift {
                            2
                        } else {
                            1
                        };
                        let now = fields
                            .iter()
                            .position(|field| field.focus_handle(cx).is_focused(window))
                            .unwrap_or(0);
                        window.focus(&fields[(now + step) % 3].focus_handle(cx), cx);
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(logo(44., 20., 12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Connect a server"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .child("Start with Jellyfin."),
                    ),
            )
            .child(
                field("Server address")
                    .child(form.address.clone())
                    .children(server_line.map(|(line, dot, color)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .text_color(color)
                            .child(div().size(px(7.)).rounded_full().bg(dot))
                            .child(line)
                    })),
            )
            .child(field("Username").child(form.user.clone()))
            .child(field("Password").child(form.password.clone()))
            .children(form.error.map(|why| {
                div()
                    .text_size(px(13.))
                    .text_color(theme::danger())
                    .child(why)
            }))
            .child(
                theme::primary_button("login-submit", None, label, window, cx)
                    .flex()
                    .justify_center()
                    .when(form.busy, |button| button.opacity(0.6))
                    .on_click(cx.listener(|shell, _, _, cx| shell.submit_login(cx))),
            ),
    )
}

/// A field under its label.
fn field(label: &'static str) -> Div {
    div().flex().flex_col().gap(px(8.)).child(
        div()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::muted())
            .child(label),
    )
}
