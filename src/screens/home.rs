//! Home, after nabby.pro: a large header whose title changes by itself (cross-fading, with a bar
//! under it for each title), and under it the rows: what was started, the next episodes, and
//! what each library gained. Each row scrolls sideways and has its "Tout voir". The page scrolls
//! up and down, and the wheel goes to the row under the pointer when it is over one.

use std::time::Instant;

use bobine::provider::{Feature, ImageRef, Title};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, Context, Div, ElementId, Entity, FontWeight,
    IntoElement, ObjectFit, ScrollHandle, SharedString, Stateful, Window, div, img,
    linear_color_stop, linear_gradient, px, relative, rgb, rgba,
};

use super::film_detail::{EDGE, ROW_POSTER, hero_frame, meta_line};
use super::{note, picture, poster, progress_bar, row_heading, series_poster};
use crate::hero::{self, Headline};
use crate::home_page::{self, ListKind, PlayTarget};
use crate::icons::{IconName, icon};
use crate::images;
use crate::motion::{self, Fade};
use crate::shell::{Loadable, Shell};
use crate::theme;

/// A card of a row of what was started, 16:9.
const WIDE: f32 = 340.;
/// What is under a wide card's picture: its title and a line, with the room between.
const WIDE_CAPTION: f32 = 52.;
const POSTER_CAPTION: f32 = 44.;
const GAP: f32 = 16.;
/// The left of the header's text, after the design.
const HERO_INSET: f32 = 150.;
const HERO_TEXT_WIDTH: f32 = 560.;
/// The space between two rows, and under the heading of a row.
const BETWEEN_ROWS: f32 = 40.;
const HEADING: f32 = 28.;
const UNDER_HEADING: f32 = 16.;
/// The most a logo takes of the header.
const LOGO: (f32, f32) = (440., 130.);

/// What a row holds.
enum Body {
    Wide(Vec<Title>),
    Posters(Vec<Title>),
    Failed(&'static str),
}

struct RowSpec {
    list: ListKind,
    caption: Option<String>,
    body: Body,
}

impl RowSpec {
    /// How tall the row is, heading included: its pictures are asked only when it is near.
    fn height(&self) -> f32 {
        let body = match &self.body {
            Body::Wide(_) => WIDE * 9. / 16. + WIDE_CAPTION,
            Body::Posters(_) => ROW_POSTER * 1.5 + POSTER_CAPTION,
            Body::Failed(_) => 20.,
        };
        HEADING + UNDER_HEADING + body
    }
}

/// The rows to draw now, in order; the ones with nothing to show are left out.
fn rows(shell: &Shell) -> Vec<RowSpec> {
    let mut rows = Vec::new();
    let started: &[Title] = match &shell.home.resume {
        Some(Loadable::Ready(started)) => started,
        _ => &[],
    };
    match &shell.home.resume {
        Some(Loadable::Ready(titles)) if !titles.is_empty() => rows.push(RowSpec {
            list: ListKind::Resume,
            caption: None,
            body: Body::Wide(titles.clone()),
        }),
        Some(Loadable::Failed(why)) => rows.push(RowSpec {
            list: ListKind::Resume,
            caption: None,
            body: Body::Failed(why),
        }),
        _ => {}
    }
    if let Some(Loadable::Ready(next)) = &shell.home.next_up {
        let next = home_page::next_up_row(next.clone(), started);
        if !next.is_empty() {
            rows.push(RowSpec {
                list: ListKind::NextUp,
                caption: None,
                body: Body::Wide(next.into_iter().map(Title::Episode).collect()),
            });
        }
    }
    if let Some(Loadable::Ready(latest)) = &shell.home.latest {
        for row in latest {
            let list = ListKind::Latest(row.library.id.clone(), row.library.kind);
            let caption = Some(row.library.name.clone());
            match &row.titles {
                Loadable::Ready(titles) if !titles.is_empty() => rows.push(RowSpec {
                    list,
                    caption,
                    body: Body::Posters(titles.clone()),
                }),
                Loadable::Failed(why) => rows.push(RowSpec {
                    list,
                    caption,
                    body: Body::Failed(why),
                }),
                _ => {}
            }
        }
    }
    rows
}

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    // The header's time runs while this is drawn.
    shell.start_hero_ticker(window, cx);
    let viewport = window.viewport_size();
    let (width, window_height) = (f32::from(viewport.width), f32::from(viewport.height));
    let hero_height = (window_height * 0.72).clamp(460., 760.);
    let features = shell.home.featured().len();
    let hero_wanted = features > 0 || matches!(shell.home.features, Some(Loadable::Loading));
    // Under the pill bar when there is no header.
    let top = if hero_wanted { hero_height } else { 104. };

