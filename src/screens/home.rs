//! Home, as tranche 1 has it: under the pill bar, the "Reprendre" row of Continue Watching. The
//! large rotating header comes with tranche 2.

use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Div, FontWeight, IntoElement, Stateful, div, linear_color_stop, linear_gradient, px,
    rgba,
};

use super::{note, picture, progress_bar, row_heading};
use crate::film_page;
use crate::images;
use crate::shell::{Loadable, Shell};
use crate::theme;

/// A card of the row, 16:9.
const CARD_WIDTH: f32 = 340.;

pub fn render(shell: &mut Shell, cx: &mut Context<Shell>) -> impl IntoElement {
    let films = match &shell.home.resume {
        Some(Loadable::Ready(films)) => Ok(films.clone()),
        Some(Loadable::Failed(why)) => Err(*why),
        Some(Loadable::Loading) | None => Err("Chargement…"),
    };
    let row = match films {
        Ok(films) if films.is_empty() => {
            note("Rien à reprendre pour l'instant.").into_any_element()
        }
        Ok(films) => {
            let cards: Vec<_> = films
                .iter()
                .enumerate()
                .map(|(index, film)| card(shell, film, index, cx))
                .collect();
            div()
                .id("resume-row")
                .flex()
                .gap(px(16.))
                .overflow_x_scroll()
                .children(cards)
                .into_any_element()
        }
        Err(why) => note(why).into_any_element(),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .gap(px(16.))
        .pt(px(104.))
        .px(px(40.))
        .child(row_heading("Reprendre"))
        .child(row)
}

fn card(
    shell: &mut Shell,
    film: &bobine::provider::Film,
    index: usize,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let image = shell.image(
        film.backdrop.as_ref().or(film.poster.as_ref()),
        images::CARD,
        cx,
    );
    let opened = film.id.clone();
    div()
        .id(("resume-card", index))
        .w(px(CARD_WIDTH))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(10.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| shell.open_film(opened.clone(), cx)))
        .child(
            div()
                .relative()
                .w(px(CARD_WIDTH))
                .h(px(CARD_WIDTH * 9. / 16.))
                .rounded(px(theme::RADIUS_LARGE))
                .overflow_hidden()
                .bg(theme::panel())
                .children(picture(image, theme::RADIUS_LARGE))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(70.))
                        .bg(linear_gradient(
                            0.,
                            linear_color_stop(rgba(0x000000b3), 0.),
                            linear_color_stop(rgba(0x00000000), 1.),
                        )),
                )
                .children(film_page::progress(film).map(|fraction| {
                    div()
                        .absolute()
                        .left(px(14.))
                        .bottom(px(14.))
                        .child(progress_bar(fraction, CARD_WIDTH - 28.))
                })),
        )
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(film.title.clone()),
                )
                .children(film_page::time_left(film).map(|left| {
                    div()
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(left)
                })),
        )
}
