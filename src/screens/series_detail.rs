//! A series' page: the film page's hero, whose button plays the next episode; the seasons in
//! tabs and the episodes of the one chosen (still, number and title, runtime, synopsis, watched
//! or how far); below, the cast and similar series. The page scrolls; over it, after an episode
//! ended, the "Next episode" card counts down.

use std::rc::Rc;
use std::time::Instant;

use bobine::provider::{Episode, Season, SeriesDetail};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, Context, Div, FontWeight, IntoElement, Stateful, Window,
    div, px, rgb, rgba,
};

use super::card::{Offer, Open, Play};
use super::film_detail::{
    EDGE, MenuState, ROW_POSTER, SERIES_TRAILER, TEXT_WIDTH, cast_row, chips_row, facts_line,
    heading, hero_frame, hero_text_under, list_button, row, trailer_button,
};
use super::{card, centered, note, picture, progress_bar, row_heading, series_poster};
use crate::film_page::{self, Choices};
use crate::icons::{IconName, icon};
use crate::images;
use crate::motion;
use crate::navigation::NavEntry;
use crate::playlists;
use crate::series_page::{self, Next};
use crate::shell::{Loadable, SeriesPage, Shell};
use crate::theme;
use crate::trailer;

const STILL_WIDTH: f32 = 190.;
const STILL_HEIGHT: f32 = 107.;
const CARD_WIDTH: f32 = 360.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.series_pages.last_mut() else {
        return centered().into_any_element();
    };
    let detail = match &page.detail {
        Loadable::Ready(detail) => detail.clone(),
        Loadable::Failed(why) => return centered().child(note(*why)).into_any_element(),
        Loadable::Loading => return centered().child(note("Loading…")).into_any_element(),
    };
    let now = Instant::now();
    let open = page.menus.open();
    let menu = MenuState {
        open,
        drawn: page.menu_fade.follow(open, now),
    };
    if page.menu_fade.moving(now) {
        window.request_animation_frame();
    }
    let shown = Shown::of(page);
    let scroll = page.scroll.clone();
    let [cast_scroll, similar_scroll] = page.rows.clone();
    shell
        .scrolls
        .extend([scroll.clone(), similar_scroll.clone()]);
    let height = f32::from(window.viewport_size().height);

    let hero = hero(shell, &detail, &shown, menu, height, window, cx);
    let seasons = seasons(shell, &shown, window, cx);
    let mut rows: Vec<AnyElement> = Vec::new();
    if !detail.cast.is_empty() {
        rows.push(cast_row(shell, &detail.cast, &cast_scroll, window, cx).into_any_element());
    }
    if !detail.similar.is_empty() {
        let cards: Vec<Stateful<Div>> = detail
            .similar
            .iter()
            .enumerate()
            .map(|(index, series)| {
                series_poster(shell, series, ROW_POSTER, ("similar-series", index), cx)
            })
            .collect();
        rows.push(row("Similar series", None, cards, &similar_scroll).into_any_element());
    }

    // Scrolled by hand, like the film page: the page up and down, a row sideways.
    div()
        .id("series-page")
        .size_full()
        .overflow_hidden()
        .track_scroll(&scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| shell.scroll_series_page(event, window, cx)),
        )
        .child(hero)
        .child(seasons)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(40.))
                .pt(px(48.))
                .pb(px(64.))
                .children(rows),
        )
        .into_any_element()
}

/// What the page shows, read once per frame.
struct Shown {
    seasons: Vec<Season>,
    season: usize,
    episodes: Option<Result<Vec<Episode>, &'static str>>,
    next: Option<Episode>,
    choices: Option<Choices>,
    chips: [super::film_detail::ChipBounds; 3],
    error: Option<&'static str>,
    busy: bool,
    favourite: bool,
}