    let specs = rows(shell);
    let scrolls = shell.home.rows_for(specs.len());
    let scrolled = -f32::from(shell.home.scroll.offset().y);
    let mut y = top + 12.;
    let mut shown: Vec<AnyElement> = Vec::new();
    for (index, (spec, scroll)) in specs.iter().zip(&scrolls).enumerate() {
        let near = home_page::row_in_view(y, spec.height(), scrolled, window_height);
        shown.push(
            titles_row(shell, spec, index, scroll, near, width, window, cx).into_any_element(),
        );
        y += spec.height() + BETWEEN_ROWS;
    }

    let loading = shell.home.resume.is_none()
        || [
            matches!(shell.home.resume, Some(Loadable::Loading)),
            matches!(shell.home.latest, Some(Loadable::Loading)),
        ]
        .contains(&true);
    let empty = shown.is_empty() && !hero_wanted;
    let status = match (empty, loading) {
        (true, true) => Some("Chargement…"),
        (true, false) => Some("Rien à afficher pour l'instant."),
        _ => None,
    };

    let header = if features > 0 {
        Some(hero(shell, hero_height, window, cx).into_any_element())
    } else if hero_wanted {
        // The header is on its way: its room is kept, so that nothing moves when it comes.
        Some(div().w_full().h(px(hero_height)).into_any_element())
    } else {
        None
    };
    let error = shell.home.error;

    div()
        .id("home-page")
        .size_full()
        .relative()
        .overflow_hidden()
        .track_scroll(&shell.home.scroll)
        .on_scroll_wheel(
            cx.listener(|shell, event, window, cx| shell.scroll_home(event, window, cx)),
        )
        .children(header)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(BETWEEN_ROWS))
                .pt(px(if hero_wanted { 12. } else { 104. }))
                .pb(px(64.))
                .children(status.map(|status| div().px(px(EDGE)).child(note(status))))
                .children(shown),
        )
        .children(error.map(error_pill))
}

