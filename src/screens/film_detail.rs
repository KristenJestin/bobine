//! A film's page, after Infuse: the backdrop fills the top; on the left the title, the facts, the
//! synopsis, the play buttons and the chips that choose the version and the tracks. Below, rows
//! aligned with the text: the cast, the collection, similar titles. A row reaches the window's
//! edges: its first item lines up with the text, and it scrolls under the edges. The page
//! scrolls.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use bobine::provider::{CastMember, Film, FilmDetail, ImageRef, ItemId, MediaSource, PersonHit};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent,
    ObjectFit, Pixels, Point, ScrollHandle, SharedString, Stateful, Window, anchored, canvas,
    deferred, div, img, linear_color_stop, linear_gradient, point, px, rgb, rgba,
};

use super::{centered, note, picture, poster, progress_bar, row_heading};
use crate::film_page::{self, Choices, Fact, From};
use crate::hero;
use crate::icons::{IconName, icon, with_stars};
use crate::images;
use crate::menu;
use crate::motion::{self, Fade};
use crate::navigation::NavEntry;
use crate::shell::{Chip, Loadable, Shell};
use crate::theme;
use crate::trailer;

/// The menu open, and the one drawn with how much of it shows (it may be fading out).
#[derive(Clone, Copy)]
pub(super) struct MenuState<K = Chip> {
    pub open: Option<K>,
    pub drawn: Option<(K, f32)>,
}

/// What the shell does when a chip is clicked, one of its choices is taken, or the mouse goes
/// down outside its open menu: the film pages and the library grids each have their own.
pub(super) struct Actions<K> {
    pub toggle: fn(&mut Shell, K, &mut Context<Shell>),
    pub choose: fn(&mut Shell, K, Option<i32>, &mut Context<Shell>),
    pub outside: fn(&mut Shell, Point<Pixels>, &mut Context<Shell>),
}

impl<K> Clone for Actions<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Actions<K> {}

/// The chips of the film and series pages.
pub(super) const CHIP_ACTIONS: Actions<Chip> = Actions {
    toggle: Shell::toggle_menu,
    choose: Shell::choose,
    outside: Shell::pressed_outside_menu,
};

/// Where a chip was last drawn, shared with the shell.
pub(super) type ChipBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The left of the text and the rows, after the design.
pub(super) const EDGE: f32 = 40.;
pub(super) const TEXT_WIDTH: f32 = 620.;
pub(super) const ROW_POSTER: f32 = 150.;
const PHOTO: f32 = 96.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.details.last() else {
        return centered().into_any_element();
    };
    let (detail, choices) = match (&page.detail, &page.choices) {
        (Loadable::Ready(detail), Some(choices)) => (detail.clone(), choices.clone()),
        (Loadable::Failed(why), _) => return centered().child(note(*why)).into_any_element(),
        _ => return centered().child(note("Loading…")).into_any_element(),
    };
    let now = Instant::now();
    let page = shell.details.last_mut().expect("the page read above");
    let open = page.menus.open();
    let menu = MenuState {
        open,
        drawn: page.menu_fade.follow(open, now),
    };
    if page.menu_fade.moving(now) {
        window.request_animation_frame();
    }
    let (error, busy, favourite) = (page.error, page.busy, page.favourite.on());
    let chips = page.chips.clone();
    let collection = page.collection.clone();
    let scroll = page.scroll.clone();
    let [cast_scroll, collection_scroll, similar_scroll] = page.rows.clone();
    let height = f32::from(window.viewport_size().height);

    let hero = hero(
        shell, &detail, &choices, menu, &chips, error, busy, favourite, height, window, cx,
    );
    let mut rows: Vec<AnyElement> = Vec::new();
    if !detail.cast.is_empty() {
        rows.push(cast_row(shell, &detail.cast, &cast_scroll, window, cx).into_any_element());
    }
    if let Some(collection) = collection {
        let mut films = collection.others.clone();
        films.push(detail.film.clone());
        films.sort_by_key(|film| film.year);
        let caption = crate::lists::films(films.len() as u32);
        rows.push(
            poster_row(
                shell,
                &collection.name,
                Some(caption),
                Some((collection.id.clone(), "collection-link")),
                &films,
                Some(&detail.film),
                &collection_scroll,
                window,
                cx,
            )
            .into_any_element(),
        );
    }
    if !detail.similar.is_empty() {
        rows.push(
            poster_row(
                shell,
                "Similar titles",
                None,
                None,
                &detail.similar,
                None,
                &similar_scroll,
                window,
                cx,
            )
            .into_any_element(),
        );
    }

    // The page scrolls itself and its rows by hand (`Shell::scroll_film_page`): GPUI's own
    // scrolling moves every container under the pointer on both axes at once.
    div()
        .id("film-page")
        .size_full()
        .overflow_hidden()
        .track_scroll(&scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| shell.scroll_film_page(event, window, cx)),
        )
        .child(hero)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(40.))
                .pt(px(12.))
                .pb(px(64.))
                .children(rows),
        )
        .into_any_element()
}

