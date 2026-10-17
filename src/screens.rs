//! The screens, and the pieces several of them show: posters, the rows under a heading, the
//! messages of a page that is loading or failed.

mod card;
mod collection_detail;
mod discover;
mod film_detail;
pub mod help;
mod home;
mod home_list;
mod library;
mod lists;
mod login;
mod person;
pub mod player;
pub mod playlist;
mod request;
pub mod search;
mod seerr_collection;
mod series_detail;
mod settings;
pub mod skip;
pub mod stats;

pub use card::{Card, Cards, Offer, Open, Play, Quick, open_card, play_card};
#[cfg(test)]
pub use film_detail::{FILM_TRAILER, SERIES_TRAILER};
pub use home::HeroView;
pub use home::pill as notice_pill;
pub use series_detail::up_next_card;

use std::rc::Rc;
use std::sync::Arc;

use bobine::provider::{Film, Series};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, Div, ElementId, FontWeight, IntoElement, ObjectFit, RenderImage, Stateful,
    Window, div, img, px, rgba,
};

use crate::film_page;
use crate::home_page::PlayTarget;
use crate::icons::{IconName, icon, with_stars};
use crate::images;
use crate::navigation::Screen;
use crate::poster_actions::Marks;
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
        Screen::MyList => lists::render_lists(shell, window, cx).into_any_element(),
        Screen::Favorites => lists::render_my_list(shell, window, cx).into_any_element(),
        Screen::Playlist => playlist::render(shell, window, cx).into_any_element(),
        Screen::Settings => settings::render(shell, window, cx).into_any_element(),
        Screen::Stats => stats::render(shell, window, cx).into_any_element(),
        Screen::Discover => discover::render(shell, window, cx).into_any_element(),
        Screen::Request => request::render(shell, window, cx).into_any_element(),
        Screen::SeerrCollection => seerr_collection::render(shell, window, cx).into_any_element(),
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
#[derive(Clone)]
enum Corner {
    Nothing,
    /// A check: the film is watched.
    Watched,
    /// How many episodes are left to watch.
    Count(String),
}

/// A poster as it is drawn, read out of its title once per draw of the page.
#[derive(Clone)]
struct Look {
    name: ElementId,
    image: Option<Arc<RenderImage>>,
    rating: Option<String>,
    corner: Corner,
    width: f32,
    /// The film of the page, in its collection.
    outlined: bool,
    /// How far the viewer went, over the foot of the poster.
    progress: Option<f32>,
    title: String,
    caption: String,
}

impl Look {
    /// The poster, with `layer` over its picture.
    fn draw(&self, layer: Option<Stateful<Div>>) -> Div {
        let width = self.width;
        div()
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                poster_image(
                    &self.name,
                    self.image.clone(),
                    self.rating.clone(),
                    self.corner.clone(),
                    width,
                    self.outlined,
                )
                .children(self.progress.map(|fraction| {
                    div()
                        .absolute()
                        .left(px(8.))
                        .bottom(px(8.))
                        .child(progress_bar(fraction, width - 20.))
                }))
                .children(layer),
            )
            .child(poster_caption(self.title.clone(), self.caption.clone()))
    }
}

/// A film as the grid and the rows show it: the poster, its rating top left, a check when
/// watched, the title and the year below. `with_progress`: how far the viewer went, over the
/// foot of the poster, when it is started.
fn film_look(
    film: &Film,
    image: Option<Arc<RenderImage>>,
    width: f32,
    name: &ElementId,
    outlined: bool,
    with_progress: bool,
    watched: bool,
) -> Look {
    let mut caption: Vec<String> = film.year.iter().map(i32::to_string).collect();
    if outlined {
        caption.push("this movie".into());
    }
    Look {
        name: name.clone(),
        image,
        rating: film_page::rating(film),
        corner: if watched {
            Corner::Watched
        } else {
            Corner::Nothing
        },
        width,
        outlined,
        // A film marked watched has no progress left to show.
        progress: (with_progress && !watched)
            .then(|| film_page::progress(film))
            .flatten(),
        title: film.title.clone(),
        caption: caption.join(" · "),
    }
}

/// A series as its grid and rows show it: like a film, with the episodes left to watch top
/// right (a check once all are watched) and its years below. `fresh` is for the Recently added
/// row, where a season handed for its series says which one is new.
fn series_look(
    series: &Series,
    image: Option<Arc<RenderImage>>,
    width: f32,
    name: &ElementId,
    watched: bool,
    fresh: bool,
) -> Look {
    Look {
        name: name.clone(),
        image,
        rating: series
            .community_rating
            .map(|rating| format!("★ {rating:.1}")),
        corner: if watched {
            Corner::Watched
        } else {
            series_page::badge(series).map_or(Corner::Nothing, Corner::Count)
        },
        width,
        outlined: false,
        progress: None,
        title: series.title.clone(),
        caption: series_page::card_detail(series, fresh).unwrap_or_default(),
    }
}

