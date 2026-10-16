//! The full list behind a row's "Tout voir": a grid of its cards, the way the libraries show
//! theirs. Only the rows in view are built, and only their pages are asked of the server.

use std::ops::Range;

use bobine::provider::Title;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, Window, div, px, uniform_list,
};

use super::home::{Pick, card_picture, error_pill, wide_card};
use super::{centered, note, poster_with, series_poster_with};
use crate::grid::{self, GAP, MARGIN};
use crate::home_page::ListKind;
use crate::images;
use crate::shell::Shell;
use crate::theme;

/// The narrowest a wide card gets before a column goes.
const MIN_WIDE: f32 = 280.;
const MAX_WIDE_COLUMNS: usize = 5;

/// How many wide cards fit across `width`, two at least.
fn wide_columns(width: f32) -> usize {
    let room = width - 2. * MARGIN + GAP;
    ((room / (MIN_WIDE + GAP)).floor() as usize).clamp(2, MAX_WIDE_COLUMNS)
}

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(list) = &shell.list else {
        return centered().into_any_element();
    };
    let wide = !matches!(list.kind, ListKind::Latest(..));
    let width = f32::from(window.viewport_size().width);
    let columns = if wide {
        wide_columns(width)
    } else {
        grid::columns(width)
    };
    let card_width = (width - 2. * MARGIN - GAP * (columns - 1) as f32) / columns as f32;
    let heading = list.kind.heading();
    let library = list.library.clone();
    let total = list.grid.total;
    let rows = list.grid.rows(columns);
    let status = match (list.error, total) {
        (Some(why), None) => Some(why),
        (_, None) => Some("Loading…"),
        (_, Some(0)) => Some("Nothing in this list."),
        _ => None,
    };
    let scroll = list.scroll.clone();
    let error = shell.home.error;

    let rows_list = uniform_list(
        "home-list",
        rows,
        cx.processor(move |shell: &mut Shell, range: Range<usize>, window, cx| {
            shell.ask_list(range.start * columns..range.end * columns, cx);
            range
                .map(|row| {
                    let cards = (row * columns..(row + 1) * columns)
                        .map(|index| card(shell, card_width, index, wide, window, cx));
                    div()
                        .flex()
                        .gap(px(GAP))
                        .px(px(MARGIN))
                        .pb(px(28.))
                        .children(cards)
                })
                .collect()
        }),
    )
    .track_scroll(&scroll)
    .flex_1();

    div()
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .pt(px(96.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(14.))
                .px(px(MARGIN))
                .pb(px(24.))
                .child(
                    div()
                        .text_size(px(28.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(heading),
                )
                .children(library.map(|name| {
                    div()
                        .text_size(px(14.))
                        .text_color(theme::dim())
                        .child(name)
                }))
                .children(total.filter(|total| *total > 0).map(|total| {
                    div()
                        .text_size(px(14.))
                        .text_color(theme::dim())
                        .child(grid::count(total))
                })),
        )
        .children(status.map(|why| div().px(px(MARGIN)).child(note(why))))
        .child(rows_list)
        .children(error.map(error_pill))
        .into_any_element()
}

fn listed(shell: &Shell, index: usize) -> Option<&Title> {
    shell.list.as_ref()?.grid.film(index)
}

/// The `index`th card of the list. Neither a wide card nor a poster copies its title: the
/// picture is asked first, then the title is read again where it stands in the list to draw.
fn card(
    shell: &mut Shell,
    width: f32,
    index: usize,
    wide: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let id = (SharedString::from("home-list-card"), index);
    match listed(shell, index) {
        // Not here yet, or past the last one: the same room, empty.
        None => div().w(px(width)).flex_none().into_any_element(),
        Some(title @ (Title::Film(_) | Title::Series(_))) if !wide => {
            let picture = match title {
                Title::Film(film) => film.poster.clone(),
                Title::Series(series) => series.poster.clone(),
                Title::Episode(_) => None,
            };
            let image = shell.image(picture.as_ref(), images::POSTER, cx);
            // Asked first, so that the title is borrowed from the shell only to draw.
            match listed(shell, index).expect("the card was just found") {
                Title::Film(film) => poster_with(film, image, width, id, cx).into_any_element(),
                Title::Series(series) => {
                    series_poster_with(series, image, width, id, cx).into_any_element()
                }
                Title::Episode(_) => unreachable!("a poster is for a film or a series"),
            }
        }
        Some(title) => {
            let picture = card_picture(title).cloned();
            let image = shell.image(picture.as_ref(), images::CARD, cx);
            let title = listed(shell, index).expect("the card was just found");
            wide_card(
                title,
                image,
                Pick::Listed(index, title.id().clone()),
                width,
                id,
                window,
                cx,
            )
            .into_any_element()
        }
    }
}
