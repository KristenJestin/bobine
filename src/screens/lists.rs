//! The Collections and Ma liste screens: a grid of posters like the libraries', over a list the
//! program already holds whole. Only the rows in view are built, so only their images are asked.

use std::ops::Range;

use bobine::provider::{CollectionCard, Favourite};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, ElementId, FontWeight, IntoElement, Stateful, UniformListScrollHandle,
    Window, div, px, uniform_list,
};

use super::{Corner, centered, note, poster, poster_caption, poster_image, series_poster};
use crate::grid::{self, GAP, MARGIN};
use crate::icons::{IconName, icon};
use crate::images;
use crate::lists;
use crate::shell::{Loadable, Shell};
use crate::theme;

/// What tells the two screens apart.
struct Cells {
    title: &'static str,
    /// How many there are, once known (`12 collections`).
    count: Option<String>,
    /// What stands in place of the grid: the wait, a failure, or an empty list.
    status: Option<&'static str>,
    len: usize,
    scroll: UniformListScrollHandle,
    /// The cell at an index, `poster_width` wide.
    cell: fn(&mut Shell, usize, f32, &mut Context<Shell>) -> Option<AnyElement>,
}

pub fn render_collections(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (len, status, count) = match &shell.collections.cards {
        Some(Loadable::Ready(cards)) if cards.is_empty() => (0, Some(lists::NO_COLLECTION), None),
        Some(Loadable::Ready(cards)) => (cards.len(), None, Some(lists::collections(cards.len()))),
        Some(Loadable::Failed(why)) => (0, Some(*why), None),
        _ => (0, Some("Chargement…"), None),
    };
    grid_screen(
        Cells {
            title: "Collections",
            count,
            status,
            len,
            scroll: shell.collections.scroll.clone(),
            cell: |shell, index, width, cx| {
                let Some(Loadable::Ready(cards)) = &shell.collections.cards else {
                    return None;
                };
                let card = cards.get(index)?.clone();
                Some(collection_poster(shell, &card, width, index, cx).into_any_element())
            },
        },
        window,
        cx,
    )
}

pub fn render_my_list(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (len, status, count) = match &shell.my_list.items {
        Some(Loadable::Ready(items)) if items.is_empty() => {
            return empty_list().into_any_element();
        }
        Some(Loadable::Ready(items)) => (items.len(), None, Some(grid::count(items.len() as u32))),
        Some(Loadable::Failed(why)) => (0, Some(*why), None),
        _ => (0, Some("Chargement…"), None),
    };
    grid_screen(
        Cells {
            title: "Ma liste",
            count,
            status,
            len,
            scroll: shell.my_list.scroll.clone(),
            cell: |shell, index, width, cx| {
                let Some(Loadable::Ready(items)) = &shell.my_list.items else {
                    return None;
                };
                Some(match items.get(index)?.clone() {
                    Favourite::Film(film) => {
                        poster(shell, &film, width, ("list-film", index), false, cx)
                            .into_any_element()
                    }
                    Favourite::Series(series) => {
                        series_poster(shell, &series, width, ("list-series", index), cx)
                            .into_any_element()
                    }
                })
            },
        },
        window,
        cx,
    )
    .into_any_element()
}

/// The list holds nothing: where its titles come from.
fn empty_list() -> impl IntoElement {
    centered()
        .text_color(theme::dim())
        .child(icon(IconName::Plus, 40.))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child("Ma liste"),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_size(px(14.))
                .line_height(px(22.))
                .text_center()
                .child(lists::EMPTY_LIST),
        )
}

fn grid_screen(cells: Cells, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let width = f32::from(window.viewport_size().width);
    let columns = grid::columns(width);
    let poster_width = (width - 2. * MARGIN - GAP * (columns - 1) as f32) / columns as f32;
    let Cells {
        title,
        count,
        status,
        len,
        scroll,
        cell,
    } = cells;

    let list = uniform_list(
        title,
        len.div_ceil(columns),
        cx.processor(move |shell: &mut Shell, range: Range<usize>, _, cx| {
            range
                .map(|row| {
                    let items = (row * columns..(row + 1) * columns).map(|index| {
                        cell(shell, index, poster_width, cx)
                            // Past the last one: the same room, empty.
                            .unwrap_or_else(|| {
                                div().w(px(poster_width)).flex_none().into_any_element()
                            })
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
                .children(count.map(|count| {
                    div()
                        .text_size(px(14.))
                        .text_color(theme::dim())
                        .child(count)
                })),
        )
        .children(status.map(|why| div().px(px(MARGIN)).child(note(why))))
        .child(list)
}

/// A collection as a poster: its image and its name, with how many films it holds below.
fn collection_poster(
    shell: &mut Shell,
    card: &CollectionCard,
    width: f32,
    index: usize,
    cx: &mut Context<Shell>,
) -> Stateful<gpui_kit::Div> {
    let image = shell.image(card.poster.as_ref(), images::POSTER, cx);
    let opened = card.id.clone();
    let id: ElementId = ("collection", index).into();
    div()
        .id(id)
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| {
            shell.open_collection(opened.clone(), cx);
        }))
        .child(poster_image(image, None, Corner::Nothing, width, false))
        .child(poster_caption(card.name.clone(), lists::card_caption(card)))
}
