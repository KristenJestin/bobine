//! A collection's page: the film page's hero (the collection's image, its name, the facts, the
//! synopsis), then its films as posters in release order, each with its watched or progress mark.

use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, Window, div, px, rgba};

use super::film_detail::{EDGE, hero_frame, hero_text, meta_line};
use super::{centered, note, row_heading, started_poster};
use crate::images;
use crate::lists;
use crate::shell::{Loadable, Shell};

const POSTER: f32 = 170.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.collection_pages.last() else {
        return centered().into_any_element();
    };
    let scroll = page.scroll.clone();
    let collection = match &page.detail {
        Loadable::Ready(collection) => collection.clone(),
        Loadable::Failed(why) => return centered().child(note(*why)).into_any_element(),
        Loadable::Loading => return centered().child(note("Loading…")).into_any_element(),
    };
    let height = (f32::from(window.viewport_size().height) * 0.5).clamp(360., 520.);
    // A collection with no image of its own takes the first film's.
    let backdrop = collection.backdrop.clone().or_else(|| {
        collection
            .films
            .iter()
            .find_map(|film| film.backdrop.clone())
    });
    let backdrop = shell.image(backdrop.as_ref(), images::BACKDROP, cx);

    let hero = hero_frame(backdrop, height).child(
        hero_text(collection.name.clone())
            .child(meta_line(lists::meta(&collection)))
            .children(collection.overview.clone().map(|overview| {
                div()
                    .text_size(px(15.))
                    .line_height(px(23.))
                    .line_clamp(3)
                    .text_color(rgba(0xf4f4f5d9))
                    .child(overview)
            })),
    );
    let posters: Vec<_> = collection
        .films
        .iter()
        .enumerate()
        .map(|(index, film)| started_poster(shell, film, POSTER, ("collection-film", index), cx))
        .collect();

    div()
        .id("collection-page")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .child(hero)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .px(px(EDGE))
                .pt(px(12.))
                .pb(px(64.))
                .child(row_heading("Movies"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_x(px(16.))
                        .gap_y(px(24.))
                        .children(posters),
                ),
        )
        .into_any_element()
}