impl Shown {
    fn of(page: &SeriesPage) -> Self {
        Self {
            seasons: page.seasons.clone(),
            season: page.season,
            episodes: page.shown_episodes().and_then(|episodes| match episodes {
                Loadable::Ready(episodes) => Some(Ok(episodes.clone())),
                Loadable::Failed(why) => Some(Err(*why)),
                Loadable::Loading => None,
            }),
            next: page.next.clone(),
            choices: page.choices.clone(),
            chips: page.chips.clone(),
            error: page.error,
            busy: page.waiting(),
            favourite: page.favourite.on(),
        }
    }
}

fn hero(
    shell: &mut Shell,
    detail: &SeriesDetail,
    shown: &Shown,
    menu: MenuState,
    window_height: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let series = &detail.series;
    let backdrop = shell.image(series.backdrop.as_ref(), images::BACKDROP, cx);
    let height = (window_height * 0.62).clamp(440., 680.);
    let play =
        shown.next.clone().map(|next| {
            let label = series_page::play_label(&next);
            theme::primary_button("series-play", Some(IconName::Play), label, window, cx)
                .when(shown.busy, |button| button.opacity(0.6))
                .on_click(cx.listener(move |shell, _, window, cx| {
                    shell.play_episode(next.clone(), window, cx)
                }))
        });
    let started = shown.next.as_ref().and_then(|next| {
        let fraction = film_page::progress(&next.item)?;
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(progress_bar(fraction, 300.))
                .children(film_page::time_left(&next.item).map(|left| {
                    div()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(left)
                })),
        )
    });
    let chips = shown
        .next
        .as_ref()
        .zip(shown.choices.as_ref())
        .map(|(next, choices)| chips_row(&next.versions, choices, menu, &shown.chips, window, cx));
    let title = heading(shell, &series.title, detail.logo.as_ref(), window, cx);
    hero_frame(backdrop, height).child(
        hero_text_under(title)
            .child(facts_line(
                series_page::meta(detail, shown.seasons.len()),
                NavEntry::Series,
                shell.font(),
                window,
                cx,
            ))
            .children(detail.overview.clone().map(|overview| {
                div()
                    .text_size(px(15.))
                    .line_height(px(23.))
                    .line_clamp(3)
                    .text_color(rgba(0xf4f4f5d9))
                    .child(overview)
            }))
            .child(
                div()
                    .pt(px(6.))
                    .flex()
                    .gap(px(10.))
                    .children(play)
                    .children(
                        trailer::choose(&detail.trailers, shell.ytdlp)
                            .map(|_| trailer_button(SERIES_TRAILER, window, cx)),
                    )
                    .child(list_button("series-list", shown.favourite, window, cx))
                    .child(super::playlist::playlist_button(
                        "series-playlist",
                        series.id.clone(),
                        true,
                        window,
                        cx,
                    )),
            )
            .children(started)
            .children(shown.error.map(|why| {
                div()
                    .text_size(px(13.))
                    .text_color(theme::danger())
                    .child(why)
            }))
            .children(chips),
    )
}