#[allow(
    clippy::too_many_arguments,
    reason = "one page's state, read once per frame"
)]
fn hero(
    shell: &mut Shell,
    detail: &FilmDetail,
    choices: &Choices,
    menu: MenuState,
    chips: &[ChipBounds; 3],
    error: Option<&'static str>,
    busy: bool,
    favourite: bool,
    window_height: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let film = &detail.film;
    let backdrop = shell.image(film.backdrop.as_ref(), images::BACKDROP, cx);
    let height = (window_height * 0.86).clamp(560., 900.);

    let play = theme::primary_button(
        "film-play",
        Some(IconName::Play),
        film_page::play_label(film),
        window,
        cx,
    )
    .on_click(cx.listener(|shell, _, window, cx| shell.play_film(From::Resume, window, cx)));
    let from_start = film.resume.is_some().then(|| {
        theme::ghost_button(
            "film-from-start",
            Some(IconName::RotateCcw),
            "From the start",
            window,
            cx,
        )
        .on_click(cx.listener(|shell, _, window, cx| shell.play_film(From::Beginning, window, cx)))
    });
    let trailer = trailer::choose(&detail.trailers, shell.ytdlp)
        .map(|_| trailer_button(FILM_TRAILER, window, cx));
    let watched = theme::ghost_button("film-watched", Some(IconName::Check), "Watched", window, cx)
        .when(film.watched, |button| {
            button
                .text_color(theme::accent())
                .border_color(theme::accent())
        })
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_watched(cx)));
    let in_list = list_button("film-list", favourite, window, cx);
    let buttons = div()
        .flex()
        .gap(px(10.))
        .when(busy, |row| row.opacity(0.6))
        .child(play)
        .children(from_start)
        .children(trailer)
        .child(watched)
        .child(in_list);

    let chips = chips_row(&detail.versions, choices, menu, chips, window, cx);

    let meta_line = facts_line(
        film_page::meta(detail),
        NavEntry::Films,
        shell.font(),
        window,
        cx,
    );

    let title = heading(shell, &film.title, detail.logo.as_ref(), window, cx);
    hero_frame(backdrop, height).child(
        hero_text_under(title)
            .child(meta_line)
            .children((!detail.directors.is_empty()).then(|| {
                div()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(format!("Directed by {}", detail.directors.join(", ")))
            }))
            .children(detail.overview.clone().map(|overview| {
                div()
                    .text_size(px(15.))
                    .line_height(px(23.))
                    .text_color(rgba(0xf4f4f5d9))
                    .child(overview)
            }))
            .child(div().pt(px(6.)).child(buttons))
            .children(film_page::progress(film).map(|fraction| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(progress_bar(fraction, 300.))
                    .children(film_page::time_left(film).map(|left| {
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(left)
                    }))
            }))
            .children(error.map(|why| {
                div()
                    .text_size(px(13.))
                    .text_color(theme::danger())
                    .child(why)
            }))
            .child(chips),
    )
}

