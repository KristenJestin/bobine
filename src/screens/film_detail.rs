//! A film's page, after Infuse: the backdrop fills the top; on the left the title, the facts, the
//! synopsis, the play buttons and the chips that choose the version and the tracks. Below, rows
//! aligned with the text: the cast, the collection, similar titles. A row reaches the window's
//! edges: its first item lines up with the text, and it scrolls under the edges. The page
//! scrolls.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use bobine::provider::{CastMember, Film, FilmDetail};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, Div, FontWeight, IntoElement, MouseDownEvent, Pixels,
    ScrollHandle, SharedString, Stateful, Window, anchored, canvas, deferred, div,
    linear_color_stop, linear_gradient, point, px, rgb, rgba,
};

use super::{centered, note, picture, poster, progress_bar, row_heading};
use crate::film_page::{self, Choices, From};
use crate::icons::{IconName, icon};
use crate::images;
use crate::menu;
use crate::shell::{Chip, Loadable, Shell};
use crate::theme;

/// The menu open, and the one drawn with how much of it shows (it may be fading out).
#[derive(Clone, Copy)]
struct MenuState {
    open: Option<Chip>,
    drawn: Option<(Chip, f32)>,
}

/// Where a chip was last drawn, shared with the shell.
type ChipBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The left of the text and the rows, after the design.
const EDGE: f32 = 40.;
const TEXT_WIDTH: f32 = 620.;
const ROW_POSTER: f32 = 150.;
const PHOTO: f32 = 96.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.details.last() else {
        return centered().into_any_element();
    };
    let (detail, choices) = match (&page.detail, &page.choices) {
        (Loadable::Ready(detail), Some(choices)) => (detail.clone(), choices.clone()),
        (Loadable::Failed(why), _) => return centered().child(note(*why)).into_any_element(),
        _ => return centered().child(note("Chargement…")).into_any_element(),
    };
    let now = Instant::now();
    let page = shell.details.last_mut().expect("the page read above");
    let open = page.menus.open();
    let menu = MenuState {
        open,
        drawn: page.menu_fade.follow(open, now),
    };
    if page.menu_fade.moving(now) {
        window.request_animation_frame();
    }
    let (error, busy) = (page.error, page.busy);
    let chips = page.chips.clone();
    let collection = page.collection.clone();
    let scroll = page.scroll.clone();
    let [cast_scroll, collection_scroll, similar_scroll] = page.rows.clone();
    let height = f32::from(window.viewport_size().height);

    let hero = hero(
        shell, &detail, &choices, menu, &chips, error, busy, height, window, cx,
    );
    let mut rows: Vec<AnyElement> = Vec::new();
    if !detail.cast.is_empty() {
        rows.push(cast_row(shell, &detail.cast, &cast_scroll, cx).into_any_element());
    }
    if let Some(collection) = collection {
        let mut films = collection.others.clone();
        films.push(detail.film.clone());
        films.sort_by_key(|film| film.year);
        let caption = format!("{} films", films.len());
        rows.push(
            poster_row(
                shell,
                &collection.name,
                Some(caption),
                &films,
                Some(&detail.film),
                &collection_scroll,
                cx,
            )
            .into_any_element(),
        );
    }
    if !detail.similar.is_empty() {
        rows.push(
            poster_row(
                shell,
                "Titres proches",
                None,
                &detail.similar,
                None,
                &similar_scroll,
                cx,
            )
            .into_any_element(),
        );
    }

    // The page scrolls itself and its rows by hand (`Shell::scroll_film_page`): GPUI's own
    // scrolling moves every container under the pointer on both axes at once.
    div()
        .id("film-page")
        .size_full()
        .overflow_hidden()
        .track_scroll(&scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| shell.scroll_film_page(event, window, cx)),
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