/// What went wrong with the last play button, over the page, fading in.
pub(super) fn error_pill(why: &'static str) -> impl IntoElement {
    div()
        .id(("home-error", why.len()))
        .absolute()
        .bottom(px(28.))
        .left(px(EDGE))
        .px(px(14.))
        .py(px(10.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(theme::line())
        .bg(theme::panel())
        .text_size(px(13.))
        .text_color(theme::danger())
        .child(why)
        .with_animation(
            ("home-error-in", why.len()),
            Animation::new(motion::quick()).with_easing(motion::ease_out),
            |pill, t| pill.opacity(t),
        )
}

// The header.

fn hero(
    shell: &mut Shell,
    height: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let now = Instant::now();
    let features: Vec<Feature> = shell.home.featured().to_vec();
    let current = shell.home.carousel.current().min(features.len() - 1);
    let blend = shell.home.carousel.blend(now);
    if shell.home.carousel.crossfading(now) {
        window.request_animation_frame();
    }
    let (text_out, text_in) = hero::text_levels(blend.level);
    // The next picture is asked while this one shows, so that the change does not wait for it.
    if features.len() > 1 {
        let next = &features[(current + 1) % features.len()];
        let _ = shell.image(next.backdrop.as_ref(), images::BACKDROP, cx);
    }

    let leaving = blend.leaving.and_then(|at| features.get(at));
    let backdrop = |shell: &mut Shell, feature: &Feature, level: f32, cx: &mut Context<Shell>| {
        let image = shell.image(feature.backdrop.as_ref(), images::BACKDROP, cx);
        div()
            .absolute()
            .inset_0()
            .opacity(level)
            .child(hero_frame(image, height))
    };
    let mut layers: Vec<AnyElement> = Vec::new();
    if let Some(leaving) = leaving {
        layers.push(backdrop(shell, leaving, 1., cx).into_any_element());
    }
    layers.push(backdrop(shell, &features[current], blend.level, cx).into_any_element());
    if let Some(leaving) = leaving {
        layers.push(slide_text(shell, leaving, text_out, false, window, cx).into_any_element());
    }
    layers
        .push(slide_text(shell, &features[current], text_in, true, window, cx).into_any_element());

    div()
        .id("hero")
        .relative()
        .w_full()
        .h(px(height))
        .overflow_hidden()
        .on_hover(
            cx.listener(|shell, hovered: &bool, window, cx| shell.hover_hero(*hovered, window, cx)),
        )
        .children(layers)
        .children((features.len() > 1).then(|| marks(shell, features.len(), now, window, cx)))
}

/// The text of one title, `level` of the way in. Only the title on show answers a click.
fn slide_text(
    shell: &mut Shell,
    feature: &Feature,
    level: f32,
    live: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let headline = match hero::headline(feature) {
        Headline::Logo(logo) => logo_or_title(shell, &logo, &feature.title, cx),
        Headline::Text(title) => title_text(title).into_any_element(),
    };
    let target = match feature.kind {
        bobine::provider::FeatureKind::Film => PlayTarget::Film(feature.id.clone()),
        bobine::provider::FeatureKind::Series => PlayTarget::Series(feature.id.clone()),
    };
    let id =
        |name: &'static str| SharedString::from(format!("hero-{name}-{}-{live}", feature.id.0));
    let play =
        theme::primary_button(
            id("play"),
            Some(IconName::Play),
            hero::play_label(feature),
            window,
            cx,
        )
        .when(live, |button| {
            button.on_click(cx.listener(move |shell, _, window, cx| {
                shell.play_from_home(target.clone(), window, cx)
            }))
        });
    let more = {
        let opened = feature.clone();
        theme::ghost_button(id("more"), Some(IconName::Plus), "Plus d'infos", window, cx).when(
            live,
            |button| {
                button.on_click(cx.listener(move |shell, _, _, cx| shell.open_feature(&opened, cx)))
            },
        )
    };
    let busy = shell.home.busy;
    div()
        .absolute()
        .left(px(HERO_INSET))
        .bottom(px(72.))
        .w(px(HERO_TEXT_WIDTH))
        .opacity(level)
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(kind_chip(hero::kind_label(feature)))
        .child(div().py(px(6.)).child(headline))
        .child(meta_line(hero::meta(feature)))
        .children(feature.overview.clone().map(|overview| {
            div()
                .max_w(px(500.))
                .text_size(px(15.))
                .line_height(px(23.))
                .text_color(rgba(0xf4f4f5d9))
                .line_clamp(3)
                .child(overview)
        }))
        .child(
            div()
                .pt(px(8.))
                .flex()
                .gap(px(10.))
                .when(busy, |row| row.opacity(0.6))
                .child(play)
                .child(more),
        )
}

fn title_text(title: String) -> Div {
    div()
        .text_size(px(60.))
        .line_height(px(64.))
        .font_weight(FontWeight::BOLD)
        .child(title)
}

/// The logo, at the size it fits in the room given, once it is here; the title in text until
/// then, so that the header is never empty while the logo comes.
fn logo_or_title(
    shell: &mut Shell,
    logo: &ImageRef,
    title: &str,
    cx: &mut Context<Shell>,
) -> AnyElement {
    match shell.image(Some(logo), images::LOGO, cx) {
        Some(image) => {
            let size = image.size(0);
            let (w, h) = (size.width.0 as f32, size.height.0 as f32);
            let scale = (LOGO.0 / w).min(LOGO.1 / h);
            img(image)
                .w(px(w * scale))
                .h(px(h * scale))
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        }
        None => title_text(title.to_string()).into_any_element(),
    }
}

/// `● FILM` over the title.
fn kind_chip(label: &'static str) -> Div {
    div().flex().child(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .py(px(6.))
            .rounded_full()
            .border_1()
            .border_color(theme::line())
            .bg(rgba(0x141416cc))
            .text_size(px(11.))
            .font_weight(FontWeight::BOLD)
            .child(div().size(px(7.)).rounded_full().bg(theme::accent()))
            .child(label),
    )
}

/// A bar for each title under the header: the one shown fills as its time runs, a click on one
/// shows its title.
fn marks(
    shell: &Shell,
    count: usize,
    now: Instant,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let current = shell.home.carousel.current();
    let progress = shell.home.carousel.progress(now);
    let shades = theme::Shades {
        rest: rgba(0xffffff33),
        hover: rgba(0xffffff66),
        press: rgba(0xffffff80),
    };
    div()
        .absolute()
        .left(px(EDGE))
        .right(px(EDGE))
        .bottom(px(18.))
        .flex()
        .gap(px(8.))
        .children((0..count).map(|index| {
            let track = theme::eased(("hero-track", index), shades, window, cx)
                .w_full()
                .h(px(4.))
                .rounded_full()
                .overflow_hidden()
                .children((index == current).then(|| {
                    div()
                        .h_full()
                        .w(relative(progress))
                        .rounded_full()
                        .bg(theme::text())
                }));
            // The bar is thin, its click is not.
            div()
                .id(("hero-mark", index))
                .flex_1()
                .h(px(22.))
                .flex()
                .items_end()
                .pb(px(0.))
                .cursor_pointer()
                .on_click(cx.listener(move |shell, _, _, cx| shell.jump_hero(index, cx)))
                .child(track)
        }))
}

// The rows.

#[allow(
    clippy::too_many_arguments,
    reason = "one row's state, read once per frame"
)]
fn titles_row(
    shell: &mut Shell,
    spec: &RowSpec,
    index: usize,
    scroll: &ScrollHandle,
    near: bool,
    width: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let list = spec.list.clone();
    let library = spec.caption.clone();
    let see_all = theme::eased(("see-all", index), theme::Shades::veil(), window, cx)
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(px(theme::RADIUS))
        .text_size(px(13.))
        .text_color(theme::muted())
        .cursor_pointer()
        .on_click(
            cx.listener(move |shell, _, _, cx| shell.open_list(list.clone(), library.clone(), cx)),
        )
        .child("Tout voir")
        .child(icon(IconName::ArrowRight, 14.));
    let heading = div()
        .h(px(HEADING))
        .flex()
        .items_center()
        .justify_between()
        .px(px(EDGE - 10.))
        .child(
            div()
                .pl(px(10.))
                .flex()
                .items_baseline()
                .gap(px(12.))
                .child(row_heading(spec.list.heading()))
                .children(spec.caption.clone().map(|caption| {
                    div()
                        .text_size(px(13.))
                        .text_color(theme::dim())
                        .child(caption)
                })),
        )
        .children((!matches!(spec.body, Body::Failed(_))).then_some(see_all));

    let body = match &spec.body {
        Body::Failed(why) => div().px(px(EDGE)).child(note(*why)).into_any_element(),
        Body::Wide(titles) => {
            let range = near_cards(scroll, width, WIDE, titles.len(), near);
            let cards: Vec<_> = titles
                .iter()
                .enumerate()
                .map(|(at, title)| {
                    let id = (SharedString::from(format!("home-wide-{index}")), at);
                    wide_card(shell, title, WIDE, id, range.contains(&at), window, cx)
                })
                .collect();
            cards_row(index, scroll, WIDE * 9. / 16. + WIDE_CAPTION, cards).into_any_element()
        }
        Body::Posters(titles) => {
            let range = near_cards(scroll, width, ROW_POSTER, titles.len(), near);
            let cards: Vec<AnyElement> = titles
                .iter()
                .enumerate()
                .map(|(at, title)| {
                    let id = (SharedString::from(format!("home-poster-{index}")), at);
                    poster_card(shell, title, id, range.contains(&at), cx)
                })
                .collect();
            cards_row(index, scroll, ROW_POSTER * 1.5 + POSTER_CAPTION, cards).into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(UNDER_HEADING))
        .child(heading)
        .child(body)
}

/// The cards of a row near enough the window for their pictures to be asked.
fn near_cards(
    scroll: &ScrollHandle,
    width: f32,
    card: f32,
    count: usize,
    near: bool,
) -> std::ops::Range<usize> {
    if !near {
        return 0..0;
    }
    home_page::cards_in_view(-f32::from(scroll.offset().x), width, card, GAP, EDGE, count)
}

/// Cards side by side, under the window's edges: the row scrolls sideways only. Its height is
/// given, so that a card without its picture yet takes the room it will.
fn cards_row<E: IntoElement>(
    index: usize,
    scroll: &ScrollHandle,
    height: f32,
    cards: Vec<E>,
) -> Stateful<Div> {
    div()
        .id(("home-row", index))
        .flex()
        .gap(px(GAP))
        .px(px(EDGE))
        .h(px(height))
        .overflow_hidden()
        .track_scroll(scroll)
        .children(cards)
}

fn poster_card(
    shell: &mut Shell,
    title: &Title,
    id: (SharedString, usize),
    near: bool,
    cx: &mut Context<Shell>,
) -> AnyElement {
    if !near {
        // Not asked yet: the room it will take, nothing more.
        return div()
            .id(id)
            .w(px(ROW_POSTER))
            .flex_none()
            .into_any_element();
    }
    match title {
        Title::Film(film) => poster(shell, film, ROW_POSTER, id, false, cx).into_any_element(),
        Title::Series(series) => {
            series_poster(shell, series, ROW_POSTER, id, cx).into_any_element()
        }
        // Posters are for what a library gained: an episode never comes here.
        Title::Episode(_) => div().into_any_element(),
    }
}

/// How much the pointer is over an element, eased.
struct Over(Fade);

/// A wide card: the picture, how far the viewer went, the title and a line; the pointer over it
/// brings a veil and a play button. A click opens the page of the title, the button plays it.
pub(super) fn wide_card(
    shell: &mut Shell,
    title: &Title,
    width: f32,
    id: (SharedString, usize),
    near: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let height = width * 9. / 16.;
    let item = match title {
        Title::Film(film) => (film.backdrop.as_ref().or(film.poster.as_ref()), Some(film)),
        Title::Episode(episode) => (
            episode
                .item
                .backdrop
                .as_ref()
                .or(episode.item.poster.as_ref()),
            Some(&episode.item),
        ),
        Title::Series(series) => (series.backdrop.as_ref().or(series.poster.as_ref()), None),
    };
    let image = near
        .then(|| shell.image(item.0, images::CARD, cx))
        .flatten();
    let (heading, detail) = home_page::card_lines(title);
    let left = home_page::time_left(title);
    let progress = home_page::progress(title);
    let opened = title.clone();
    let played = PlayTarget::of(title);

    let over_id: ElementId = (id.0.clone(), id.1 + 100_000).into();
    let over = window.use_keyed_state(over_id, cx, |_, _| Over(Fade::new(motion::quick())));
    let now = Instant::now();
    let level = over.read(cx).0.level(now);
    if over.read(cx).0.moving(now) {
        window.request_animation_frame();
    }

    let picture_box = div()
        .id(("wide-picture", id.1 + id.0.len() * 1000))
        .relative()
        .w(px(width))
        .h(px(height))
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
        .children(progress.map(|fraction| {
            div()
                .absolute()
                .left(px(14.))
                .bottom(px(14.))
                .child(progress_bar(fraction, width - 28.))
        }))
        .child(hover_layer(&id, &over, level, played, window, cx));

    div()
        .id(id)
        .w(px(width))
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(10.))
        .cursor_pointer()
        .on_click(cx.listener(move |shell, _, _, cx| shell.open_title(&opened, cx)))
        .child(picture_box)
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
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
                                .child(heading),
                        )
                        .children(left.map(|left| {
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(left)
                        })),
                )
                .children(detail.map(|detail| {
                    div()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .truncate()
                        .child(detail)
                })),
        )
}