/// "My list": a plus while the item is out of the list, a check once it is in, the green of
/// the other states of the page; the change eases from one to the other.
pub(super) fn list_button(
    id: &'static str,
    in_list: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let fade = window.use_keyed_state(SharedString::from(format!("{id}-fade")), cx, |_, _| {
        Fade::starting_at(motion::quick(), in_list)
    });
    let now = Instant::now();
    fade.update(cx, |fade, _| fade.set(in_list, now));
    let fade = *fade.read(cx);
    let level = fade.level(now);
    if fade.moving(now) {
        window.request_animation_frame();
    }
    let size = theme::BUTTON_ICON;
    let icons = div()
        .relative()
        .flex_none()
        .size(px(size))
        .child(
            icon(IconName::Plus, size)
                .absolute()
                .inset_0()
                .opacity(1. - level),
        )
        .child(
            icon(IconName::CheckCircle, size)
                .absolute()
                .inset_0()
                .opacity(level),
        );
    theme::ghost_button_with(id, Some(icons.into_any_element()), "My list", window, cx)
        .text_color(motion::mix(theme::text(), theme::accent(), level))
        .border_color(motion::mix(rgb(0x3a3a40), theme::accent(), level))
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_favourite(cx)))
}

/// The Trailer buttons, by page: their ids, which are also the selectors every button carries.
pub const FILM_TRAILER: &str = "film-trailer";
pub const SERIES_TRAILER: &str = "series-trailer";

/// "Trailer": it fades in the first time the page draws it, like what the page shows by
/// itself. Like its neighbours, it has no dimming of its own: its row decides.
pub(super) fn trailer_button(
    id: &'static str,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let fade = window.use_keyed_state(SharedString::from(format!("{id}-fade")), cx, |_, _| {
        Fade::new(motion::quick())
    });
    let now = Instant::now();
    fade.update(cx, |fade, _| fade.set(true, now));
    let fade = *fade.read(cx);
    if fade.moving(now) {
        window.request_animation_frame();
    }
    theme::ghost_button(id, Some(IconName::PlayCircle), "Trailer", window, cx)
        .opacity(fade.level(now))
        .on_click(cx.listener(|shell, _, window, cx| shell.play_trailer(window, cx)))
}

/// The backdrop over the whole width, darkened towards the text and the page below.
pub(super) fn hero_frame(
    backdrop: Option<std::sync::Arc<gpui_kit::RenderImage>>,
    height: f32,
) -> Div {
    let bg = theme::background();
    let clear = rgba(0x09090a00);
    div()
        .relative()
        .w_full()
        .h(px(height))
        .children(picture(backdrop, 0.))
        .child(div().absolute().inset_0().bg(linear_gradient(
            90.,
            linear_color_stop(bg, 0.),
            linear_color_stop(clear, 0.75),
        )))
        .child(div().absolute().inset_0().bg(linear_gradient(
            0.,
            linear_color_stop(bg, 0.),
            linear_color_stop(clear, 0.45),
        )))
}

/// The column of text at the bottom left of the hero, under its title.
pub(super) fn hero_text(title: String) -> Div {
    hero_text_under(title_text(title))
}

/// The column of text at the bottom left of the hero, under `heading`: a title or its logo.
pub(super) fn hero_text_under(heading: impl IntoElement) -> Div {
    div()
        .absolute()
        .left(px(EDGE))
        .bottom(px(28.))
        .w(px(TEXT_WIDTH))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(heading)
}

fn title_text(title: String) -> Div {
    div()
        .debug_selector(|| "detail-title".to_string())
        .text_size(px(44.))
        .line_height(px(50.))
        .font_weight(FontWeight::BOLD)
        .child(title)
}