/// The card of a film poster: a click opens the film, its layer plays it, puts it in the list
/// and marks it watched.
fn film_offer(
    marks: &Marks,
    film: &Film,
    image: Option<Arc<RenderImage>>,
    width: f32,
    name: &ElementId,
    with_progress: bool,
) -> Offer {
    let quick = Quick::new(marks, &film.id, false, film.favourite, film.watched);
    let look = film_look(
        film,
        image,
        width,
        name,
        false,
        with_progress,
        quick.watched.now,
    );
    Offer {
        open: Open::Film(film.id.clone()),
        play: Play::Target(PlayTarget::Film(film.id.clone())),
        quick: Some(quick),
        draw: Rc::new(move |layer| look.draw(Some(layer))),
    }
}

/// The card of a series poster: a click opens the series (on its new season, for a card of
/// one), its layer plays its next episode, puts it in the list and marks all of it watched. A
/// card of a new season, or of a lone episode, does not know whether its series is in the list
/// or watched: it has no button for them.
fn series_offer(
    marks: &Marks,
    series: &Series,
    image: Option<Arc<RenderImage>>,
    width: f32,
    name: &ElementId,
    fresh: bool,
) -> Offer {
    let quick = series
        .favourite
        .zip(series.watched)
        .map(|(in_list, watched)| Quick::new(marks, &series.id, true, in_list, watched));
    let watched = quick.as_ref().is_some_and(|quick| quick.watched.now);
    let look = series_look(series, image, width, name, watched, fresh);
    Offer {
        open: Open::Series(
            series.id.clone(),
            series.season.as_ref().map(|season| season.id.clone()),
        ),
        play: Play::Target(PlayTarget::Series(series.id.clone())),
        quick,
        draw: Rc::new(move |layer| look.draw(Some(layer))),
    }
}

/// The card `id`, a view of its own, in its place on the page.
fn card(shell: &mut Shell, id: ElementId, offer: Offer, cx: &mut Context<Shell>) -> Stateful<Div> {
    let view = shell.card(&id, offer, cx);
    div().id(id).flex_none().child(view)
}

/// A film poster; `outlined`: the film of the page, in its collection, which opens nothing.
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
    let id = id.into();
    if outlined {
        // It opens nothing and plays nothing.
        let look = film_look(film, image, width, &id, true, with_progress, film.watched);
        return div().id(id).flex_none().child(look.draw(None));
    }
    let offer = film_offer(&shell.marks, film, image, width, &id, with_progress);
    card(shell, id, offer, cx)
}

fn series_poster(
    shell: &mut Shell,
    series: &Series,
    width: f32,
    id: impl Into<ElementId>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    series_card(shell, series, width, false, id, cx)
}

/// A series poster; `fresh`: see `series_look`.
fn series_card(
    shell: &mut Shell,
    series: &Series,
    width: f32,
    fresh: bool,
    id: impl Into<ElementId>,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let image = shell.image(series.poster.as_ref(), images::POSTER, cx);
    let id = id.into();
    let offer = series_offer(&shell.marks, series, image, width, &id, fresh);
    card(shell, id, offer, cx)
}

/// The picture of the poster `id`, with its marks.
fn poster_image(
    id: &ElementId,
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
            Corner::Watched => Some(
                round(div())
                    .debug_selector(|| format!("watched-mark {id}"))
                    .child(icon(IconName::Check, 13.)),
            ),
            Corner::Count(count) => Some(
                round(div())
                    .px(px(7.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .child(count),
            ),
        })
}

/// The text sizes of a card's caption: its title, and the line under it.
const CAPTION_TITLE: f32 = 13.;
const CAPTION_DETAIL: f32 = 12.;
/// Between the two lines of a caption.
const CAPTION_GAP: f32 = 2.;

/// The height of a line of caption text of `size`, descenders included: the lines are drawn at
/// it and the rows keep room from it, so the room is the text's whatever its size.
const fn caption_line(size: f32) -> f32 {
    (size * 1.4).ceil()
}

/// The height of a caption whose title is `title` px: its two lines and the gap between.
const fn caption_height(title: f32) -> f32 {
    caption_line(title) + CAPTION_GAP + caption_line(CAPTION_DETAIL)
}

/// A line of a card's caption, cut with an ellipsis rather than wrapped: a caption is two lines.
fn caption_text(size: f32, text: impl IntoElement) -> Div {
    div()
        .text_size(px(size))
        .line_height(px(caption_line(size)))
        .truncate()
        .child(text)
}

fn poster_caption(title: String, caption: String) -> Div {
    div()
        .debug_selector(|| "card-caption".into())
        .flex()
        .flex_col()
        .gap(px(CAPTION_GAP))
        .child(caption_text(CAPTION_TITLE, title).font_weight(FontWeight::SEMIBOLD))
        .child(caption_text(CAPTION_DETAIL, caption).text_color(theme::dim()))
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