/// The veil and the play button over a picture, `level` of the way in; it hears the pointer
/// come and go.
fn hover_layer(
    id: &(SharedString, usize),
    over: &Entity<Over>,
    level: f32,
    target: PlayTarget,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let hovered = over.clone();
    let play = theme::eased(
        (id.0.clone(), id.1 + 200_000),
        theme::Shades {
            rest: rgb(0xffffff),
            hover: rgb(0xe4e4e7),
            press: rgb(0xd4d4d8),
        },
        window,
        cx,
    )
    .size(px(48.))
    .rounded_full()
    .flex()
    .items_center()
    .justify_center()
    .text_color(rgb(0x000000))
    .cursor_pointer()
    // A click on the button plays; it never opens the page behind it.
    .on_click(cx.listener(move |shell, _, window, cx| {
        cx.stop_propagation();
        shell.play_from_home(target.clone(), window, cx)
    }))
    .child(icon(IconName::Play, 22.));
    div()
        .id((id.0.clone(), id.1 + 300_000))
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        // A veil of black that comes with the pointer.
        .bg(rgba((0x66 as f32 * level) as u32))
        .on_hover(move |hover, _, cx: &mut App| {
            hovered.update(cx, |over, cx| {
                over.0.set(*hover, Instant::now());
                cx.notify();
            });
        })
        .child(play.opacity(level))
}
