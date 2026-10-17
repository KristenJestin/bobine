//! The page of a title the library does not hold (#63), laid out like the film and series pages:
//! the backdrop over the top, the title, its facts, its pill and its synopsis on the left, the
//! request button under them; the cast and similar titles in rows below. A series' button opens
//! the seasons its request offers, checked, under it.

use std::time::Instant;

use bobine::requests::{CastPerson, MediaKind, TitleDetail};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent, Pixels,
    ScrollHandle, Stateful, Window, anchored, canvas, deferred, div, point, px, rgba,
};

use super::discover::{card, status_pill};
use super::film_detail::{hero_frame, hero_text, meta_line, row, trailer_button};
use super::seerr_collection::part_of;
use super::{centered, note, picture};
use crate::discover::{self, SeasonPicker};
use crate::film_page;
use crate::icons::{IconName, icon};
use crate::images;
use crate::menu;
use crate::shell::{Loadable, Shell};
use crate::theme;
use crate::trailer;

/// The Trailer button of a request page: its id, which is also its selector.
pub const REQUEST_TRAILER: &str = "request-trailer";

const PHOTO: f32 = 96.;
/// The thin bar under the status of a title that downloads, and its height.
const BAR: f32 = 220.;
const BAR_HEIGHT: f32 = 3.;
/// The send button at the foot of the seasons menu, and the room around it.
const MENU_FOOT: f32 = 60.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.seerr.requests.last() else {
        return centered().into_any_element();
    };
    let detail = match &page.detail {
        Loadable::Ready(detail) => detail.clone(),
        Loadable::Failed(why) => return centered().child(note(*why)).into_any_element(),
        Loadable::Loading => return centered().child(note("Loading…")).into_any_element(),
    };
    let (kind, tmdb) = (page.kind, page.tmdb);
    let in_library = shell.library_page_of(kind, tmdb).is_some();
    // Until the library is indexed, a title may be in it: nothing is offered yet.
    let offered = shell.seerr.index.is_some() && page.can_request(in_library);
    let now = Instant::now();
    let page = shell
        .seerr
        .requests
        .last_mut()
        .expect("the page read above");
    let open = page.menus.open().is_some();
    let drawn = page.menu_fade.follow(page.menus.open(), now);
    if page.menu_fade.moving(now) {
        window.request_animation_frame();
    }
    let (busy, error, picker) = (page.busy, page.error, page.picker.clone());
    let bounds = page.button.clone();
    let scroll = page.scroll.clone();
    let [cast_scroll, similar_scroll] = page.rows.clone();
    let similar = page.similar.clone();

    let window_height = f32::from(window.viewport_size().height);
    let backdrop = shell.tmdb_image(detail.title.backdrop.as_ref(), images::BACKDROP, cx);
    let height = (window_height * 0.86).clamp(560., 900.);

    let button = offered.then(|| {
        let label = match (busy, kind) {
            (true, _) => "Requesting…",
            (false, MediaKind::Movie) => "Request",
            (false, MediaKind::Tv) => "Request seasons",
        };
        let measured = bounds.clone();
        let seasons = drawn
            .zip(bounds.get())
            .zip(picker.clone())
            .map(|(((_, shown), at), picker)| seasons_menu(at, shown, open, picker, window, cx));
        div()
            .relative()
            .child(
                theme::primary_button(
                    "request-button",
                    Some(IconName::PaperPlane),
                    label,
                    window,
                    cx,
                )
                .debug_selector(|| "request-button".into())
                .when(busy, |button| button.opacity(0.6))
                .when(kind == MediaKind::Tv, |button| {
                    button.child(icon(IconName::ChevronDown, 16.))
                })
                .on_click(cx.listener(|shell, _, _, cx| shell.request_title(cx))),
            )
            // Where the button is drawn, for its menu.
            .child(
                canvas(move |at, _, _| measured.set(Some(at)), |_, _, _, _| {})
                    .absolute()
                    .inset_0(),
            )
            .children(seasons)
    });
    let trailer = trailer::requested(detail.trailer.as_deref(), shell.ytdlp)
        .map(|_| trailer_button(REQUEST_TRAILER, window, cx));
    let availability = detail.title.availability;
    let pill =
        discover::page_status(kind, availability, in_library, &detail.downloads).map(status_pill);
    let bar = discover::download_progress(availability, in_library, &detail.downloads)
        .map(|progress| download_bar(progress.percent));
    // Once the title is in the library, the page leads there.
    let open_library = in_library.then(|| {
        theme::primary_button(
            "request-open-library",
            Some(IconName::Play),
            "Open in library",
            window,
            cx,
        )
        .debug_selector(|| "request-open-library".into())
        .on_click(cx.listener(move |shell, _, _, cx| shell.open_discovered(kind, tmdb, cx)))
    });
    let part_of = (detail.collection.clone()).map(|collection| part_of(collection, window, cx));

    let hero = hero_frame(backdrop, height).child(
        hero_text(detail.title.title.clone())
            .child(meta_line(facts(&detail), shell.font(), window))
            .children(pill.map(|pill| {
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.))
                    .child(pill)
                    .children(bar)
            }))
            .children(part_of.map(|link| div().flex().child(link)))
            .children(detail.title.overview.clone().map(|overview| {
                div()
                    .text_size(px(15.))
                    .line_height(px(23.))
                    .text_color(rgba(0xf4f4f5d9))
                    .child(overview)
            }))
            .children(
                (button.is_some() || trailer.is_some() || open_library.is_some()).then(|| {
                    div()
                        .pt(px(6.))
                        .flex()
                        .gap(px(10.))
                        .children(open_library)
                        .children(button)
                        .children(trailer)
                }),
            )
            .children(error.map(|why| {
                div()
                    .debug_selector(|| "request-error".into())
                    .text_size(px(13.))
                    .text_color(theme::danger())
                    .child(why)
            })),
    );

    let mut rows: Vec<AnyElement> = Vec::new();
    if !detail.cast.is_empty() {
        rows.push(cast_row(shell, &detail.cast, &cast_scroll, cx).into_any_element());
    }
    if !similar.is_empty() {
        let cards = similar
            .iter()
            .enumerate()
            .map(|(at, title)| card(shell, title, ("similar", at), None, cx))
            .collect();
        rows.push(row("Similar titles", None, cards, &similar_scroll).into_any_element());
    }

    div()
        .debug_selector(|| "request-page".into())
        .id("request-page")
        .size_full()
        .overflow_hidden()
        .track_scroll(&scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| shell.scroll_request_page(event, window, cx)),
        )
        .child(hero)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(40.))
                .pt(px(12.))
                .pb(px(64.))
                .children(rows),
        )
        .into_any_element()
}

