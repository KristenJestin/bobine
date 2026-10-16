//! The Films and Séries libraries: a grid of identical posters, the newest first. Only the rows
//! in view are built, and only their pages are asked of the server: thousands of titles scroll
//! as smoothly as a dozen.

use std::ops::Range;
use std::time::Instant;

use bobine::provider::FilterChoices;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, IntoElement, ScrollStrategy, Stateful, Window, div, px,
    rgb, rgba, uniform_list,
};

use super::film_detail::{Actions, ChipBounds, MenuState, chip};
use super::{note, poster, series_poster};
use crate::grid::{self, GAP, Listed, MARGIN};
use crate::icons::{IconName, icon};
use crate::library_tools::{
    LETTERS, Sort, Tools, decade_label, letter_index, letter_of, letter_weight,
};
use crate::motion;
use crate::shell::LibraryPage;
use crate::shell::{GONE, Loadable, Shell, Tool};
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
    let now = Instant::now();
    let shown = page(shell);
    // The list on screen is the old one while the new one is awaited and the old one fades out.
    shown.follow_list(now);
    let leaving = shown.grid.has_stale();
    let rows = shown.grid.shown_rows(columns);
    let total = shown.grid.shown_total();
    let count = shown.header_count();
    if let Some(index) = shown.take_jump() {
        shown
            .scroll
            .scroll_to_item_strict(index / columns, ScrollStrategy::Top);
        shown.appear.jump(0.35);
        shown.appear.set(1., now);
    }
    let opacity = shown.appear.value(now);
    let cells = shown.cells_visible(now);
    // Nothing to jump in when the filters leave nothing.
    shown
        .letters
        .set(shown.tools.letters_shown() && total != Some(0), now);
    let letters = shown.letters.level(now);
    let open = shown.menus.open();
    let menu = MenuState {
        open,
        drawn: shown.menu_fade.follow(open, now),
    };
    shown.unwatched.set(shown.tools.filters.unwatched, now);
    let unwatched = shown.unwatched.level(now);
    let loading = shown.grid.total.is_none();
    let status = match &shown.library {
        Some(Loadable::Failed(why)) => Some(*why),
        _ if shown.error.is_some() => shown.error,
        // Not while the old list is still seen leaving: it says enough.
        _ if loading && (!leaving || opacity <= GONE) => Some("Chargement…"),
        _ if rows == 0 && shown.tools.narrowed() => Some(NO_MATCH),
        _ if rows == 0 => Some(empty),
        _ => None,
    };
    // The retries are used up and no list is there: the failure has a way to try again.
    let gave_up = shown.gave_up();
    let scroll = shown.scroll.clone();
    let tools = shown.tools.clone();
    let choices = match &shown.choices {
        Some(Loadable::Ready(choices)) => Some(choices.clone()),
        _ => None,
    };
    let chips = shown.chips.clone();
    // The letter the top of the grid is at, when the bar is showing.
    let top_row = {
        let state = scroll.0.borrow();
        let contents = state
            .last_item_size
            .map_or(0., |size| f32::from(size.contents.height));
        grid::top_row(-f32::from(state.base_handle.offset().y), contents, rows)
    };
    let current = shown
        .grid
        .shown_film(top_row * columns)
        .map(|first| letter_of(first.title()));
    // The highlight goes from letter to letter instead of changing at once; while the bar is
    // hidden it only keeps its place, so that it shows at the right letter and does not glide.
    if let Some(index) = current.and_then(letter_index) {
        if letters > 0. {
            shown.highlight.set(index as f32, now);
        } else {
            shown.highlight.jump(index as f32);
        }
    }
    let highlight = shown.highlight.value(now);
    if shown.appear.moving(now)
        || shown.letters.moving(now)
        || shown.menu_fade.moving(now)
        || shown.unwatched.moving(now)
        || shown.highlight.moving(now)
    {
        window.request_animation_frame();
    }

    let list = uniform_list(
        title,
        rows,
        cx.processor(move |shell: &mut Shell, range: Range<usize>, _, cx| {
            ask(shell, range.start * columns..range.end * columns, cx);
            range
                .map(|row| {
                    let items = (row * columns..(row + 1) * columns).map(|index| {
                        let item = cells
                            .then(|| page(shell).grid.shown_film(index).cloned())
                            .flatten();
                        match item {
                            Some(item) => cell(shell, &item, poster_width, index, cx),
                            // Not here yet, past the last one, or the old list gone and
                            // invisible: the same room, empty, with nothing to click.
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
        .relative()
        .flex()
        .flex_col()
        .pt(px(96.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(MARGIN))
                .pb(px(24.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(14.))
                        .child(
                            div()
                                .text_size(px(28.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .children(count.map(|total| {
                            div()
                                .text_size(px(14.))
                                .text_color(theme::dim())
                                .child(grid::count(total))
                        })),
                )
                .child(tool_chips(
                    &tools,
                    unwatched,
                    choices.as_ref(),
                    menu,
                    &chips,
                    window,
                    cx,
                )),
        )
        .children(status.map(|why| {
            div()
                .px(px(MARGIN))
                .flex()
                .items_center()
                .gap(px(14.))
                .child(note(why))
                .children(gave_up.then(|| {
                    theme::ghost_button(
                        "library-retry",
                        Some(IconName::RotateCcw),
                        "Réessayer",
                        window,
                        cx,
                    )
                    .on_click(cx.listener(|shell, _, _, cx| shell.retry_list(cx)))
                }))
        }))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .opacity(opacity)
                .child(list),
        )
        .children((letters > 0.).then(|| alphabet(highlight, letters, window, cx)))
}

/// What the grid says when the filters leave nothing.
const NO_MATCH: &str = "Aucun titre ne correspond à ces filtres.";

/// Genre, decade, unwatched only and the order, right of the title.
fn tool_chips(
    tools: &Tools,
    unwatched: f32,
    choices: Option<&FilterChoices>,
    menu: MenuState<Tool>,
    bounds: &[ChipBounds; 3],
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let genres = tools.filters.genre.as_deref();
    let genre_options = std::iter::once((None, "Tous les genres".to_string(), genres.is_none()))
        .chain(
            choices
                .into_iter()
                .flat_map(|choices| choices.genres.iter().enumerate())
                .map(|(index, genre)| {
                    (
                        Some(index as i32),
                        genre.clone(),
                        Some(genre.as_str()) == genres,
                    )
                }),
        )
        .collect();
    let decade = tools.filters.decade;
    let decade_options = std::iter::once((None, "Toutes les années".to_string(), decade.is_none()))
        .chain(
            choices
                .into_iter()
                .flat_map(|choices| choices.decades.iter())
                .map(|&d| (Some(d), decade_label(d), Some(d) == decade)),
        )
        .collect();
    let sort_options = Sort::ALL
        .iter()
        .enumerate()
        .map(|(index, sort)| {
            (
                Some(index as i32),
                sort.label().to_string(),
                *sort == tools.sort,
            )
        })
        .collect();
    let tool = |window: &mut Window,
                cx: &mut Context<Shell>,
                kind: Tool,
                label: &'static str,
                value: String,
                options| {
        chip(
            kind,
            LIBRARY_CHIP + kind as usize,
            label,
            Some(value),
            options,
            menu,
            &bounds[kind as usize],
            TOOL_ACTIONS,
            window,
            cx,
        )
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(tool(
            window,
            cx,
            Tool::Genre,
            "GENRE",
            genres.unwrap_or("Tous").to_string(),
            genre_options,
        ))
        .child(tool(
            window,
            cx,
            Tool::Decade,
            "ANNÉE",
            decade.map_or("Toutes".to_string(), decade_label),
            decade_options,
        ))
        .child(unwatched_chip(unwatched, window, cx))
        .child(tool(
            window,
            cx,
            Tool::Sort,
            "TRI",
            tools.sort.label().to_string(),
            sort_options,
        ))
}

/// Element ids of the library's chips, apart from the film page's.
const LIBRARY_CHIP: usize = 100;

const TOOL_ACTIONS: Actions<Tool> = Actions {
    toggle: Shell::toggle_tool_menu,
    choose: Shell::pick,
    outside: Shell::pressed_outside_tool_menu,
};

/// `● NON VUS`: lit when only what the user has not watched shows. `on` is how much it is, from
/// 0 to 1, so that it lights and goes out by degrees.
fn unwatched_chip(on: f32, window: &mut Window, cx: &mut Context<Shell>) -> Stateful<Div> {
    let shades = theme::Shades {
        rest: rgba(0x141416cc),
        hover: theme::line(),
        press: rgb(0x2e2e33),
    };
    let lit = |off, on_| motion::mix(off, on_, on);
    theme::eased("unwatched", shades, window, cx)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(7.))
        .rounded_full()
        .border_1()
        .border_color(lit(theme::line(), theme::muted()))
        .cursor_pointer()
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_unwatched(cx)))
        .child(icon(IconName::Circle, 8.).text_color(lit(theme::dim(), theme::accent())))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(lit(theme::dim(), theme::text()))
                .child("NON VUS"),
        )
}

/// A to Z and `#` down the right edge: a click goes to the first title of the letter. The
/// current one is where the `highlight` is, a position on the bar that glides.
fn alphabet(highlight: f32, shown: f32, window: &mut Window, cx: &mut Context<Shell>) -> Div {
    div()
        .absolute()
        .top(px(150.))
        .bottom(px(24.))
        .right(px(6.))
        .w(px(24.))
        .opacity(shown)
        .flex()
        .flex_col()
        .justify_between()
        .children(LETTERS.iter().enumerate().map(|(index, &letter)| {
            let here = letter_weight(highlight, index);
            theme::eased(("letter", index), theme::Shades::veil(), window, cx)
                .flex()
                .items_center()
                .justify_center()
                .h(px(16.))
                .w_full()
                .rounded(px(5.))
                .text_size(px(10.))
                .font_weight(if here > 0.5 {
                    FontWeight::BOLD
                } else {
                    FontWeight::MEDIUM
                })
                .text_color(motion::mix(theme::dim(), theme::text(), here))
                .cursor_pointer()
                .on_click(cx.listener(move |shell, _, _, cx| shell.jump_to_letter(letter, cx)))
                .child(letter.to_string())
        }))
}
