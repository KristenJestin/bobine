//! The Stats page: the hours of this week, month and year, this year in one card, the titles and
//! genres watched most, and when, as quiet monochrome bars. Each part fades in a little after
//! the one above it, its bars growing.

use std::rc::Rc;

use bobine::provider::{HistorySource, WatchedKind};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, Context, Div, ElementId, FontWeight, IntoElement,
    ScrollHandle, Stateful, Window, div, px,
};

use super::card::{Offer, Open, Play};
use super::film_detail::{EDGE, ROW_POSTER};
use super::library::GRID_TOP;
use super::{Corner, note, poster_caption, poster_image, row_heading};
use crate::home_page::PlayTarget;
use crate::images;
use crate::motion;
use crate::shell::Shell;
use crate::stats::{self, Ranked, Stats};
use crate::theme;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
/// How tall the activity bars are at most.
const CHART_HEIGHT: f32 = 120.;

/// What the page says of where its numbers come from.
pub fn source_note(source: HistorySource) -> &'static str {
    match source {
        HistorySource::PlaybackReporting => "From the server's Playback Reporting plugin.",
        HistorySource::PlayedItems => {
            "From your played items: each title counts its length once per play. A season marked \
             played at once counts as watched that day."
        }
    }
}

pub fn render(
    shell: &mut Shell,
    _window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let Some(stats) = shell.stats.clone() else {
        return div().into_any_element();
    };
    let [scroll, movies, series] = shell.stats_scrolls.clone();
    shell
        .scrolls
        .extend([scroll.clone(), movies.clone(), series.clone()]);
    let header = div()
        .px(px(EDGE))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Stats"),
        )
        .child(
            div()
                .debug_selector(|| "stats-source".into())
                .text_size(px(13.))
                .text_color(theme::dim())
                .child(source_note(stats.source)),
        );
    let mut parts: Vec<AnyElement> = vec![
        periods(&stats).into_any_element(),
        review(&stats).into_any_element(),
    ];
    for (kind, heading, row) in [
        (WatchedKind::Movie, "Most watched movies", movies),
        (WatchedKind::Series, "Most watched series", series),
    ] {
        let list = match kind {
            WatchedKind::Movie => &stats.movies,
            WatchedKind::Series => &stats.series,
        };
        if !list.is_empty() {
            parts.push(titles(shell, heading, kind, list, &row, cx).into_any_element());
        }
    }
    if !stats.genres.is_empty() {
        parts.push(genres(&stats).into_any_element());
    }
    parts.push(activity(&stats).into_any_element());

    div()
        .debug_selector(|| "stats-page".into())
        .id("stats")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(36.))
                .pt(px(GRID_TOP + 24.))
                .pb(px(64.))
                .child(header)
                .children(parts.into_iter().enumerate().map(|(at, part)| {
                    div().child(part).with_animation(
                        ElementId::NamedInteger("stats-part".into(), at as u64),
                        Animation::new(motion::page()).with_easing(motion::ease_out),
                        |part, t| part.opacity(t),
                    )
                })),
        )
        .into_any_element()
}

/// A rounded card of the page's own panel colour.
fn card() -> Div {
    div()
        .p(px(16.))
        .rounded(px(theme::RADIUS_LARGE))
        .border_1()
        .border_color(theme::line())
        .bg(theme::panel())
}

/// A label over its value.
fn fact(label: &'static str, value: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .min_w_0()
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme::dim())
                .child(label),
        )
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(value),
        )
}

fn periods(stats: &Stats) -> Div {
    let tiles = [
        ("This week", stats.week),
        ("This month", stats.month),
        ("This year", stats.year),
    ];
    div()
        .debug_selector(|| "stats-periods".into())
        .px(px(EDGE))
        .flex()
        .gap(px(16.))
        .children(
            tiles
                .into_iter()
                .map(|(label, seconds)| card().flex_1().child(fact(label, stats::length(seconds)))),
        )
}

fn review(stats: &Stats) -> Div {
    let review = &stats.review;
    let body = if review.seconds == 0 {
        div().child(note("Nothing watched yet this year."))
    } else {
        let dash = || "—".to_string();
        div()
            .flex()
            .gap(px(24.))
            .child(fact("Watched", stats::length(review.seconds)).flex_1())
            .child(fact("Top title", review.top_title.clone().unwrap_or_else(dash)).flex_1())
            .child(fact("Top genre", review.top_genre.clone().unwrap_or_else(dash)).flex_1())
            .child(
                fact(
                    "Busiest month",
                    review
                        .busiest_month
                        .map_or_else(dash, |month| stats::month_name(month).to_string()),
                )
                .flex_1(),
            )
    };
    div().px(px(EDGE)).child(
        card()
            .debug_selector(|| "stats-review".into())
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("{} in review", review.year)),
            )
            .child(body),
    )
}