/// The title of a film or series page: its logo, at the size it fits in the room, once the
/// picture is here, fading in; its name in text until then, and when the server has no logo, so
/// that the page is never without a title.
pub(super) fn heading(
    shell: &mut Shell,
    title: &str,
    logo: Option<&ImageRef>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let drawn = logo
        .zip(shell.image(logo, images::LOGO, cx))
        .and_then(|(logo, image)| {
            let size = image.size(0);
            let fit =
                hero::logo_fit_in((size.width.0 as f32, size.height.0 as f32), hero::PAGE_LOGO)?;
            Some((logo, image, fit))
        });
    let Some((logo, image, (width, height))) = drawn else {
        return title_text(title.to_string()).into_any_element();
    };
    let fade = window.use_keyed_state(
        SharedString::from(format!("page-logo-{}", logo.item.0)),
        cx,
        |_, _| Fade::new(motion::page()),
    );
    // The executor's clock, which the tests drive.
    let now = cx.background_executor().now();
    fade.update(cx, |fade, _| fade.set(true, now));
    let fade = *fade.read(cx);
    if fade.moving(now) {
        window.request_animation_frame();
    }
    logo_over_title(
        img(image)
            .debug_selector(|| "detail-logo".to_string())
            .w(px(width))
            .h(px(height))
            .object_fit(ObjectFit::Contain),
        title_text(title.to_string()),
        TEXT_WIDTH,
        fade.level(now),
    )
    .debug_selector(|| "detail-heading".to_string())
    .into_any_element()
}

/// A logo come in place of its title: the logo's box holds the place, the logo fades in
/// (`level`) while the title, `width` wide and laid at its foot, fades out. The title shows alone
/// until the logo is here, so the heading is never empty.
pub(super) fn logo_over_title(logo: impl IntoElement, title: Div, width: f32, level: f32) -> Div {
    div()
        .relative()
        .child(div().opacity(level).child(logo))
        .when(level < 1., |heading| {
            heading.child(
                div()
                    .absolute()
                    .left_0()
                    .bottom_0()
                    .w(px(width))
                    .opacity(1. - level)
                    .child(title),
            )
        })
}

/// `2025 | ★ 6.0 | 2 h 11`: the facts under a title, apart.
pub(super) fn meta_line(parts: Vec<String>, font: Option<SharedString>, window: &Window) -> Div {
    meta_row(
        parts
            .iter()
            .map(|part| with_stars(part, META_SIZE).into_any_element()),
        font,
        window,
    )
}

/// The facts under a film's or a series' title, apart; each genre is a link to the library of
/// that kind (`library`) filtered on it.
pub(super) fn facts_line(
    facts: Vec<Fact>,
    library: NavEntry,
    font: Option<SharedString>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let parts: Vec<_> = facts
        .into_iter()
        .map(|fact| match fact {
            Fact::Text(text) => with_stars(&text, META_SIZE).into_any_element(),
            Fact::Genres(genres) => genre_links(genres, library, window, cx).into_any_element(),
        })
        .collect();
    meta_row(parts, font, window)
}