#[allow(
    clippy::too_many_arguments,
    reason = "one page's state, read once per frame"
)]
fn hero(
    shell: &mut Shell,
    detail: &FilmDetail,
    choices: &Choices,
    menu: MenuState,
    chips: &[ChipBounds; 3],
    error: Option<&'static str>,
    busy: bool,
    window_height: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let film = &detail.film;
    let backdrop = shell.image(film.backdrop.as_ref(), images::BACKDROP, cx);
    let height = (window_height * 0.86).clamp(560., 900.);
    let source = choices.source(detail);
    let bg = theme::background();
    let clear = rgba(0x09090a00);

    let play = theme::primary_button(
        "film-play",
        Some(IconName::Play),
        film_page::play_label(film),
        window,
        cx,
    )
    .on_click(cx.listener(|shell, _, window, cx| shell.play_film(From::Resume, window, cx)));
    let from_start = film.resume.is_some().then(|| {
        theme::ghost_button(
            "film-from-start",
            Some(IconName::RotateCcw),
            "Depuis le début",
            window,
            cx,
        )
        .on_click(cx.listener(|shell, _, window, cx| shell.play_film(From::Beginning, window, cx)))
    });
    let watched = theme::ghost_button("film-watched", Some(IconName::Check), "Vu", window, cx)
        .when(film.watched, |button| {
            button
                .text_color(theme::accent())
                .border_color(theme::accent())
        })
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_watched(cx)));
    let buttons = div()
        .flex()
        .gap(px(10.))
        .when(busy, |row| row.opacity(0.6))
        .child(play)
        .children(from_start)
        .child(watched);

    let chips = div()
        .flex()
        .gap(px(8.))
        .children(source.filter(|_| detail.versions.len() > 1).map(|_| {
            let options = detail
                .versions
                .iter()
                .enumerate()
                .map(|(index, version)| {
                    (
                        Some(index as i32),
                        film_page::version_label(version),
                        index == choices.version,
                    )
                })
                .collect();
            chip(
                Chip::Version,
                "VERSION",
                source.map(film_page::version_label),
                options,
                menu,
                &chips[Chip::Version as usize],
                window,
                cx,
            )
        }))
        .children(source.filter(|s| !s.audio.is_empty()).map(|source| {
            let current = source.audio.iter().find(|t| Some(t.index) == choices.audio);
            let options = source
                .audio
                .iter()
                .map(|t| {
                    (
                        Some(t.index),
                        film_page::audio_label(t),
                        Some(t.index) == choices.audio,
                    )
                })
                .collect();
            chip(
                Chip::Audio,
                "AUDIO",
                current.map(film_page::audio_label),
                options,
                menu,
                &chips[Chip::Audio as usize],
                window,
                cx,
            )
        }))
        .children(source.map(|source| {
            let current = source
                .subtitles
                .iter()
                .find(|t| Some(t.index) == choices.subtitle);
            let options = std::iter::once((
                None,
                film_page::subtitle_label(None),
                choices.subtitle.is_none(),
            ))
            .chain(source.subtitles.iter().map(|t| {
                (
                    Some(t.index),
                    film_page::subtitle_label(Some(t)),
                    Some(t.index) == choices.subtitle,
                )
            }))
            .collect();
            chip(
                Chip::Subtitle,
                "SOUS-TITRES",
                Some(film_page::subtitle_label(current)),
                options,
                menu,
                &chips[Chip::Subtitle as usize],
                window,
                cx,
            )
        }));

    let mut meta = film_page::meta(detail).into_iter();
    let meta_line = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(14.))
        .text_color(theme::muted())
        .children(meta.next())
        .children(meta.map(|part| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(div().w(px(1.)).h(px(12.)).bg(theme::line()))
                .child(part)
        }));

    div()
        .relative()
        .w_full()
        .h(px(height))
        .children(picture(backdrop, 0.))
        .child(div().absolute().inset_0().bg(linear_gradient(
            90.,
            linear_color_stop(bg, 0.),
            linear_color_stop(clear, 0.75),
        )))
        .child(div().absolute().inset_0().bg(linear_gradient(
            0.,
            linear_color_stop(bg, 0.),
            linear_color_stop(clear, 0.45),
        )))
        .child(
            div()
                .absolute()
                .left(px(EDGE))
                .bottom(px(28.))
                .w(px(TEXT_WIDTH))
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(
                    div()
                        .text_size(px(44.))
                        .line_height(px(50.))
                        .font_weight(FontWeight::BOLD)
                        .child(film.title.clone()),
                )
                .child(meta_line)
                .children((!detail.directors.is_empty()).then(|| {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::muted())
                        .child(format!("Réalisé par {}", detail.directors.join(", ")))
                }))
                .children(detail.overview.clone().map(|overview| {
                    div()
                        .text_size(px(15.))
                        .line_height(px(23.))
                        .text_color(rgba(0xf4f4f5d9))
                        .child(overview)
                }))
                .child(div().pt(px(6.)).child(buttons))
                .children(film_page::progress(film).map(|fraction| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(progress_bar(fraction, 300.))
                        .children(film_page::time_left(film).map(|left| {
                            div()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(left)
                        }))
                }))
                .children(error.map(|why| {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::danger())
                        .child(why)
                }))
                .child(chips),
        )
}