/// A row of posters that scrolls sideways; each is a card, as on the other pages, when the
/// library holds its title.
fn titles(
    shell: &mut Shell,
    heading: &'static str,
    kind: WatchedKind,
    list: &[Ranked],
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> Div {
    let row_id = match kind {
        WatchedKind::Movie => "stats-movies",
        WatchedKind::Series => "stats-series",
    };
    let posters: Vec<AnyElement> = list
        .iter()
        .enumerate()
        .map(|(at, ranked)| {
            let image = shell.image(ranked.title.poster.as_ref(), images::POSTER, cx);
            let name = ElementId::NamedInteger(row_id.into(), at as u64);
            let plays = match ranked.plays {
                1 => "1 play".to_string(),
                plays => format!("{plays} plays"),
            };
            let (title, caption) = (
                ranked.title.name.clone(),
                format!("{} · {plays}", stats::length(ranked.seconds)),
            );
            let draw = {
                let name = name.clone();
                move |layer: Option<Stateful<Div>>| {
                    div()
                        .w(px(ROW_POSTER))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            poster_image(
                                &name,
                                image.clone(),
                                None,
                                Corner::Nothing,
                                ROW_POSTER,
                                false,
                            )
                            .children(layer),
                        )
                        .child(poster_caption(title.clone(), caption.clone()))
                }
            };
            // A title the library lost opens nothing: it is no card for the arrows.
            let Some(id) = ranked.title.id.clone() else {
                return div()
                    .id(name)
                    .flex_none()
                    .child(draw(None))
                    .into_any_element();
            };
            let (open, play) = match kind {
                WatchedKind::Movie => (Open::Film(id.clone()), PlayTarget::Film(id)),
                WatchedKind::Series => (Open::Series(id.clone(), None), PlayTarget::Series(id)),
            };
            let offer = Offer {
                open,
                play: Play::Target(play),
                quick: None,
                draw: Rc::new(move |layer| draw(Some(layer))),
            };
            super::card(shell, name, offer, cx).into_any_element()
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(div().px(px(EDGE)).child(row_heading(heading)))
        .child(
            div()
                .id(row_id)
                .debug_selector(move || row_id.into())
                .flex()
                .gap(px(16.))
                .px(px(EDGE))
                .overflow_x_scroll()
                .track_scroll(scroll)
                .children(posters),
        )
}

/// A bar `fraction` of the way, growing into place.
fn grown(id: ElementId, bar: Div, grow: impl Fn(Div, f32) -> Div + 'static) -> AnyElement {
    bar.with_animation(
        id,
        Animation::new(motion::page() * 2).with_easing(motion::ease_out),
        grow,
    )
    .into_any_element()
}

fn genres(stats: &Stats) -> Div {
    let most = stats
        .genres
        .first()
        .map_or(1, |(_, seconds)| *seconds)
        .max(1);
    let lines = stats
        .genres
        .iter()
        .enumerate()
        .map(|(at, (genre, seconds))| {
            let fraction = *seconds as f32 / most as f32;
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(
                    div()
                        .w(px(140.))
                        .flex_none()
                        .truncate()
                        .text_size(px(14.))
                        .child(genre.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .h(px(8.))
                        .rounded_full()
                        .bg(theme::line())
                        .child(grown(
                            ElementId::NamedInteger("stats-genre".into(), at as u64),
                            div().h_full().rounded_full().bg(theme::muted()),
                            move |bar, t| bar.w(gpui_kit::relative(fraction * t)),
                        )),
                )
                .child(
                    div()
                        .w(px(110.))
                        .flex_none()
                        .text_size(px(13.))
                        .text_color(theme::dim())
                        .child(stats::length(*seconds)),
                )
        });
    div()
        .px(px(EDGE))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(row_heading("Genres"))
        .child(
            card()
                .debug_selector(|| "stats-genres".into())
                .flex()
                .flex_col()
                .gap(px(12.))
                .children(lines),
        )
}

fn activity(stats: &Stats) -> Div {
    let charts = match (stats.days, stats.hours) {
        (Some(days), Some(hours)) => div()
            .flex()
            .gap(px(16.))
            .child(
                card()
                    .debug_selector(|| "stats-days".into())
                    .flex_1()
                    .child(chart("By day of the week", "stats-day", &days, |at| {
                        Some(WEEKDAYS[at])
                    })),
            )
            .child(
                card()
                    .debug_selector(|| "stats-hours".into())
                    // The hours have more bars: twice the room.
                    .flex_grow(2.)
                    .flex_basis(px(0.))
                    .min_w_0()
                    .child(chart("By hour", "stats-hour", &hours, |at| {
                        [(0, "00:00"), (6, "06:00"), (12, "12:00"), (18, "18:00")]
                            .iter()
                            .find(|(hour, _)| *hour == at)
                            .map(|(_, label)| *label)
                    })),
            ),
        _ => card()
            .debug_selector(|| "stats-no-activity".into())
            .child(note(
                "The days and hours need the server's Playback Reporting plugin: \
             played items do not say when they were watched.",
            )),
    };
    div()
        .px(px(EDGE))
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(row_heading("Activity"))
        .child(charts)
}

/// Vertical bars of `values`, each with its label under it when it has one.
fn chart(
    title: &'static str,
    id: &'static str,
    values: &[u64],
    label: impl Fn(usize) -> Option<&'static str>,
) -> Div {
    let most = values.iter().copied().max().unwrap_or(0).max(1);
    let top = values.iter().copied().max().unwrap_or(0);
    let bars = values.iter().enumerate().map(|(at, value)| {
        let fraction = *value as f32 / most as f32;
        let height = CHART_HEIGHT * fraction;
        let colour = if *value == top && top > 0 {
            theme::text()
        } else {
            theme::muted()
        };
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .h(px(CHART_HEIGHT))
                    .w_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .items_center()
                    .child(grown(
                        ElementId::NamedInteger(id.into(), at as u64),
                        div()
                            .w_full()
                            .max_w(px(28.))
                            .rounded(px(3.))
                            .bg(colour)
                            .opacity(if *value == 0 { 0. } else { 0.85 }),
                        move |bar, t| bar.h(px((height * t).max(2.))),
                    )),
            )
            .child(
                div()
                    .h(px(14.))
                    .text_size(px(11.))
                    .text_color(theme::dim())
                    .whitespace_nowrap()
                    .children(label(at)),
            )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(div().flex().items_end().gap(px(4.)).children(bars))
}
