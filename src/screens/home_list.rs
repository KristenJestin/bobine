//! The full list behind a row's "Tout voir": a grid of its cards, the way the libraries show
//! theirs. Only the rows in view are built, and only their pages are asked of the server.

use std::ops::Range;
use std::rc::Rc;

use bobine::provider::Title;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, IntoElement, SharedString, Window, div, px, uniform_list,
};

use super::home::{error_pill, wide_card};
use super::{centered, note, poster, series_poster};
use crate::grid::{self, GAP, MARGIN};
use crate::home_page::ListKind;
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
        (_, None) => Some("Chargement…"),
        (_, Some(0)) => Some("Rien dans cette liste."),
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
                    let cards = (row * columns..(row + 1) * columns).map(|index| {
                        let found = shell
                            .list
                            .as_ref()
                            .and_then(|list| list.grid.film(index).cloned());
                        match found {
                            Some(title) => card(shell, &title, card_width, index, wide, window, cx),
                            // Not here yet, or past the last one: the same room, empty.
                            None => div().w(px(card_width)).flex_none().into_any_element(),
                        }
                    });
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

fn card(
    shell: &mut Shell,
    title: &Title,
    width: f32,
    index: usize,
    wide: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let id = (SharedString::from("home-list-card"), index);
    let alone = || Rc::<[Title]>::from([title.clone()]);
    match title {
        _ if wide => wide_card(shell, &alone(), 0, width, id, true, window, cx).into_any_element(),
        Title::Film(film) => poster(shell, film, width, id, false, cx).into_any_element(),
        Title::Series(series) => series_poster(shell, series, width, id, cx).into_any_element(),
        Title::Episode(_) => {
            wide_card(shell, &alone(), 0, width, id, true, window, cx).into_any_element()
        }
    }
}