/// `AUDIO  Français · E-AC-3 5.1 ⌄`; a click opens the choices under it.
#[allow(
    clippy::too_many_arguments,
    reason = "one chip's state, read once per frame"
)]
fn chip(
    kind: Chip,
    label: &'static str,
    value: Option<String>,
    options: Vec<(Option<i32>, String, bool)>,
    menu: MenuState,
    bounds: &ChipBounds,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let drawn = bounds.clone();
    let open = menu.open == Some(kind);
    let shades = theme::Shades {
        rest: rgba(0x141416cc),
        hover: theme::line(),
        press: rgb(0x2e2e33),
    };
    let button = theme::eased(("chip", kind as usize), shades, window, cx)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(7.))
        .rounded_full()
        .border_1()
        .border_color(if open { theme::muted() } else { theme::line() })
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| shell.toggle_menu(kind, cx)))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::dim())
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value.unwrap_or_else(|| "—".into())),
        )
        .child(icon(IconName::ChevronDown, 14.).text_color(theme::muted()));
    let under = bounds
        .get()
        .zip(menu.drawn.filter(|(drawn, _)| *drawn == kind));
    div()
        .relative()
        .child(button)
        // Where the chip is drawn, for its menu.
        .child(
            canvas(move |at, _, _| drawn.set(Some(at)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .children(
            under
                .map(|(chip, (_, shown))| self::menu(chip, shown, open, kind, options, window, cx)),
        )
}

/// The choices of a chip, over the rest of the page, `shown` of the way in. Only an open menu
/// answers the mouse: one fading out lets clicks through.
fn menu(
    chip: Bounds<Pixels>,
    shown: f32,
    open: bool,
    kind: Chip,
    options: Vec<(Option<i32>, String, bool)>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let lines: Vec<Stateful<Div>> = options
        .into_iter()
        .enumerate()
        .map(|(index, (value, label, current))| {
            theme::eased(("choice", index), theme::Shades::line(), window, cx)
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(7.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(open, |line| {
                    line.on_click(cx.listener(move |shell, _, _, cx| shell.choose(kind, value, cx)))
                })
                .child(
                    div()
                        .size(px(14.))
                        .text_color(theme::accent())
                        .when(current, |mark| mark.child(icon(IconName::Check, 14.))),
                )
                .child(label)
        })
        .collect();
    // It drops into place from a little higher.
    let rise = point(px(0.), px(-8. * (1. - shown)));
    deferred(
        anchored()
            .position(menu::position(chip) + rise)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .id("chip-menu")
                    .opacity(shown)
                    .min_w(px(240.))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .rounded(px(theme::RADIUS_LARGE))
                    .border_1()
                    .border_color(theme::line())
                    .bg(theme::panel())
                    .when(open, |menu| {
                        menu.occlude().on_mouse_down_out(cx.listener(
                            |shell, event: &MouseDownEvent, _, cx| {
                                shell.pressed_outside_menu(event.position, cx);
                            },
                        ))
                    })
                    .children(lines),
            ),
    )
}

/// Round photos, the name and the role.
fn cast_row(
    shell: &mut Shell,
    cast: &[CastMember],
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> Div {
    let people: Vec<Stateful<Div>> = cast
        .iter()
        .enumerate()
        .map(|(index, person)| {
            let photo = shell.image(person.photo.as_ref(), images::PHOTO, cx);
            div()
                .id(("person", index))
                .w(px(PHOTO + 24.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
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
                .children(person.role.clone().map(|role| {
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
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(row_heading("Distribution").px(px(EDGE)))
        .child(
            div()
                .id("cast-row")
                .flex()
                .gap(px(12.))
                .px(px(EDGE))
                .overflow_hidden()
                .track_scroll(scroll)
                .children(people),
        )
}

/// A heading and posters; `current` is outlined and not clickable.
fn poster_row(
    shell: &mut Shell,
    title: &str,
    caption: Option<String>,
    films: &[Film],
    current: Option<&Film>,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> Div {
    let posters: Vec<Stateful<Div>> = films
        .iter()
        .enumerate()
        .map(|(index, film)| {
            let outlined = current.is_some_and(|current| current.id == film.id);
            let id = (SharedString::from(format!("{title}-film")), index);
            poster(shell, film, ROW_POSTER, id, outlined, cx)
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(12.))
                .px(px(EDGE))
                .child(row_heading(title.to_string()))
                .children(caption.map(|caption| {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::dim())
                        .child(caption)
                })),
        )
        .child(
            div()
                .id(format!("{title}-row"))
                .flex()
                .gap(px(GAP))
                .px(px(EDGE))
                .overflow_hidden()
                .track_scroll(scroll)
                .children(posters),
        )
}

const GAP: f32 = 16.;
