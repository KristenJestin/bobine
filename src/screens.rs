//! The screens. Empty for now: each one says what it is and offers the way to the next, so the
//! navigation can be tried. The issues that fill them replace the body of their module.

mod film_detail;
mod home;
mod library;
mod login;
mod player;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, IntoElement, div, px};

use crate::navigation::Screen;
use crate::shell::Shell;
use crate::theme;

pub fn render(screen: Screen, cx: &mut Context<Shell>) -> AnyElement {
    match screen {
        Screen::Login => login::render(cx).into_any_element(),
        Screen::Home => home::render(cx).into_any_element(),
        Screen::Library => library::render(cx).into_any_element(),
        Screen::FilmDetail => film_detail::render(cx).into_any_element(),
        Screen::Player => player::render(cx).into_any_element(),
    }
}

/// The frame of a screen that is not built yet: whatever is added is centred.
fn centered() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.))
}

/// The whole body of a screen with nothing else to offer yet.
fn empty_screen(title: &'static str) -> impl IntoElement {
    centered().child(heading(title))
}

/// Its name and a note saying so.
fn heading(title: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(theme::dim())
                .child(div().size(px(7.)).rounded_full().bg(theme::accent()))
                .child("Cet écran est vide pour l'instant."),
        )
}