/// The genres, a comma after each but the last, each answering the pointer.
fn genre_links(
    genres: Vec<String>,
    library: NavEntry,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let last = genres.len() - 1;
    div()
        .flex()
        .items_center()
        .children(genres.into_iter().enumerate().map(|(index, genre)| {
            let selector = genre.clone();
            let separator = (index < last).then(|| {
                // Beside the link, so that neither its highlight nor its click take the comma.
                div()
                    .debug_selector({
                        let genre = genre.clone();
                        move || format!("genre-separator {genre}")
                    })
                    .mr(px(3.))
                    .child(",")
            });
            div()
                .flex()
                .items_center()
                .child(
                    theme::eased(("genre", index), theme::Shades::veil(), window, cx)
                        .debug_selector(move || format!("genre {selector}"))
                        .pl(px(4.))
                        .pr(px(1.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .on_click(cx.listener({
                            let genre = genre.clone();
                            move |shell, _, _, cx| shell.open_genre(library, genre.clone(), cx)
                        }))
                        .child(genre),
                )
                .children(separator)
        }))
}

/// The size and the height of the meta line's text.
const META_SIZE: f32 = 14.;
const META_LINE: f32 = 22.;

/// The bar between two facts covers the digits' box, from the cap height to the baseline: taller,
/// it reads as sitting above the text. Read from the font the line is drawn with (`font`, the
/// shell's, else the window's), as GPUI places a line: half the leading above the ascent. The
/// digits are drawn on whole pixels (Inter's 10.2 px take 11 rows), so the bar's height is too.
fn meta_bar(font: Option<SharedString>, window: &Window) -> (Pixels, Pixels) {
    let mut style = window.text_style();
    if let Some(family) = font {
        style.font_family = family;
    }
    let text = window.text_system();
    let id = text.resolve_font(&style.font());
    let size = px(META_SIZE);
    let ascent = text.ascent(id, size);
    let descent = text.descent(id, size).abs();
    let cap = text.cap_height(id, size).ceil();
    let baseline = (px(META_LINE) - ascent - descent) / 2. + ascent;
    (baseline - cap, cap)
}

fn meta_row(
    parts: impl IntoIterator<Item = AnyElement>,
    font: Option<SharedString>,
    window: &Window,
) -> Div {
    let (top, height) = meta_bar(font, window);
    let mut meta = parts.into_iter().enumerate();
    let part = |index: usize, part: AnyElement| {
        div()
            .debug_selector(move || format!("meta-part {index}"))
            .flex()
            .items_center()
            .child(part)
    };
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .text_size(px(META_SIZE))
        .line_height(px(META_LINE))
        .text_color(theme::muted())
        .children(meta.next().map(|(index, first)| part(index, first)))
        .children(meta.map(|(index, next)| {
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div().h(px(META_LINE)).child(
                        div()
                            .debug_selector(move || format!("meta-separator {index}"))
                            .w(px(1.))
                            .mt(top)
                            .h(height)
                            .bg(theme::line()),
                    ),
                )
                .child(part(index, next))
        }))
}

/// The version, audio and subtitle chips of what the play button starts.
pub(super) fn chips_row(
    versions: &[MediaSource],
    choices: &Choices,
    menu: MenuState,
    chips: &[ChipBounds; 3],
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let source = choices.source(versions);
    div()
        .flex()
        .gap(px(8.))
        .children(source.filter(|_| versions.len() > 1).map(|_| {
            let options = versions
                .iter()
                .enumerate()
                .map(|(index, version)| {
                    (
                        Some(index as i32),
                        film_page::version_label(version),
                        index == choices.version,
                    )
                })
                .collect();
            chip(
                Chip::Version,
                Chip::Version as usize,
                "VERSION",
                source.map(film_page::version_label),
                options,
                menu,
                &chips[Chip::Version as usize],
                CHIP_ACTIONS,
                window,
                cx,
            )
        }))
        .children(source.filter(|s| !s.audio.is_empty()).map(|source| {
            let current = source.audio.iter().find(|t| Some(t.index) == choices.audio);
            let options = source
                .audio
                .iter()
                .map(|t| {
                    (
                        Some(t.index),
                        film_page::audio_label(t),
                        Some(t.index) == choices.audio,
                    )
                })
                .collect();
            chip(
                Chip::Audio,
                Chip::Audio as usize,
                "AUDIO",
                current.map(film_page::audio_label),
                options,
                menu,
                &chips[Chip::Audio as usize],
                CHIP_ACTIONS,
                window,
                cx,
            )
        }))
        .children(source.map(|source| {
            let current = source
                .subtitles
                .iter()
                .find(|t| Some(t.index) == choices.subtitle);
            let options = std::iter::once((
                None,
                film_page::subtitle_label(None),
                choices.subtitle.is_none(),
            ))
            .chain(source.subtitles.iter().map(|t| {
                (
                    Some(t.index),
                    film_page::subtitle_label(Some(t)),
                    Some(t.index) == choices.subtitle,
                )
            }))
            .collect();
            chip(
                Chip::Subtitle,
                Chip::Subtitle as usize,
                "SUBTITLES",
                Some(film_page::subtitle_label(current)),
                options,
                menu,
                &chips[Chip::Subtitle as usize],
                CHIP_ACTIONS,
                window,
                cx,
            )
        }))
}

/// `AUDIO  French · E-AC-3 5.1 ⌄`; a click opens the choices under it.
#[allow(
    clippy::too_many_arguments,
    reason = "one chip's state, read once per frame"
)]
pub(super) fn chip<K: Copy + PartialEq + 'static>(
    kind: K,
    id: usize,
    label: &'static str,
    value: Option<String>,
    options: Vec<(Option<i32>, String, bool)>,
    menu: MenuState<K>,
    bounds: &ChipBounds,
    actions: Actions<K>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let drawn = bounds.clone();
    let open = menu.open == Some(kind);
    let shades = theme::Shades {
        rest: rgba(0x141416cc),
        hover: theme::line(),
        press: rgb(0x2e2e33),
    };
    let button = theme::eased(("chip", id), shades, window, cx)
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(7.))
        .rounded_full()
        .border_1()
        .border_color(if open { theme::muted() } else { theme::line() })
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| (actions.toggle)(shell, kind, cx)))
        .child(
            div()
                .text_size(px(10.))
                .font_weight(FontWeight::BOLD)
                .text_color(theme::dim())
                .child(label),
        )
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(value.unwrap_or_else(|| "—".into())),
        )
        .child(icon(IconName::ChevronDown, 14.).text_color(theme::muted()));
    let under = bounds
        .get()
        .zip(menu.drawn.filter(|(drawn, _)| *drawn == kind));
    div()
        .relative()
        .child(button)
        // Where the chip is drawn, for its menu.
        .child(
            canvas(move |at, _, _| drawn.set(Some(at)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .children(under.map(|(chip, (_, shown))| {
            self::menu(chip, shown, open, kind, options, actions, window, cx)
        }))
}

/// The choices of a chip, over the rest of the page, `shown` of the way in. Only an open menu
/// answers the mouse: one fading out lets clicks through.
#[allow(
    clippy::too_many_arguments,
    reason = "one menu's state, read once per frame"
)]
fn menu<K: Copy + 'static>(
    chip: Bounds<Pixels>,
    shown: f32,
    open: bool,
    kind: K,
    options: Vec<(Option<i32>, String, bool)>,
    actions: Actions<K>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let lines: Vec<Stateful<Div>> =
        options
            .into_iter()
            .enumerate()
            .map(|(index, (value, label, current))| {
                theme::eased(("choice", index), theme::Shades::line(), window, cx)
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_none()
                    .h(px(menu::LINE))
                    .px(px(12.))
                    .rounded(px(7.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .when(open, |line| {
                        line.on_click(cx.listener(move |shell, _, _, cx| {
                            (actions.choose)(shell, kind, value, cx)
                        }))
                    })
                    .child(
                        div()
                            .size(px(14.))
                            .text_color(theme::accent())
                            .when(current, |mark| mark.child(icon(IconName::Check, 14.))),
                    )
                    .child(label)
            })
            .collect();
    let wanted = menu::height(lines.len());
    let window_height = f32::from(window.viewport_size().height);
    let (at, height) = menu::place(chip, wanted, window_height);
    // It comes into place from a little nearer its chip.
    let toward_chip = if at.y > chip.top() { -8. } else { 8. };
    let rise = point(px(0.), px(toward_chip * (1. - shown)));
    deferred(
        anchored()
            .position(at + rise)
            .snap_to_window_with_margin(px(menu::MARGIN))
            .child(
                div()
                    .id("chip-menu")
                    .opacity(shown)
                    .h(px(height))
                    .overflow_y_scroll()
                    .min_w(px(240.))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .rounded(px(theme::RADIUS_LARGE))
                    .border_1()
                    .border_color(theme::line())
                    .bg(theme::panel())
                    .when(open, move |menu| {
                        menu.occlude().on_mouse_down_out(cx.listener(
                            move |shell, event: &MouseDownEvent, _, cx| {
                                // A side button is Back: it closes the menu itself, once.
                                if matches!(event.button, MouseButton::Navigate(_)) {
                                    return;
                                }
                                (actions.outside)(shell, event.position, cx);
                            },
                        ))
                    })
                    .children(lines),
            ),
    )
}

/// Round photos, the name and the role; a click on a person opens their page.
pub(super) fn cast_row(
    shell: &mut Shell,
    cast: &[CastMember],
    scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let people: Vec<Stateful<Div>> = cast
        .iter()
        .enumerate()
        .map(|(index, person)| {
            let photo = shell.image(person.photo.as_ref(), images::PHOTO, cx);
            let hit = PersonHit {
                id: person.id.clone(),
                name: person.name.clone(),
                photo: person.photo.clone(),
            };
            theme::eased(("person", index), theme::Shades::line(), window, cx)
                .debug_selector(|| format!("cast-member {index}"))
                .w(px(PHOTO + 24.))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .py(px(8.))
                .rounded(px(theme::RADIUS))
                .cursor_pointer()
                .on_click(cx.listener(move |shell, _, _, cx| shell.open_person(hit.clone(), cx)))
                .child(
                    div()
                        .relative()
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
                        // Under the photo while it comes, alone when there is none.
                        .child(film_page::initials(&person.name))
                        .children(picture(photo, PHOTO / 2.)),
                )
                .child(
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(person.name.clone()),
                )
                .children(person.role.clone().map(|role| {
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .truncate()
                        .child(role)
                }))
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(row_heading("Cast").px(px(EDGE)))
        .child(
            div()
                .id("cast-row")
                .flex()
                .gap(px(12.))
                .px(px(EDGE))
                .overflow_hidden()
                .track_scroll(scroll)
                .children(people),
        )
}

/// A heading and posters; `current` is outlined and not clickable. `link`: where the heading
/// leads, as a pill after its caption (the collection's page).
#[allow(
    clippy::too_many_arguments,
    reason = "one row's parts, put together once"
)]
fn poster_row(
    shell: &mut Shell,
    title: &str,
    caption: Option<String>,
    link: Option<(ItemId, &'static str)>,
    films: &[Film],
    current: Option<&Film>,
    scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let posters: Vec<Stateful<Div>> = films
        .iter()
        .enumerate()
        .map(|(index, film)| {
            let outlined = current.is_some_and(|current| current.id == film.id);
            let id = (SharedString::from(format!("{title}-film")), index);
            poster(shell, film, ROW_POSTER, id, outlined, cx)
        })
        .collect();
    let link = link.map(|(target, id)| {
        theme::eased(id, theme::Shades::veil(), window, cx)
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(3.))
            .rounded(px(6.))
            .text_size(px(13.))
            .text_color(theme::muted())
            .cursor_pointer()
            .child("View collection")
            .child(icon(IconName::ChevronRight, 13.))
            .on_click(cx.listener(move |shell, _, _, cx| {
                shell.open_collection(target.clone(), cx);
            }))
    });
    row_with(title, caption, link, posters, scroll)
}

/// A heading, and cards in a row that scrolls sideways under the window's edges.
pub(super) fn row(
    title: &str,
    caption: Option<String>,
    cards: Vec<Stateful<Div>>,
    scroll: &ScrollHandle,
) -> Div {
    row_with(title, caption, None, cards, scroll)
}

/// A row whose heading also holds a link, after its caption.
fn row_with(
    title: &str,
    caption: Option<String>,
    link: Option<Stateful<Div>>,
    cards: Vec<Stateful<Div>>,
    scroll: &ScrollHandle,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(12.))
                .px(px(EDGE))
                .child(row_heading(title.to_string()))
                .children(caption.map(|caption| {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::dim())
                        .child(caption)
                }))
                .children(link),
        )
        .child(
            div()
                .id(format!("{title}-row"))
                .flex()
                .gap(px(GAP))
                .px(px(EDGE))
                .overflow_hidden()
                .track_scroll(scroll)
                .children(cards),
        )
}

const GAP: f32 = 16.;
