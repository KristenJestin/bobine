//! The Collections, Lists and Favorites screens. Collections and Favorites are a grid of
//! posters like the libraries', over a list the program already holds whole: only the rows in
//! view are built, so only their images are asked. The Lists are a row of the favorites, then
//! the playlists' cards.

use std::ops::Range;

use bobine::provider::{CollectionCard, Favourite};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, IntoElement, ScrollHandle, Stateful,
    UniformListScrollHandle, Window, div, px, uniform_list,
};

use super::film_detail::ROW_POSTER;
use super::home::suggestion_row;
use super::home_list::{empty_page, page_under_the_bar};
use super::library::heading;
use super::{Corner, note, poster, poster_caption, poster_image, row_heading, series_poster};
use crate::grid::{self, GAP, MARGIN};
use crate::home_page;
use crate::icons::{IconName, icon};
use crate::images;
use crate::lists;
use crate::playlists;
use crate::shell::{ListCard, Loadable, Shell};
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
        Some(Loadable::Ready(cards)) if cards.is_empty() => {
            let scroll = shell.collections.suggestions.clone();
            return empty(
                shell,
                "Collections",
                lists::NO_COLLECTION,
                &scroll,
                window,
                cx,
            );
        }
        Some(Loadable::Ready(cards)) => (cards.len(), None, Some(lists::collections(cards.len()))),
        Some(Loadable::Failed(why)) => (0, Some(*why), None),
        _ => (0, Some("Loading…"), None),
    };
    shell.scrolls_list(&shell.collections.scroll.clone());
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
    .into_any_element()
}

pub fn render_my_list(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (len, status, count) = match &shell.my_list.items {
        Some(Loadable::Ready(items)) if items.is_empty() => {
            let scroll = shell.my_list.suggestions.clone();
            return empty(shell, "Favorites", lists::EMPTY_LIST, &scroll, window, cx);
        }
        Some(Loadable::Ready(items)) => (items.len(), None, Some(grid::count(items.len() as u32))),
        Some(Loadable::Failed(why)) => (0, Some(*why), None),
        _ => (0, Some("Loading…"), None),
    };
    shell.scrolls_list(&shell.my_list.scroll.clone());
    grid_screen(
        Cells {
            title: "Favorites",
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

/// The Lists: the favorites first, as a row of posters with "See all", then a card for each
/// playlist. A user has a few playlists: their cards are all built.
pub fn render_lists(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let width = f32::from(window.viewport_size().width);
    let columns = grid::columns(width);
    let card_width = (width - 2. * MARGIN - GAP * (columns - 1) as f32) / columns as f32;

    let (favorites, count) = match &shell.my_list.items {
        Some(Loadable::Ready(items)) if items.is_empty() => (Err(lists::EMPTY_LIST), None),
        Some(Loadable::Ready(items)) => (
            Ok(items
                .iter()
                .take(home_page::ROW as usize)
                .cloned()
                .collect()),
            Some(grid::count(items.len() as u32)),
        ),
        Some(Loadable::Failed(why)) => (Err(*why), None),
        _ => (Err("Loading…"), None),
    };
    let see_all = favorites.is_ok().then(|| {
        theme::eased("lists-see-all", theme::Shades::veil(), window, cx)
            .debug_selector(|| "lists-see-all".into())
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(10.))
            .py(px(5.))
            .rounded(px(theme::RADIUS))
            .text_size(px(13.))
            .text_color(theme::muted())
            .cursor_pointer()
            .on_click(cx.listener(|shell, _, _, cx| shell.open_favorites(cx)))
            .child("See all")
            .child(icon(IconName::ArrowRight, 14.))
    });
    shell
        .scrolls
        .extend([shell.lists.favorites.clone(), shell.lists.scroll.clone()]);
    let favorites: AnyElement = match favorites {
        Ok(items) => {
            let items: Vec<Favourite> = items;
            let posters: Vec<AnyElement> = items
                .iter()
                .enumerate()
                .map(|(at, favourite)| match favourite {
                    Favourite::Film(film) => {
                        poster(shell, film, ROW_POSTER, ("lists-film", at), false, cx)
                            .into_any_element()
                    }
                    Favourite::Series(series) => {
                        series_poster(shell, series, ROW_POSTER, ("lists-series", at), cx)
                            .into_any_element()
                    }
                })
                .collect();
            div()
                .id("lists-favorites")
                .debug_selector(|| "lists-favorites".into())
                .flex()
                .gap(px(GAP))
                .px(px(MARGIN))
                .overflow_x_scroll()
                .track_scroll(&shell.lists.favorites)
                .children(posters)
                .into_any_element()
        }
        Err(why) => div().px(px(MARGIN)).child(note(why)).into_any_element(),
    };

    let cards: Vec<AnyElement> = shell
        .lists
        .cards()
        .into_iter()
        .enumerate()
        .map(|(index, card)| list_card(shell, card, card_width, index, cx).into_any_element())
        .collect();
    let playlists = match &shell.lists.playlists {
        Some(Loadable::Failed(why)) => div().px(px(MARGIN)).child(note(*why)),
        Some(Loadable::Ready(_)) if cards.is_empty() => {
            div().px(px(MARGIN)).child(note(playlists::NONE))
        }
        Some(Loadable::Ready(_)) => div()
            .flex()
            .flex_wrap()
            .gap(px(GAP))
            .px(px(MARGIN))
            .children(cards),
        _ => div().px(px(MARGIN)).child(note("Loading…")),
    };

    div()
        .id("lists")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&shell.lists.scroll)
        .pt(px(96.))
        .pb(px(48.))
        .flex()
        .flex_col()
        .child(
            div()
                .flex_none()
                .px(px(MARGIN))
                .pb(px(24.))
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Lists"),
        )
        .child(section("Favorites", count, see_all))
        // Taller than the window, the page scrolls: nothing in it shrinks.
        .child(div().flex_none().child(favorites))
        .child(div().flex_none().h(px(36.)))
        .child(section("Playlists", None, None::<Div>))
        .child(playlists.flex_none())
}

/// The heading of a section of the Lists, with its count and its "See all".
fn section(title: &'static str, count: Option<String>, see_all: Option<impl IntoElement>) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .px(px(MARGIN - 10.))
        .pb(px(12.))
        .child(
            div()
                .pl(px(10.))
                .flex()
                .items_baseline()
                .gap(px(12.))
                .child(row_heading(title))
                .children(count.map(|count| {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::dim())
                        .child(count)
                })),
        )
        .children(see_all)
}