/// The tabs of the seasons, the season's check, and its episodes.
fn seasons(shell: &mut Shell, shown: &Shown, window: &mut Window, cx: &mut Context<Shell>) -> Div {
    let tabs = shown
        .seasons
        .iter()
        .enumerate()
        .map(|(index, season)| {
            let selected = index == shown.season;
            let label = series_page::season_label(season);
            theme::eased(("season-tab", index), theme::Shades::line(), window, cx)
                .relative()
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(14.))
                .py(px(7.))
                .rounded_full()
                .border_1()
                .border_color(theme::line())
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .on_click(cx.listener(move |shell, _, _, cx| shell.select_season(index, cx)))
                // The tab chosen turns white, eased in.
                .when(selected, |tab| {
                    tab.text_color(rgb(0x000000)).child(
                        div()
                            .id(("season-tab-on", index))
                            .absolute()
                            .inset_0()
                            .rounded_full()
                            .bg(theme::text())
                            .with_animation(
                                ("season-tab-on", index),
                                Animation::new(motion::quick()).with_easing(motion::ease_out),
                                |layer, t| layer.opacity(t),
                            ),
                    )
                })
                .child(div().relative().child(label))
                .when(season.unwatched == 0, |tab| {
                    tab.child(div().relative().child(icon(IconName::Check, 13.)))
                })
        })
        .collect::<Vec<_>>();
    let season = shown.seasons.get(shown.season);
    let all_watched = season.is_some_and(|s| s.unwatched == 0);
    let season_check = season.map(|_| {
        let label = if all_watched {
            "Season watched"
        } else {
            "Mark season as watched"
        };
        theme::ghost_button("season-watched", Some(IconName::Check), label, window, cx)
            .px(px(14.))
            .py(px(7.))
            .text_size(px(13.))
            .when(all_watched, |button| {
                button
                    .text_color(theme::accent())
                    .border_color(theme::accent())
            })
            .when(shown.busy, |button| button.opacity(0.6))
            .on_click(cx.listener(|shell, _, _, cx| shell.toggle_season_watched(cx)))
    });

    let list: AnyElement = match &shown.episodes {
        None => note("Loading…").into_any_element(),
        Some(Err(why)) => note(*why).into_any_element(),
        Some(Ok(episodes)) if episodes.is_empty() => {
            note("No episodes in this season.").into_any_element()
        }
        Some(Ok(episodes)) => {
            let next = shown.next.as_ref().map(|e| e.item.id.clone());
            let rows: Vec<Stateful<Div>> = episodes
                .iter()
                .enumerate()
                .map(|(index, episode)| {
                    let is_next = next.as_ref() == Some(&episode.item.id);
                    episode_row(shell, episode, index, is_next, window, cx)
                })
                .collect();
            // A season's episodes come in from a little lower, and fade in.
            div()
                .id(("episodes", shown.season))
                .flex()
                .flex_col()
                .gap(px(6.))
                .children(rows)
                .with_animation(
                    ("episodes", shown.season),
                    Animation::new(motion::page()).with_easing(motion::ease_out),
                    |list, t| list.opacity(t).mt(px(12. * (1. - t))),
                )
                .into_any_element()
        }
    };

    div()
        .flex()
        .flex_col()
        .gap(px(18.))
        .px(px(EDGE))
        .pt(px(8.))
        .child(row_heading("Episodes"))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(div().flex().flex_wrap().gap(px(6.)).children(tabs))
                .children(season_check),
        )
        .child(list)
}

/// An episode: its still (a check when watched, a bar when started), its number, title,
/// runtime and synopsis, and its own watched check. A click plays it from where it stopped.
fn episode_row(
    shell: &mut Shell,
    episode: &Episode,
    index: usize,
    is_next: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let still = shell.image(episode.item.poster.as_ref(), images::CARD, cx);
    let item = &episode.item;
    let title = match episode.number {
        Some(number) => format!("{number} · {}", item.title),
        None => item.title.clone(),
    };
    let played = episode.clone();
    let marked = episode.clone();
    // The pointer over the picture brings the veil and the play button of the cards; the click
    // anywhere on the row, the picture included, plays.
    let progress = film_page::progress(item);
    let draw = move |layer: Stateful<Div>| {
        div()
            .relative()
            .flex_none()
            .w(px(STILL_WIDTH))
            .h(px(STILL_HEIGHT))
            .rounded(px(theme::RADIUS))
            .overflow_hidden()
            .bg(theme::panel())
            .children(picture(still.clone(), theme::RADIUS))
            .children(progress.map(|fraction| {
                div()
                    .absolute()
                    .left(px(8.))
                    .bottom(px(8.))
                    .child(progress_bar(fraction, STILL_WIDTH - 16.))
            }))
            .child(layer)
    };
    let offer = Offer {
        open: Open::Row(item.id.clone()),
        play: Play::Episode(Box::new(episode.clone())),
        quick: None,
        draw: Rc::new(draw),
    };
    let still = card(shell, ("episode-still", index).into(), offer, cx);
    let shades = theme::Shades {
        rest: if is_next {
            rgb(0x16161a)
        } else {
            rgba(0x16161a00)
        },
        hover: theme::line(),
        press: rgb(0x2e2e33),
    };
    let check = theme::eased(
        ("episode-watched", index),
        theme::Shades::veil(),
        window,
        cx,
    )
    .flex_none()
    .size(px(30.))
    .rounded_full()
    .flex()
    .items_center()
    .justify_center()
    .border_1()
    .cursor_pointer()
    .map(|check| {
        if item.watched {
            check
                .bg(theme::accent())
                .border_color(theme::accent())
                .text_color(theme::background())
        } else {
            check.border_color(rgb(0x3a3a40)).text_color(theme::dim())
        }
    })
    .child(icon(IconName::Check, 14.))
    .on_click(cx.listener(move |shell, _, _, cx| {
        // The check is not the row: it marks, it does not play.
        cx.stop_propagation();
        shell.toggle_episode_watched(marked.clone(), cx);
    }));
    theme::eased(("episode", index), shades, window, cx)
        .flex()
        .items_center()
        .gap(px(18.))
        .p(px(10.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(if is_next {
            theme::line()
        } else {
            rgba(0x00000000)
        })
        .cursor_pointer()
        .on_click(
            cx.listener(move |shell, _, window, cx| shell.play_episode(played.clone(), window, cx)),
        )
        .child(still)
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::dim())
                                .child(series_page::duration_line(episode)),
                        ),
                )
                .children(episode.overview.clone().map(|overview| {
                    div()
                        .max_w(px(TEXT_WIDTH + 200.))
                        .text_size(px(13.))
                        .line_height(px(19.))
                        .line_clamp(2)
                        .text_color(theme::muted())
                        .child(overview)
                })),
        )
        .child(check)
}

