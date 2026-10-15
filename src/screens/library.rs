//! The Films and Séries libraries: a grid of identical posters, the newest first. Only the rows
//! in view are built, and only their pages are asked of the server: thousands of titles scroll
//! as smoothly as a dozen.

use std::ops::Range;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, FontWeight, IntoElement, Window, div, px, uniform_list};

use super::{note, poster, series_poster};
use crate::grid::{self, GAP, Listed, MARGIN};
use crate::shell::LibraryPage;
use crate::shell::{Loadable, Shell};
use crate::theme;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    grid_screen(
        shell,
        Grid {
            title: "Films",
            empty: "Aucun film dans cette bibliothèque.",
            page: |shell| &mut shell.library,
            ask: Shell::ask_films,
            cell: |shell, film, width, index, cx| {
                poster(shell, film, width, ("film", index), false, cx).into_any_element()
            },
        },
        window,
        cx,
    )
}

pub fn render_series(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    grid_screen(
        shell,
        Grid {
            title: "Séries",
            empty: "Aucune série dans cette bibliothèque.",
            page: |shell| &mut shell.series,
            ask: Shell::ask_series,
            cell: |shell, series, width, index, cx| {
                series_poster(shell, series, width, ("series", index), cx).into_any_element()
            },
        },
        window,
        cx,
    )
}

/// What tells the two grids apart.
struct Grid<T: 'static> {
    title: &'static str,
    empty: &'static str,
    page: fn(&mut Shell) -> &mut LibraryPage<T>,
    ask: fn(&mut Shell, Range<usize>, &mut Context<Shell>),
    cell: fn(&mut Shell, &T, f32, usize, &mut Context<Shell>) -> AnyElement,
}

fn grid_screen<T: Listed + 'static>(
    shell: &mut Shell,
    kind: Grid<T>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let width = f32::from(window.viewport_size().width);
    let columns = grid::columns(width);
    let poster_width = (width - 2. * MARGIN - GAP * (columns - 1) as f32) / columns as f32;
    let Grid {
        title,
        empty,
        page,
        ask,
        cell,
    } = kind;
    let shown = page(shell);
    let rows = shown.grid.rows(columns);
    let total = shown.grid.total;
    let status = match &shown.library {
        Some(Loadable::Failed(why)) => Some(*why),
        _ if total.is_none() => Some("Chargement…"),
        _ if rows == 0 => Some(empty),
        _ => None,
    };
    let scroll = shown.scroll.clone();

    let list = uniform_list(
        title,
        rows,
        cx.processor(move |shell: &mut Shell, range: Range<usize>, _, cx| {
            ask(shell, range.start * columns..range.end * columns, cx);
            range
                .map(|row| {
                    let items = (row * columns..(row + 1) * columns).map(|index| {
                        match page(shell).grid.film(index).cloned() {
                            Some(item) => cell(shell, &item, poster_width, index, cx),
                            // Not here yet, or past the last one: the same room, empty.
                            None => div().w(px(poster_width)).flex_none().into_any_element(),
                        }
                    });
                    div()
                        .flex()
                        .gap(px(GAP))
                        .px(px(MARGIN))
                        .pb(px(28.))
                        .children(items)
                })
                .collect()
        }),
    )
    .track_scroll(&scroll)
    .flex_1();

    div()
        .size_full()
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
                        .child(title),
                )
                .children(total.map(|total| {
                    div()
                        .text_size(px(14.))
                        .text_color(theme::dim())
                        .child(grid::count(total))
                })),
        )
        .children(status.map(|why| div().px(px(MARGIN)).child(note(why))))
        .child(list)
}
