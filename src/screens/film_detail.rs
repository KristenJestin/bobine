use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement};

use super::{centered, heading};
use crate::navigation::Screen;
use crate::shell::Shell;
use crate::theme;

pub fn render(cx: &mut Context<Shell>) -> impl IntoElement {
    centered().child(heading("Fiche du film")).child(
        theme::primary_button("film-play", "Lecture").on_click(cx.listener(|app, _, _, cx| {
            app.navigator.open(Screen::Player);
            cx.notify();
        })),
    )
}