/// "Next episode", bottom right over the series page: the still, the episode, the seconds
/// left with a bar that empties, "Play" and "Cancel". Fades in and out.
pub fn up_next_card(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let now = Instant::now();
    let pending = shell.up_next.episode().cloned();
    let counting = pending.is_some();
    let (next, shown) = shell.up_next_fade.follow(pending, now)?;
    let left = shell.up_next.seconds_left(now);
    let fraction = shell.up_next.fraction_left(now);
    // The bar empties smoothly; nothing asks for frames once the card is gone.
    window.request_animation_frame();
    let (picture_of, heading, subtitle) = match &next {
        Next::Episode(episode) => (
            episode.item.poster.as_ref(),
            "NEXT EPISODE",
            series_page::player_title(episode).1,
        ),
        Next::Entry(entry) => (
            playlists::still(&entry.title),
            "Up next",
            playlists::name(&entry.title),
        ),
    };
    let still = shell.image(picture_of, images::CARD, cx);
    let play = theme::primary_button("up-next-play", Some(IconName::Play), "Play", window, cx)
        .px(px(16.))
        .on_click(cx.listener(|shell, _, window, cx| shell.play_up_next_now(window, cx)));
    let cancel = theme::ghost_button("up-next-cancel", None, "Cancel", window, cx)
        .px(px(16.))
        .on_click(cx.listener(|shell, _, _, cx| {
            shell.cancel_up_next(cx);
        }));
    Some(
        div()
            .absolute()
            .right(px(32.))
            .bottom(px(32. - 16. * (1. - shown)))
            .w(px(CARD_WIDTH))
            .opacity(shown)
            .when(counting, |card| card.occlude())
            .flex()
            .flex_col()
            .rounded(px(theme::RADIUS_LARGE))
            .border_1()
            .border_color(theme::line())
            .bg(theme::panel())
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(CARD_WIDTH * 9. / 16.))
                    .rounded_t(px(theme::RADIUS_LARGE))
                    .bg(theme::line())
                    .children(picture(still, theme::RADIUS_LARGE))
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .bottom_0()
                            .h(px(3.))
                            .w(px(CARD_WIDTH * fraction))
                            .bg(theme::accent()),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(16.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::dim())
                            .child(heading),
                    )
                    .child(
                        div()
                            .text_size(px(15.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(subtitle),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::muted())
                            .child(format!("In {left}s")),
                    )
                    .child(
                        div()
                            .pt(px(8.))
                            .flex()
                            .gap(px(8.))
                            .child(play)
                            .child(cancel),
                    ),
            )
            .into_any_element(),
    )
}
