//! What frames the screens: the floating pill bar of the root screens and the back link of the
//! detail pages.

use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, FontWeight, IntoElement, Stateful, div, px, rgba};

use crate::navigation::{NavEntry, Screen};
use crate::shell::Shell;
use crate::theme;

/// Centred at the top, over the screen. Series, Collections, Ma liste and the search are
/// visible but inert until their screens exist.
pub fn pill_bar(current: Screen, cx: &mut Context<Shell>) -> impl IntoElement {
    let entries = NavEntry::ALL.into_iter().enumerate().map(|(index, entry)| {
        let active = entry.is_current(current);
        let live = entry.target().is_some();
        let item = div()
            .id(("nav-entry", index))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(theme::RADIUS))
            .text_size(px(13.))
            .text_color(match (active, live) {
                (true, _) => theme::text(),
                (false, true) => theme::muted(),
                (false, false) => theme::dim(),
            })
            .when(active, |item| item.bg(theme::line()))
            .child(entry.label());
        if live {
            item.cursor_pointer()
                .on_click(cx.listener(move |app, _, _, cx| app.select(entry, cx)))
        } else {
            item
        }
    });

    div()
        .absolute()
        .top(px(16.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .p(px(5.))
                .rounded(px(theme::RADIUS_LARGE))
                .border_1()
                .border_color(theme::line())
                // A click on the bar never reaches what is under it.
                .occlude()
                // Opaque enough to read over a hero image: there is no blur behind it.
                .bg(rgba(0x141416ec))
                .child(logo(32., 15., 9.).mr(px(6.)))
                .children(entries)
                .child(search_field()),
        )
}

/// The Bobine mark: a letter on a tile, until there is a drawn logo.
pub fn logo(side: f32, font: f32, radius: f32) -> Div {
    div()
        .size(px(side))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius))
        .border_1()
        .border_color(theme::line())
        .bg(rgba(0x1d1d20ff))
        .text_size(px(font))
        .font_weight(FontWeight::EXTRA_BOLD)
        .child("b")
}

/// Inert: the search palette comes later.
fn search_field() -> Div {
    div()
        .ml(px(6.))
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .pl(px(10.))
        .pr(px(8.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(theme::line())
        .text_size(px(13.))
        .text_color(theme::dim())
        .child("Rechercher")
        .child(
            div()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(5.))
                .bg(theme::line())
                .text_size(px(11.))
                .text_color(theme::muted())
                .child("Ctrl K"),
        )
}

/// Top left of a detail page, where the pill bar is not: `← Films`.
pub fn back_link(label: &'static str, cx: &mut Context<Shell>) -> Stateful<Div> {
    div()
        .id("back-link")
        .absolute()
        .top(px(24.))
        .left(px(40.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(theme::RADIUS))
        .text_size(px(14.))
        .text_color(theme::muted())
        // Readable over a page scrolled under it.
        .bg(rgba(0x09090acc))
        .cursor_pointer()
        .hover(|style| style.text_color(theme::text()).bg(theme::line()))
        .on_click(cx.listener(|app, _, _, cx| {
            app.go_back(cx);
        }))
        .child(format!("← {label}"))
}
