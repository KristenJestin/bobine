//! The page of a TMDb collection on the request server (#81), laid out like the request page:
//! the backdrop over the top, the name, how many movies, the overview and "Request collection"
//! on the left; the movies in a row below, each with its pill and, when it may be asked for, a
//! box that eases between unchecked and checked.

use std::time::Instant;

use bobine::requests::{CollectionRef, Discovered};
use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, IntoElement, SharedString, Stateful, Window, div, px, rgb, rgba};

use super::discover::card;
use super::film_detail::{hero_frame, hero_text, meta_line, row};
use super::{centered, note};
use crate::discover::FilmPicker;
use crate::icons::{IconName, icon};
use crate::images;
use crate::motion::{self, Fade};
use crate::shell::{Loadable, Shell};
use crate::theme;

const CHECKBOX: f32 = 28.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.seerr_collection_shown() else {
        return centered().into_any_element();
    };
    let parts = match &page.parts {
        Loadable::Ready(parts) => parts.clone(),
        Loadable::Failed(why) => return centered().child(note(*why)).into_any_element(),
        Loadable::Loading => return centered().child(note("Loading…")).into_any_element(),
    };
    let (picker, busy, outcome) = (page.picker.clone(), page.busy, page.outcome.clone());
    let scroll = page.scroll.clone();
    let [films_scroll] = page.rows.clone();

    let window_height = f32::from(window.viewport_size().height);
    let backdrop = shell.tmdb_image(parts.backdrop.as_ref(), images::BACKDROP, cx);
    let height = (window_height * 0.86).clamp(560., 900.);

    // Until the library is indexed, a movie may be in it: nothing is offered yet.
    let button = picker
        .as_ref()
        .filter(|picker| picker.offers_any())
        .map(|picker| {
            let none = picker.chosen().is_empty();
            let label = if busy {
                "Requesting…".to_string()
            } else {
                picker.button()
            };
            theme::primary_button(
                "collection-request",
                Some(IconName::Stack),
                label,
                window,
                cx,
            )
            .when(busy || none, |button| button.opacity(0.6))
            .when(!busy && !none, |button| {
                button.on_click(cx.listener(|shell, _, _, cx| shell.request_collection(cx)))
            })
        });
    let count = match parts.films.len() {
        1 => "1 movie".to_string(),
        many => format!("{many} movies"),
    };

    let hero = hero_frame(backdrop, height).child(
        hero_text(parts.name.clone())
            .child(meta_line(vec![count], shell.font(), window))
            .children(parts.overview.clone().map(|overview| {
                div()
                    .text_size(px(15.))
                    .line_height(px(23.))
                    .text_color(rgba(0xf4f4f5d9))
                    .child(overview)
            }))
            .children(button.map(|button| div().pt(px(6.)).flex().child(button)))
            .children(outcome.map(|(line, all)| {
                div()
                    .debug_selector(|| "collection-outcome".into())
                    .text_size(px(13.))
                    .text_color(if all { theme::muted() } else { theme::danger() })
                    .child(line)
            })),
    );

    let cards = parts
        .films
        .iter()
        .enumerate()
        .map(|(at, film)| film_card(shell, film, at, picker.as_ref(), busy, window, cx))
        .collect();

    div()
        .debug_selector(|| "collection-page".into())
        .id("collection-page")
        .size_full()
        .overflow_hidden()
        .track_scroll(&scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| {
                shell.scroll_seerr_collection(event, window, cx)
            }),
        )
        .child(hero)
        .child(
            div()
                .pt(px(12.))
                .pb(px(64.))
                .child(row("Movies", None, cards, &films_scroll)),
        )
        .into_any_element()
}

/// A movie's card; one that may be asked for wears its box at the top right of its poster.
fn film_card(
    shell: &mut Shell,
    film: &Discovered,
    at: usize,
    picker: Option<&FilmPicker>,
    busy: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let card = card(shell, film, ("collection-film", at), None, cx);
    let Some(picker) = picker.filter(|picker| picker.is_offered(film.tmdb)) else {
        return card;
    };
    let checked = picker.is_checked(film.tmdb);
    let tmdb = film.tmdb;
    let selector = format!("collection-check {}", film.title);
    let fade = window.use_keyed_state(
        SharedString::from(format!("collection-check-{tmdb}")),
        cx,
        |_, _| Fade::starting_at(motion::quick(), checked),
    );
    let now = Instant::now();
    fade.update(cx, |fade, _| fade.set(checked, now));
    let fade = *fade.read(cx);
    let level = fade.level(now);
    if fade.moving(now) {
        window.request_animation_frame();
    }
    let checkbox = div()
        .id(("collection-check", at))
        .debug_selector(move || selector.clone())
        .absolute()
        .top(px(8.))
        .right(px(8.))
        .size(px(CHECKBOX))
        .rounded(px(7.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(motion::mix(rgba(0xffffff4d), theme::accent(), level))
        .bg(motion::mix(rgba(0x09090ad9), theme::accent(), level))
        .text_color(rgb(0x000000))
        .cursor_pointer()
        .occlude()
        .when(!busy, |checkbox| {
            checkbox.on_click(cx.listener(move |shell, _, _, cx| {
                cx.stop_propagation();
                shell.toggle_collection_film(tmdb, cx);
            }))
        })
        .child(icon(IconName::Check, 16.).opacity(level));
    card.relative().child(checkbox)
}

/// "Part of …": the collection a movie belongs to, on its request page and its library page.
pub(super) fn part_of(
    collection: CollectionRef,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    theme::eased("part-of-collection", theme::Shades::veil(), window, cx)
        .debug_selector(|| "part-of-collection".into())
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(8.))
        .py(px(4.))
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(theme::muted())
        .cursor_pointer()
        .child(icon(IconName::Stack, 14.))
        .child(format!("Part of {}", collection.name))
        .child(icon(IconName::ChevronRight, 13.))
        .on_click(cx.listener(move |shell, _, _, cx| {
            shell.open_seerr_collection(collection.id, cx);
        }))
}
