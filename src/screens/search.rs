//! The search palette, after the design's Ctrl+K window: a field, the results grouped under
//! their headings, the keys it answers to. It fades and slides in over the page, the highlight
//! glides from row to row and the list grows to what it holds.

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, FontWeight, IntoElement, MouseButton, MouseMoveEvent, Window, div,
    point, px, rgba,
};

use super::picture;
use crate::icons::{IconName, icon};
use crate::images;
use crate::search::{self, Hit, Line};
use crate::shell::{SERIES_PAGE, Shell};
use crate::theme;

const WIDTH: f32 = 680.;
/// What the list says in place of results.
const MESSAGE_HEIGHT: f32 = 64.;
/// Room kept under the last row.
const LIST_END: f32 = 8.;
/// How far the palette slides while it fades.
const SLIDE: f32 = 14.;

/// The palette over the page, while it shows or fades out.
pub fn palette(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let now = Instant::now();
    let list_max = shell.search_list_height(window);
    let state = shell.search.as_mut()?;
    state.fade.set(state.open, now);
    if !state.fade.visible(now) {
        shell.search = None;
        return None;
    }
    let level = state.fade.level(now);
    let lines = state.palette.lines();
    let selected = state.palette.selected();
    let message = state.palette.message();
    let failure = state.palette.failure();
    let input = state.input.clone();
    let scroll = state.scroll.clone();

    // The highlight and the list's height ease to where the results put them.
    let hits = lines
        .iter()
        .filter(|line| matches!(line, Line::Hit(..)))
        .count();
    let content = search::lines_height(&lines) + LIST_END;
    let body = match (hits, &message) {
        (0, Some(_)) => MESSAGE_HEIGHT,
        (0, None) => 0.,
        _ => content.min(list_max),
    };
    let top = search::row_top(&lines, selected).unwrap_or(0.);
    if !state.had_hits {
        state.highlight.jump(top);
    }
    state.had_hits = hits > 0;
    state.highlight.set(top, now);
    state.body.set(body, now);
    if let Some(offset) = state.scrolling.advance(now) {
        scroll.set_offset(point(px(0.), px(-offset)));
        window.request_animation_frame();
    }
    if state.fade.moving(now) || state.highlight.moving(now) || state.body.moving(now) {
        window.request_animation_frame();
    }
    let (highlight, body_height) = (state.highlight.value(now), state.body.value(now));
    let open = state.open;

    let list = if hits > 0 {
        let mut rows: Vec<AnyElement> = Vec::new();
        for line in lines {
            rows.push(match line {
                Line::Heading(group) => heading(group.label()).into_any_element(),
                Line::Hit(hit, index) => result(shell, hit, index, index == selected, cx),
            });
        }
        Some(
            div()
                .id("palette-list")
                .h_full()
                .overflow_y_scroll()
                .track_scroll(&scroll)
                .child(
                    div()
                        .relative()
                        .px(px(10.))
                        .pb(px(LIST_END))
                        .child(
                            div()
                                .absolute()
                                .left(px(10.))
                                .right(px(10.))
                                .top(px(highlight))
                                .h(px(search::ROW_HEIGHT - 4.))
                                .rounded(px(10.))
                                .bg(rgba(0x232327ff)),
                        )
                        .children(rows),
                )
                .into_any_element(),
        )
    } else {
        message.map(|message| {
            div()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(theme::muted())
                .child(message)
                .into_any_element()
        })
    };

    let panel = div()
        .w(px(WIDTH))
        .flex()
        .flex_col()
        .rounded(px(16.))
        .border_1()
        .border_color(rgba(0x2a2a30ff))
        .bg(theme::panel())
        .overflow_hidden()
        .shadow_lg()
        .occlude()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(20.))
                .h(px(60.))
                .border_b_1()
                .border_color(theme::line())
                .text_color(theme::muted())
                .child(icon(IconName::Search, 18.))
                .child(div().flex_1().text_color(theme::text()).child(input))
                .child(key_cap("Échap")),
        )
        .child(
            div()
                .h(px(body_height))
                .overflow_hidden()
                .children(list)
                .children(failure.map(|why| {
                    div()
                        .px(px(20.))
                        .py(px(8.))
                        .text_size(px(12.))
                        .text_color(theme::danger())
                        .child(why)
                })),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .px(px(20.))
                .h(px(44.))
                .border_t_1()
                .border_color(theme::line())
                .text_size(px(11.))
                .text_color(theme::dim())
                .child(hint(&["↑", "↓"], "choisir"))
                .child(hint(&["Entrée"], "ouvrir"))
                .child(hint(&["Échap"], "fermer")),
        );

    Some(
        div()
            .id("palette")
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .child(
                // Everything under the palette is out of reach; a click on it closes the palette.
                div()
                    .id("palette-backdrop")
                    .absolute()
                    .size_full()
                    .occlude()
                    .bg(rgba((0x8c as f32 * level) as u32))
                    .when(open, |backdrop| {
                        backdrop.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|shell, _, window, cx| shell.close_search(window, cx)),
                        )
                    }),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_start()
                    .justify_center()
                    .pt(px(search::PANEL_TOP - SLIDE * (1. - level)))
                    .opacity(level)
                    .child(panel),
            )
            .into_any_element(),
    )
}

