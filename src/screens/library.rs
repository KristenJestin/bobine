//! The Films library: a grid of identical posters, the newest first. Only the rows in view are
//! built, and only their pages of films are asked of the server: thousands of titles scroll as
//! smoothly as a dozen.

use gpui_kit::prelude::*;
use gpui_kit::{Context, FontWeight, IntoElement, Window, div, px, uniform_list};

use super::{note, poster};
use crate::grid::{self, GAP, MARGIN};
use crate::shell::{Loadable, Shell};
use crate::theme;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let width = f32::from(window.viewport_size().width);
    let columns = grid::columns(width);
    let poster_width = (width - 2. * MARGIN - GAP * (columns - 1) as f32) / columns as f32;
    let rows = shell.library.grid.rows(columns);
    let status = match &shell.library.library {
        Some(Loadable::Failed(why)) => Some(*why),
        _ if shell.library.grid.total.is_none() => Some("Chargement…"),
        _ if rows == 0 => Some("Aucun film dans cette bibliothèque."),
        _ => None,
    };

    let list = uniform_list(
        "films",
        rows,
        cx.processor(
            move |shell: &mut Shell, range: std::ops::Range<usize>, _, cx| {
                shell.ask_films(range.start * columns..range.end * columns, cx);
                range
                    .map(|row| {
                        let films = (row * columns..(row + 1) * columns).map(|index| {
                            match shell.library.grid.film(index).cloned() {
                                Some(film) => {
                                    poster(shell, &film, poster_width, ("film", index), false, cx)
                                        .into_any_element()
                                }
                                // Not here yet, or past the last film: the same room, empty.
                                None => div().w(px(poster_width)).flex_none().into_any_element(),
                            }
                        });
                        div()
                            .flex()
                            .gap(px(GAP))
                            .px(px(MARGIN))
                            .pb(px(28.))
                            .children(films)
                    })
                    .collect()
            },
        ),
    )
    .track_scroll(&shell.library.scroll)
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
                        .child("Films"),
                )
                .children(shell.library.grid.total.map(|total| {
                    div()
                        .text_size(px(14.))
                        .text_color(theme::dim())
                        .child(grid::count(total))
                })),
        )
        .children(status.map(|why| div().px(px(MARGIN)).child(note(why))))
        .child(list)
}
