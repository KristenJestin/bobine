//! Discover (#63): what the request server offers beyond the library. A search field at the top;
//! under it the viewer's requests, then Trending, Popular movies, Popular series and Upcoming,
//! each a row that scrolls sideways. A search replaces the rows with its results. Every title
//! wears a small pill: in the library, pending, processing, available.

use bobine::requests::{Discovered, MediaKind};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, Context, Div, ElementId, FontWeight, IntoElement,
    SharedString, Stateful, Window, div, px, rgba,
};

use super::film_detail::{EDGE, ROW_POSTER, row};
use super::library::GRID_TOP;
use super::{note, picture};
use crate::discover;
use crate::icons::{IconName, icon};
use crate::images;
use crate::motion;
use crate::shell::{Loadable, Shell};
use crate::theme;

/// The row headings, in `DiscoverRow::ALL`'s order.
const ROWS: [&str; 4] = ["Trending", "Popular movies", "Popular series", "Upcoming"];

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let page = &shell.seerr.discover;
    let field = page.field.clone();
    let scroll = page.scroll.clone();
    let scrolls = page.row_scrolls.clone();
    let query = page.query.clone();
    let results = page.results.clone();
    let mine = page.mine.clone();
    let rows = page.rows.clone();

    let search = div()
        .debug_selector(|| "discover-search".into())
        .flex()
        .items_center()
        .gap(px(10.))
        .w(px(420.))
        .px(px(14.))
        .py(px(4.))
        .rounded_full()
        .border_1()
        .border_color(theme::line())
        .bg(theme::panel())
        .child(icon(IconName::Search, 16.).text_color(theme::muted()))
        .child(div().flex_1().children(field));

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .px(px(EDGE))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(icon(IconName::Compass, 26.).text_color(theme::accent()))
                .child(
                    div()
                        .text_size(px(28.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Discover"),
                ),
        )
        .child(search);

    let mut body: Vec<AnyElement> = Vec::new();
    if query.is_empty() {
        match mine {
            Some(Loadable::Ready(mine)) if !mine.is_empty() => {
                let cards = mine
                    .iter()
                    .enumerate()
                    .map(|(at, request)| {
                        let label = discover::request_label(request.status);
                        card(shell, &request.title, ("mine", at), Some(label), cx)
                    })
                    .collect();
                body.push(row("My requests", None, cards, &scrolls[0]).into_any_element());
            }
            Some(Loadable::Failed(why)) => {
                body.push(failed_row("My requests", why).into_any_element());
            }
            _ => {}
        }
        for (at, loaded) in rows.into_iter().enumerate() {
            let title = ROWS[at];
            match loaded {
                Some(Loadable::Ready(titles)) if !titles.is_empty() => {
                    let cards = titles
                        .iter()
                        .enumerate()
                        .map(|(index, found)| card(shell, found, (title, index), None, cx))
                        .collect();
                    body.push(row(title, None, cards, &scrolls[at + 1]).into_any_element());
                }
                Some(Loadable::Failed(why)) => body.push(failed_row(title, why).into_any_element()),
                Some(Loadable::Loading) => {
                    body.push(failed_row(title, "Loading…").into_any_element())
                }
                _ => {}
            }
        }
    } else {
        body.push(results_grid(shell, &query, results, window, cx).into_any_element());
    }

    div()
        .debug_selector(|| "discover-page".into())
        .id("discover-page")
        .size_full()
        .overflow_hidden()
        .track_scroll(&scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| shell.scroll_discover(event, window, cx)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(40.))
                .pt(px(GRID_TOP + 8.))
                .pb(px(64.))
                .child(header)
                .children(body.into_iter().enumerate().map(|(at, part)| {
                    // Each part comes in with a fade, a little after the one above it.
                    div().child(part).with_animation(
                        ElementId::NamedInteger("discover-part".into(), at as u64),
                        Animation::new(motion::page()).with_easing(motion::ease_out),
                        |part, t| part.opacity(t),
                    )
                })),
        )
}

/// A row that could not be read, or is being read: its heading and a line.
fn failed_row(title: &'static str, why: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .px(px(EDGE))
        .child(super::row_heading(title))
        .child(note(why))
}

