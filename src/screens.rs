//! The screens, and the pieces several of them show: posters, the rows under a heading, the
//! messages of a page that is loading or failed.

mod film_detail;
mod home;
mod library;
mod login;
pub mod player;

use bobine::provider::Film;
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, IntoElement, ObjectFit, Stateful, Window, div,
    img, px, rgba,
};

use crate::film_page;
use crate::icons::{IconName, icon};
use crate::images;
use crate::navigation::Screen;
use crate::shell::Shell;
use crate::theme;

pub fn render(
    shell: &mut Shell,
    screen: Screen,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    match screen {
        Screen::Login => login::render(shell, cx).into_any_element(),
        Screen::Home => home::render(shell, cx).into_any_element(),
        Screen::Library => library::render(shell, window, cx).into_any_element(),
        Screen::FilmDetail => film_detail::render(shell, window, cx).into_any_element(),
        Screen::Player => player::empty(cx).into_any_element(),
    }
}

/// The frame of a screen with only a few things to show: centred.
fn centered() -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.))
}

/// The whole body of a screen with nothing else to offer.
fn empty_screen(title: &'static str) -> impl IntoElement {
    centered().child(
        div()
            .text_size(px(28.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(title),
    )
}

/// A line of a page: loading, empty, or what went wrong.
fn note(text: impl Into<gpui_kit::SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(theme::dim())
        .child(div().size(px(7.)).rounded_full().bg(theme::accent()))
        .child(text.into())
}

/// A row's heading: `Reprendre`, `Distribution`, `Titres proches`.
fn row_heading(title: impl Into<gpui_kit::SharedString>) -> Div {
    div()
        .text_size(px(18.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}

/// An image filling its box, once it is here; the box's own colour until then. The box does
/// not clip its children to its rounded corners: the image is rounded by `radius` itself.
fn picture(
    image: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    radius: f32,
) -> Option<AnyElement> {
    image.map(|image| {
        img(image)
            .rounded(px(radius))
            .absolute()
            .inset_0()
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element()
    })
}

/// A small dark pill over a poster.
fn badge(text: impl Into<gpui_kit::SharedString>) -> Div {
    div()
        .px(px(7.))
        .py(px(3.))
        .rounded(px(6.))
        .bg(rgba(0x09090acc))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// A film as the grid and the rows show it: the poster, its rating top left, a check when
/// watched, the title and the year below. `outlined`: the film of the page, in its collection.
fn poster(
    shell: &mut Shell,
    film: &Film,
    width: f32,
    id: impl Into<ElementId>,
    outlined: bool,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let image = shell.image(film.poster.as_ref(), images::POSTER, cx);
    let opened = film.id.clone();
    let mut caption: Vec<String> = film.year.iter().map(i32::to_string).collect();
    if outlined {
        caption.push("ce film".into());
    }
    div()
        .id(id)
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .when(!outlined, |card| {
            card.on_click(cx.listener(move |shell, _, _, cx| {
                shell.open_film(opened.clone(), cx);
            }))
        })
        .child(
            div()
                .relative()
                .w(px(width))
                .h(px(width * 1.5))
                .rounded(px(theme::RADIUS))
                .overflow_hidden()
                .bg(theme::panel())
                .border_2()
                .border_color(if outlined {
                    theme::text()
                } else {
                    rgba(0x00000000)
                })
                .children(picture(image, theme::RADIUS))
                .children(
                    film_page::rating(film)
                        .map(|rating| badge(rating).absolute().top(px(8.)).left(px(8.))),
                )
                .when(film.watched, |poster| {
                    poster.child(
                        div()
                            .absolute()
                            .top(px(8.))
                            .right(px(8.))
                            .size(px(22.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(theme::accent())
                            .text_color(theme::background())
                            .child(icon(IconName::Check, 13.)),
                    )
                }),
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
                        .child(film.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .child(caption.join(" · ")),
                ),
        )
}

/// A thin bar: how far the viewer went.
fn progress_bar(fraction: f32, width: f32) -> Div {
    div()
        .w(px(width))
        .h(px(3.))
        .rounded_full()
        .bg(rgba(0xffffff33))
        .child(
            div()
                .h_full()
                .w(px(width * fraction.clamp(0., 1.)))
                .rounded_full()
                .bg(theme::accent()),
        )
}