/// A thin bar filled to `percent` of its width.
fn download_bar(percent: u8) -> Div {
    div()
        .debug_selector(|| "download-bar".into())
        .w(px(BAR))
        .h(px(BAR_HEIGHT))
        .rounded_full()
        .bg(theme::line())
        .child(
            div()
                .debug_selector(|| "download-fill".into())
                .w(px(BAR * f32::from(percent.min(100)) / 100.))
                .h_full()
                .rounded_full()
                .bg(theme::accent()),
        )
}

/// `2025 | ★ 7.1 | 1h 48m | Drama, Mystery`; a series says its seasons instead of a length.
fn facts(detail: &TitleDetail) -> Vec<String> {
    let title = &detail.title;
    let mut parts = Vec::new();
    parts.extend(title.year.map(|year| year.to_string()));
    parts.extend(title.rating.map(|rating| format!("★ {rating:.1}")));
    parts.extend(detail.runtime.map(film_page::short_duration));
    if title.kind == MediaKind::Tv && !detail.seasons.is_empty() {
        parts.push(match detail.seasons.len() {
            1 => "1 season".into(),
            many => format!("{many} seasons"),
        });
    }
    if !detail.genres.is_empty() {
        parts.push(detail.genres.join(", "));
    }
    parts
}

/// The seasons a series' request offers, a box each, and the button that sends them, under the
/// request button, `shown` of the way in. Only an open menu answers the mouse.
fn seasons_menu(
    button: Bounds<Pixels>,
    shown: f32,
    open: bool,
    picker: SeasonPicker,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let lines: Vec<Stateful<Div>> = picker
        .offered
        .iter()
        .map(|season| {
            let number = season.number;
            let checked = picker.is_checked(number);
            theme::eased(
                ("season", number as usize),
                theme::Shades::line(),
                window,
                cx,
            )
            .debug_selector(move || format!("season {number}"))
            .flex()
            .items_center()
            .gap(px(10.))
            .flex_none()
            .h(px(menu::LINE))
            .px(px(12.))
            .rounded(px(7.))
            .text_size(px(13.))
            .cursor_pointer()
            .when(open, |line| {
                line.on_click(cx.listener(move |shell, _, _, cx| shell.toggle_season(number, cx)))
            })
            .child(
                icon(
                    if checked {
                        IconName::CheckSquare
                    } else {
                        IconName::Square
                    },
                    16.,
                )
                .text_color(if checked {
                    theme::brand()
                } else {
                    theme::dim()
                }),
            )
            .child(div().flex_1().child(season.name.clone()))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme::dim())
                    .child(format!("{} episodes", season.episodes)),
            )
        })
        .collect();
    let none = picker.chosen().is_empty();
    let send = theme::primary_button("seasons-send", None, picker.button(), window, cx)
        .debug_selector(|| "seasons-send".into())
        .w_full()
        .justify_center()
        .when(none, |button| button.opacity(0.4))
        .when(open && !none, |button| {
            button.on_click(cx.listener(|shell, _, _, cx| shell.request_seasons(cx)))
        });
    let wanted = menu::height(lines.len()) + MENU_FOOT;
    let window_height = f32::from(window.viewport_size().height);
    // Under its button, never over the title above it: on a short window its seasons scroll.
    let least = menu::height(1) + MENU_FOOT;
    let (at, height) = menu::place_below(button, wanted, window_height, least);
    // It comes into place from a little nearer its button.
    let rise = point(px(0.), px(-8. * (1. - shown)));
    deferred(
        anchored()
            .position(at + rise)
            .snap_to_window_with_margin(px(menu::MARGIN))
            .child(
                div()
                    .id("seasons-menu")
                    .when(open, |menu| menu.debug_selector(|| "seasons-menu".into()))
                    .opacity(shown)
                    .h(px(height))
                    .w(px(300.))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .rounded(px(theme::RADIUS_LARGE))
                    .border_1()
                    .border_color(theme::line())
                    .bg(theme::panel())
                    .when(open, move |menu| {
                        menu.occlude().on_mouse_down_out(cx.listener(
                            move |shell, event: &MouseDownEvent, _, cx| {
                                // A side button is Back: it closes the menu itself, once.
                                if matches!(event.button, MouseButton::Navigate(_)) {
                                    return;
                                }
                                shell.pressed_outside_seasons(event.position, cx);
                            },
                        ))
                    })
                    .child(
                        div()
                            .id("seasons-lines")
                            .flex_1()
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .children(lines),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pt(px(8.))
                            .px(px(4.))
                            .pb(px(4.))
                            .child(send),
                    ),
            ),
    )
    .into_any_element()
}

/// Round photos from TMDb, the name and the role.
fn cast_row(
    shell: &mut Shell,
    cast: &[CastPerson],
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> Div {
    let people: Vec<Stateful<Div>> = cast
        .iter()
        .enumerate()
        .map(|(index, person)| {
            let photo = shell.tmdb_image(person.photo.as_ref(), images::PHOTO, cx);
            div()
                .id(("request-cast", index))
                .debug_selector(move || format!("request-cast {index}"))
                .w(px(PHOTO + 24.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .py(px(8.))
                .child(
                    div()
                        .relative()
                        .size(px(PHOTO))
                        .rounded_full()
                        .overflow_hidden()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(theme::line())
                        .text_size(px(26.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::muted())
                        // Under the photo while it comes, alone when there is none.
                        .child(film_page::initials(&person.name))
                        .children(picture(photo, PHOTO / 2.)),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(person.name.clone()),
                )
                .children(person.character.clone().map(|role| {
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .truncate()
                        .child(role)
                }))
        })
        .collect();
    row("Cast", None, people, scroll)
}
