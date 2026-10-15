use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, div, px};

use super::{centered, heading};
use crate::navigation::Screen;
use crate::shell::Shell;
use crate::theme;

pub fn render(cx: &mut Context<Shell>) -> impl IntoElement {
    centered()
        .child(heading("Films"))
        .child(
            div()
                .flex()
                .gap(px(6.))
                .child(theme::chip("Genre"))
                .child(theme::chip("Année"))
                .child(theme::chip("Non vus"))
                .child(theme::chip("Tri")),
        )
        .child(
            // Stands in for a poster of the grid of a later issue.
            theme::ghost_button("library-open-film", "Ouvrir une fiche").on_click(cx.listener(
                |app, _, _, cx| {
                    app.navigator.open(Screen::FilmDetail);
                    cx.notify();
                },
            )),
        )
}