/// The results of a search, wrapped in lines of cards.
fn results_grid(
    shell: &mut Shell,
    query: &str,
    results: Option<Loadable<Vec<Discovered>>>,
    _window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let heading = super::row_heading(format!("Results for “{query}”"));
    let content = match results {
        None | Some(Loadable::Loading) => note("Searching…").into_any_element(),
        Some(Loadable::Failed(why)) => note(why).into_any_element(),
        Some(Loadable::Ready(found)) if found.is_empty() => {
            note(format!("Nothing on TMDb for “{query}”.")).into_any_element()
        }
        Some(Loadable::Ready(found)) => div()
            .debug_selector(|| "discover-results".into())
            .flex()
            .flex_wrap()
            .gap(px(20.))
            .children(
                found
                    .iter()
                    .enumerate()
                    .map(|(at, title)| card(shell, title, ("result", at), None, cx)),
            )
            .into_any_element(),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .px(px(EDGE))
        .child(heading)
        .child(content)
}

/// "Similar, not in your library", under a film or series of the library: what the request
/// server recommends after it that the library does not hold. Nothing without any.
pub(super) fn similar_row(
    shell: &mut Shell,
    titles: &[Discovered],
    scroll: &gpui_kit::ScrollHandle,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    if titles.is_empty() {
        return None;
    }
    let cards = titles
        .iter()
        .enumerate()
        .map(|(at, title)| card(shell, title, ("not-in-library", at), None, cx))
        .collect();
    Some(
        div()
            .debug_selector(|| "similar-not-in-library".into())
            .child(row("Similar, not in your library", None, cards, scroll))
            .into_any_element(),
    )
}

/// A small pill over a poster or beside a title: what the request server or the library says.
pub(super) fn status_pill(label: &'static str) -> Div {
    let had = matches!(label, "In library" | "Available");
    div()
        .debug_selector(move || format!("status {label}"))
        .flex()
        .items_center()
        .gap(px(5.))
        .px(px(8.))
        .py(px(3.))
        .rounded_full()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .when(had, |pill| {
            pill.bg(theme::accent()).text_color(theme::background())
        })
        .when(!had, |pill| {
            pill.bg(rgba(0x09090ad9))
                .border_1()
                .border_color(rgba(0xffffff26))
                .text_color(theme::text())
        })
        .children(had.then(|| icon(IconName::CheckCircle, 11.)))
        .children((!had).then(|| div().size(px(6.)).rounded_full().bg(theme::accent())))
        .child(label)
}

/// A title of the request server as a poster: its pill top left, its name and year under it. A
/// click opens its library page when the library holds it, else its request page. `said`: the
/// pill to wear instead of the title's own (a request's status).
pub(super) fn card(
    shell: &mut Shell,
    title: &Discovered,
    id: (impl Into<SharedString>, usize),
    said: Option<&'static str>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let (name, index) = id;
    let name: SharedString = name.into();
    let width = ROW_POSTER;
    let image = shell.tmdb_image(title.poster.as_ref(), images::POSTER, cx);
    let in_library = shell.library_page_of(title.kind, title.tmdb).is_some();
    let label = said.or_else(|| discover::status_label(title.availability, in_library));
    let (kind, tmdb) = (title.kind, title.tmdb);
    let caption = match (title.year, kind) {
        (Some(year), MediaKind::Movie) => format!("{year} · Movie"),
        (Some(year), MediaKind::Tv) => format!("{year} · Series"),
        (None, MediaKind::Movie) => "Movie".into(),
        (None, MediaKind::Tv) => "Series".into(),
    };
    let selector = format!("discover-card {}", title.title);
    div()
        .id(ElementId::NamedInteger(name, index as u64))
        .debug_selector(move || selector.clone())
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| shell.open_discovered(kind, tmdb, cx)))
        .group("discover-card")
        .child(
            div()
                .relative()
                .w(px(width))
                .h(px(width * 1.5))
                .rounded(px(theme::RADIUS))
                .overflow_hidden()
                .bg(theme::panel())
                .border_1()
                .border_color(theme::line())
                .group_hover("discover-card", |poster| {
                    poster.border_color(theme::muted())
                })
                .children(picture(image, theme::RADIUS))
                .children(label.map(|label| {
                    div()
                        .absolute()
                        .top(px(8.))
                        .left(px(8.))
                        .child(status_pill(label))
                })),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(title.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .child(caption),
                ),
        )
}