/// A playlist as a poster: a mosaic of its first posters, its name and its count.
fn list_card(
    shell: &mut Shell,
    card: ListCard,
    width: f32,
    index: usize,
    cx: &mut Context<Shell>,
) -> Stateful<gpui_kit::Div> {
    let id: ElementId = ("list-card", index).into();
    let height = width * 1.5;
    let named = card.name.clone();
    let places = playlists::mosaic_cells(card.posters.len(), width, height, MOSAIC_GAP);
    let cells: Vec<AnyElement> = places
        .iter()
        .enumerate()
        .map(|(at, &[x, y, w, h])| {
            // Past the posters, the fill of a card of three.
            let image = card
                .posters
                .get(at)
                .and_then(|poster| shell.image(Some(poster), images::POSTER, cx));
            let named = named.clone();
            div()
                .debug_selector(move || format!("list-mosaic {named} {at}"))
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(w))
                .h(px(h))
                .bg(theme::line())
                .children(super::picture(image, 0.))
                .into_any_element()
        })
        .collect();
    let empty = cells.is_empty();
    let opens = card.id.clone();
    let name = card.name.clone();
    div()
        .id(id)
        .debug_selector(move || format!("list-card {name}"))
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| shell.open_playlist(opens.clone(), cx)))
        .child(
            div()
                .debug_selector({
                    let named = named.clone();
                    move || format!("list-mosaic {named}")
                })
                .relative()
                .w(px(width))
                .h(px(height))
                .rounded(px(theme::RADIUS))
                .overflow_hidden()
                .bg(theme::panel())
                .flex()
                .when(empty, |frame| {
                    frame
                        .items_center()
                        .justify_center()
                        .text_color(theme::dim())
                        .child(icon(IconName::List, 32.))
                })
                .children(cells),
        )
        .child(poster_caption(card.name, card.caption))
}

/// Between the posters of a list's mosaic.
const MOSAIC_GAP: f32 = 2.;

/// The page holds nothing: the heading, why, and a row of the Home to watch instead, drawn
/// like the empty full lists.
fn empty(
    shell: &mut Shell,
    title: &'static str,
    reason: &'static str,
    scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let width = f32::from(window.viewport_size().width);
    let row = suggestion_row(shell, None, scroll, width, window, cx);
    let page = page_under_the_bar().child(heading(title, None, None));
    empty_page(page, reason, row)
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
        .id(id.clone())
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| {
            shell.open_collection(opened.clone(), cx);
        }))
        .child(poster_image(
            &id,
            image,
            None,
            Corner::Nothing,
            width,
            false,
        ))
        .child(poster_caption(card.name.clone(), lists::card_caption(card)))
}
