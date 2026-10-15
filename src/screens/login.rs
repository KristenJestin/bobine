use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, div, px};

use super::{centered, heading};
use crate::chrome::logo;
use crate::navigation::Screen;
use crate::shell::Shell;
use crate::theme;

pub fn render(cx: &mut Context<Shell>) -> impl IntoElement {
    centered()
        .bg(theme::panel())
        .child(logo(56., 24., 15.))
        .child(heading("Connexion"))
        .child(div().mt(px(10.)).child(
            // Stands in for the real login (address, user, password) of a later issue.
            theme::primary_button("login-continue", "Continuer").on_click(cx.listener(
                |app, _, _, cx| {
                    app.navigator.switch_root(Screen::Home);
                    cx.notify();
                },
            )),
        ))
}
