//! The screens, and the pieces several of them show: posters, the rows under a heading, the
//! messages of a page that is loading or failed.

mod collection_detail;
mod film_detail;
mod home;
mod home_list;
mod library;
mod lists;
mod login;
mod person;
pub mod player;
pub mod search;
mod series_detail;
pub mod skip;

pub use home::HeroView;
pub use series_detail::up_next_card;

use std::sync::Arc;

use bobine::provider::{Film, Series};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, IntoElement, ObjectFit, RenderImage, Stateful,
    Window, div, img, px, rgba,
};

use crate::film_page;
use crate::icons::{IconName, icon, with_stars};
use crate::images;
use crate::navigation::Screen;
use crate::series_page;
use crate::shell::Shell;
use crate::theme;

pub fn render(
    shell: &mut Shell,
    screen: Screen,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    match screen {
        Screen::Login => login::render(shell, window, cx).into_any_element(),
        Screen::Home => home::render(shell, window, cx).into_any_element(),
        Screen::Library => library::render(shell, window, cx).into_any_element(),
        Screen::FilmDetail => film_detail::render(shell, window, cx).into_any_element(),
        Screen::Person => person::render(shell, window, cx).into_any_element(),
        Screen::Player => player::empty(cx).into_any_element(),
        Screen::Series => library::render_series(shell, window, cx).into_any_element(),
        Screen::SeriesDetail => series_detail::render(shell, window, cx).into_any_element(),
        Screen::HomeList => home_list::render(shell, window, cx).into_any_element(),
        Screen::Collections => lists::render_collections(shell, window, cx).into_any_element(),
        Screen::CollectionDetail => collection_detail::render(shell, window, cx).into_any_element(),
        Screen::MyList => lists::render_my_list(shell, window, cx).into_any_element(),
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

/// A row's heading: `Continue watching`, `Cast`, `Similar titles`.
fn row_heading(title: impl Into<gpui_kit::SharedString>) -> Div {
    div()
        .text_size(px(18.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title.into())
}

/// An image filling its box, once it is here; the box's own colour until then. The box does
/// not clip its children to its rounded corners: the image is rounded by `radius` itself.
fn picture(image: Option<Arc<RenderImage>>, radius: f32) -> Option<AnyElement> {
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
    let text: gpui_kit::SharedString = text.into();
    div()
        .px(px(7.))
        .py(px(3.))
        .rounded(px(6.))
        .bg(rgba(0x09090acc))
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(with_stars(&text, 11.))
}

/// What a poster shows top right.
enum Corner {
    Nothing,
    /// A check: the film is watched.
    Watched,
    /// How many episodes are left to watch.
    Count(String),
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
    film_card(shell, film, width, id, outlined, false, cx)
}

/// `poster`, its picture already asked: see `film_card_with`.
fn poster_with(
    film: &Film,
    image: Option<Arc<RenderImage>>,
    width: f32,
    id: impl Into<ElementId>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    film_card_with(film, image, width, id, false, false, cx)
}

/// A film with, over the foot of its poster, how far the viewer went when it is started.
fn started_poster(
    shell: &mut Shell,
    film: &Film,
    width: f32,
    id: impl Into<ElementId>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    film_card(shell, film, width, id, false, true, cx)
}

fn film_card(
    shell: &mut Shell,
    film: &Film,
    width: f32,
    id: impl Into<ElementId>,
    outlined: bool,
    with_progress: bool,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let image = shell.image(film.poster.as_ref(), images::POSTER, cx);
    film_card_with(film, image, width, id, outlined, with_progress, cx)
}

/// A film poster whose picture is already asked: drawing it needs no shell, so a caller that
/// holds the film borrowed from the shell does not copy it at every frame.
fn film_card_with(
    film: &Film,
    image: Option<Arc<RenderImage>>,
    width: f32,
    id: impl Into<ElementId>,
    outlined: bool,
    with_progress: bool,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let opened = film.id.clone();
    let mut caption: Vec<String> = film.year.iter().map(i32::to_string).collect();
    if outlined {
        caption.push("this film".into());
    }
    let corner = if film.watched {
        Corner::Watched
    } else {
        Corner::Nothing
    };
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
            poster_image(image, film_page::rating(film), corner, width, outlined).children(
                with_progress
                    .then(|| film_page::progress(film))
                    .flatten()
                    .map(|fraction| {
                        div()
                            .absolute()
                            .left(px(8.))
                            .bottom(px(8.))
                            .child(progress_bar(fraction, width - 20.))
                    }),
            ),
        )
        .child(poster_caption(film.title.clone(), caption.join(" · ")))
}

/// A series as its grid and rows show it: like a film, with the episodes left to watch top
/// right and its years below.
fn series_poster(
    shell: &mut Shell,
    series: &Series,
    width: f32,
    id: impl Into<ElementId>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let image = shell.image(series.poster.as_ref(), images::POSTER, cx);
    series_poster_with(series, image, width, id, cx)
}

/// A series poster whose picture is already asked: see `film_card_with`.
fn series_poster_with(
    series: &Series,
    image: Option<Arc<RenderImage>>,
    width: f32,
    id: impl Into<ElementId>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let opened = series.id.clone();
    let corner = series_page::badge(series).map_or(Corner::Nothing, Corner::Count);
    let rating = series
        .community_rating
        .map(|rating| format!("★ {rating:.1}"));
    div()
        .id(id)
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| {
            shell.open_series(opened.clone(), cx);
        }))
        .child(poster_image(image, rating, corner, width, false))
        .child(poster_caption(
            series.title.clone(),
            series_page::years(series).unwrap_or_default(),
        ))
}

fn poster_image(
    image: Option<Arc<RenderImage>>,
    rating: Option<String>,
    corner: Corner,
    width: f32,
    outlined: bool,
) -> Div {
    let round = |child: Div| {
        child
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .h(px(22.))
            .min_w(px(22.))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::accent())
            .text_color(theme::background())
    };
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
        .children(rating.map(|rating| badge(rating).absolute().top(px(8.)).left(px(8.))))
        .children(match corner {
            Corner::Nothing => None,
            Corner::Watched => Some(round(div()).child(icon(IconName::Check, 13.))),
            Corner::Count(count) => Some(
                round(div())
                    .px(px(7.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .child(count),
            ),
        })
}

fn poster_caption(title: String, caption: String) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .truncate()
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme::dim())
                .child(caption),
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