fn heading(label: &'static str) -> Div {
    div()
        .h(px(search::HEADING_HEIGHT))
        .flex()
        .items_end()
        .px(px(10.))
        .pb(px(8.))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme::dim())
        .child(label.to_uppercase())
}

/// One result: a small image, the title and its line; "Entrée" on the highlighted one.
fn result(
    shell: &mut Shell,
    hit: Hit,
    index: usize,
    selected: bool,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let disabled = matches!(hit, Hit::Series(_)) && !SERIES_PAGE;
    let (image, round) = match &hit {
        Hit::Film(film) => (film.poster.clone(), false),
        Hit::Series(series) => (series.poster.clone(), false),
        Hit::Person(person) => (person.photo.clone(), true),
    };
    let image = shell.image(image.as_ref(), images::THUMB, cx);
    let (width, radius) = if round { (42., 21.) } else { (36., 5.) };
    let caption = match (hit.caption(), disabled) {
        (caption, false) => caption,
        (caption, true) if caption.is_empty() => "page à venir".to_string(),
        (caption, true) => format!("{caption} · page à venir"),
    };
    let title = hit.title().to_string();
    let initials = crate::film_page::initials(&title);
    div()
        .id(("palette-result", index))
        .h(px(search::ROW_HEIGHT - 4.))
        .mb(px(4.))
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .when(disabled, |row| row.opacity(0.5))
        .on_mouse_move(cx.listener(move |shell, _: &MouseMoveEvent, _, cx| {
            shell.search_hover(index, cx);
        }))
        .on_click(cx.listener(move |shell, _, window, cx| {
            shell.open_hit(hit.clone(), window, cx);
        }))
        .child(
            div()
                .relative()
                .flex_none()
                .w(px(width))
                .h(px(if round { width } else { 54. }))
                .rounded(px(radius))
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .bg(theme::line())
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::muted())
                .when(round, |disc| disc.child(initials))
                .children(picture(image, radius)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(title),
                )
                .when(!caption.is_empty(), |text| {
                    text.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .truncate()
                            .child(caption),
                    )
                }),
        )
        .when(selected && !disabled, |row| row.child(key_cap("Entrée")))
        .into_any_element()
}

/// A key as the design draws it: small, outlined.
fn key_cap(label: &'static str) -> Div {
    div()
        .flex_none()
        .px(px(5.))
        .py(px(1.))
        .rounded(px(4.))
        .border_1()
        .border_color(rgba(0x33333aff))
        .text_size(px(10.))
        .text_color(theme::dim())
        .child(label)
}

fn hint(keys: &[&'static str], what: &'static str) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .children(keys.iter().map(|key| key_cap(key)))
        .child(what)
}
