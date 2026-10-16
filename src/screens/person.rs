//! What a person took part in, reached from a search: their photo and name, then the films and
//! the series, as posters.

use bobine::provider::SeriesHit;
use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, FontWeight, IntoElement, Stateful, Window, div, px};

use super::{note, picture, poster, row_heading};
use crate::film_page;
use crate::images;
use crate::lists::plural;
use crate::shell::{Loadable, Shell};
use crate::theme;

const EDGE: f32 = 40.;
const POSTER: f32 = 160.;
const PHOTO: f32 = 88.;

pub fn render(shell: &mut Shell, _: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.people.last() else {
        return div().into_any_element();
    };
    let (name, photo_ref, scroll) = (page.name.clone(), page.photo.clone(), page.scroll.clone());
    let titles = match &page.titles {
        Loadable::Ready(titles) => Ok(titles.clone()),
        Loadable::Failed(why) => Err(*why),
        Loadable::Loading => Err("Loading…"),
    };
    let photo = shell.image(photo_ref.as_ref(), images::PHOTO, cx);

    let (counts, body) = match titles {
        Ok(titles) => {
            let mut sections: Vec<Div> = Vec::new();
            let counts = format!(
                "{} · {}",
                plural(titles.films.len(), "film", "films"),
                plural(titles.series.len(), "series", "series")
            );
            if !titles.films.is_empty() {
                let cards: Vec<_> = titles
                    .films
                    .iter()
                    .enumerate()
                    .map(|(index, film)| {
                        poster(shell, film, POSTER, ("person-film", index), false, cx)
                            .into_any_element()
                    })
                    .collect();
                sections.push(section("Films", cards));
            }
            if !titles.series.is_empty() {
                let cards: Vec<_> = titles
                    .series
                    .iter()
                    .enumerate()
                    .map(|(index, series)| series_card(shell, series, index, cx).into_any_element())
                    .collect();
                sections.push(section("Series", cards));
            }
            if sections.is_empty() {
                sections.push(div().child(note("No titles for this person.")));
            }
            (Some(counts), sections)
        }
        Err(why) => (None, vec![div().child(note(why))]),
    };

    div()
        .id("person-page")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(32.))
                // Under the back link, itself under the pill bar.
                .pt(px(120.))
                .px(px(EDGE))
                .pb(px(64.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(20.))
                        .child(
                            div()
                                .relative()
                                .flex_none()
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
                                .child(film_page::initials(&name))
                                .children(picture(photo, PHOTO / 2.)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.))
                                .child(
                                    div()
                                        .text_size(px(28.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name),
                                )
                                .children(counts.map(|counts| {
                                    div()
                                        .text_size(px(14.))
                                        .text_color(theme::dim())
                                        .child(counts)
                                })),
                        ),
                )
                .children(body),
        )
        .into_any_element()
}

/// A heading and its titles, wrapping onto as many lines as they need.
fn section(title: &'static str, cards: Vec<gpui_kit::AnyElement>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(row_heading(title))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_x(px(16.))
                .gap_y(px(24.))
                .children(cards),
        )
}

/// A series as a poster: it opens the series page.
fn series_card(
    shell: &mut Shell,
    series: &SeriesHit,
    index: usize,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let image = shell.image(series.poster.as_ref(), images::POSTER, cx);
    let opened = series.id.clone();
    div()
        .id(("person-series", index))
        .w(px(POSTER))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| {
            shell.open_series(opened.clone(), cx);
        }))
        .child(
            div()
                .relative()
                .w(px(POSTER))
                .h(px(POSTER * 1.5))
                .rounded(px(theme::RADIUS))
                .overflow_hidden()
                .bg(theme::panel())
                .children(picture(image, theme::RADIUS)),
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
                        .child(series.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .child(series.year.map(|year| year.to_string()).unwrap_or_default()),
                ),
        )
}
