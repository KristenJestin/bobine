//! Home, after nabby.pro: a large header whose title changes by itself (cross-fading, with a bar
//! under it for each title), and under it the rows: what was started, the next episodes, and
//! what each library gained. Each row scrolls sideways and has its "See all". The page scrolls
//! up and down, and the wheel goes to the row under the pointer when it is over one.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use bobine::provider::{Feature, ImageRef, ItemId, Title};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, App, Context, Div, FontWeight, IntoElement, ObjectFit,
    Render, RenderImage, ScrollHandle, SharedString, Stateful, WeakEntity, Window, div, img,
    linear_color_stop, linear_gradient, px, relative, rgba,
};

use super::card::{Offer, Open, Play};
use super::film_detail::{EDGE, ROW_POSTER, hero_frame, logo_over_title, meta_line};
use super::{
    CAPTION_DETAIL, CAPTION_GAP, CAPTION_TITLE, UNDER_POSTER, UNDER_WIDE, caption_height,
    caption_text, card, note, picture, poster, progress_bar, row_heading, series_card,
};
use crate::grid::Grid;
use crate::hero::{self, Headline};
use crate::home_page::{self, ListKind};
use crate::icons::{IconName, icon};
use crate::images;
use crate::motion;
use crate::shell::{Loadable, Shell};
use crate::theme;

/// A card of a row of what was started, 16:9.
const WIDE: f32 = 340.;
/// What is under a wide card's picture: the gap, its title and a line, with the room between.
const WIDE_TITLE: f32 = 14.;
const WIDE_CAPTION: f32 = UNDER_WIDE + caption_height(WIDE_TITLE);
/// What is under a poster: the gap and its caption.
const POSTER_CAPTION: f32 = UNDER_POSTER + caption_height(CAPTION_TITLE);
const GAP: f32 = 16.;
/// The left of the header's text, after the design.
const HERO_INSET: f32 = 150.;
const HERO_TEXT_WIDTH: f32 = 560.;
/// The space between two rows, and under the heading of a row.
const BETWEEN_ROWS: f32 = 40.;
const HEADING: f32 = 28.;
const UNDER_HEADING: f32 = 16.;

/// What a row holds.
enum Body {
    Wide(Rc<[Title]>),
    Posters(Rc<[Title]>),
    Failed(&'static str),
}

struct RowSpec {
    /// Which row it is: the same one keeps its number as the rows around it come and go.
    slot: usize,
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

/// The rows to draw now, in order; the ones with nothing to show are left out. The titles are
/// shared with the page, not copied: this runs at every frame.
fn rows(shell: &Shell) -> Vec<RowSpec> {
    let mut rows = Vec::new();
    match &shell.home.resume {
        Some(Loadable::Ready(titles)) if !titles.is_empty() => rows.push(RowSpec {
            slot: 0,
            list: ListKind::Resume,
            caption: None,
            body: Body::Wide(Rc::clone(titles)),
        }),
        Some(Loadable::Failed(why)) => rows.push(RowSpec {
            slot: 0,
            list: ListKind::Resume,
            caption: None,
            body: Body::Failed(why),
        }),
        _ => {}
    }
    if !shell.home.next_row.is_empty() {
        rows.push(RowSpec {
            slot: 1,
            list: ListKind::NextUp,
            caption: None,
            body: Body::Wide(Rc::clone(&shell.home.next_row)),
        });
    }
    if let Some(Loadable::Ready(latest)) = &shell.home.latest {
        for (at, row) in latest.iter().enumerate() {
            let slot = 2 + at;
            let list = ListKind::Latest(row.library.id.clone(), row.library.kind);
            let caption = Some(row.library.name.clone());
            match &row.titles {
                Loadable::Ready(titles) if !titles.is_empty() => rows.push(RowSpec {
                    slot,
                    list,
                    caption,
                    body: Body::Posters(Rc::clone(titles)),
                }),
                Loadable::Failed(why) => rows.push(RowSpec {
                    slot,
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

/// How tall the header is in a window of this height.
fn hero_height(window_height: f32) -> f32 {
    (window_height * 0.72).clamp(460., 760.)
}

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    // The header's time runs while this is drawn.
    shell.start_hero_ticker(window, cx);
    shell.ask_hero_images(cx);
    let viewport = window.viewport_size();
    let (width, window_height) = (f32::from(viewport.width), f32::from(viewport.height));
    let hero_height = hero_height(window_height);
    let features = shell.home.featured().len();
    let hero_wanted = features > 0 || matches!(shell.home.features, Some(Loadable::Loading));
    // Under the pill bar when there is no header.
    let top = if hero_wanted { hero_height } else { 104. };

    let specs = rows(shell);
    let scrolls = shell.home.rows_for(specs.len());
    shell.scrolls.push(shell.home.scroll.clone());
    shell.scrolls.extend(scrolls.iter().cloned());
    let scrolled = -f32::from(shell.home.scroll.offset().y);
    let mut y = top + 12.;
    let mut shown: Vec<AnyElement> = Vec::new();
    for (index, (spec, scroll)) in specs.iter().zip(&scrolls).enumerate() {
        let near = home_page::row_in_view(y, spec.height(), scrolled, window_height);
        // A row comes in with a fade when its titles are here, instead of appearing at once.
        shown.push(
            titles_row(shell, spec, index, scroll, near, width, true, window, cx)
                .with_animation(
                    ("home-row-in", spec.slot),
                    Animation::new(motion::page()).with_easing(motion::ease_out),
                    |row, t| row.opacity(t),
                )
                .into_any_element(),
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
        (true, true) => Some("Loading…"),
        (true, false) => Some("Nothing to show yet."),
        _ => None,
    };

    let header = if features > 0 {
        Some(shell.hero_view.clone().into_any_element())
    } else if hero_wanted {
        // The header is on its way: its room is kept, so that nothing moves when it comes.
        Some(div().w_full().h(px(hero_height)).into_any_element())
    } else {
        None
    };

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
}

/// What went wrong, in a pill at the foot of the page.
pub fn pill(why: &'static str) -> Div {
    div()
        .debug_selector(|| format!("pill {why}"))
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
}

// The header.

/// The header as a view of its own: its time makes it redraw 30 times a second, and nothing
/// else on the page has to be built again for that. The shell tells it when what it shows
/// changes (`Shell::notify_hero`). It only reads the shell: a write to the shell during a draw
/// (an image asked, a listener) has the whole page built again with it. Its pictures are asked
/// by `Shell::ask_hero_images`, its clicks reach the shell through a weak handle.
pub struct HeroView {
    shell: WeakEntity<Shell>,
    logos: hero::Logos,
}

impl HeroView {
    pub fn new(shell: WeakEntity<Shell>) -> Self {
        Self {
            shell,
            logos: hero::Logos::default(),
        }
    }

    /// The logos of the titles of a session that ended.
    pub fn forget_logos(&mut self) {
        self.logos.keep_only(&[]);
    }

    #[cfg(test)]
    pub fn logos_len(&self) -> usize {
        self.logos.len()
    }
}

impl Render for HeroView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(shell) = self.shell.upgrade() else {
            return div().into_any_element();
        };
        let height = hero_height(f32::from(window.viewport_size().height));
        // The header's time is the executor's, the one that ticks it: the same one the tests drive.
        let now = cx.background_executor().now();
        let (features, frame) = {
            let shell = shell.read(cx);
            #[cfg(feature = "harness")]
            shell.note_frame();
            #[cfg(any(test, feature = "harness"))]
            shell.draws.borrow_mut().hero.push(now);
            let Some(features) = shell.home.shared_features().filter(|f| !f.is_empty()) else {
                return div().into_any_element();
            };
            let carousel = &shell.home.carousel;
            let frame = Frame {
                current: carousel.current().min(features.len() - 1),
                blend: carousel.blend(now),
                crossfading: carousel.crossfading(now),
                progress: carousel.progress(now),
                busy: shell.home.busy,
                backdrops: features
                    .iter()
                    .map(|f| shell.peek_image(f.backdrop.as_ref(), images::BACKDROP))
                    .collect(),
                logos: features
                    .iter()
                    .map(|f| shell.peek_image(f.logo.as_ref(), images::LOGO))
                    .collect(),
                font: shell.font(),
            };
            (features, frame)
        };
        self.logos.keep_only(&features);
        let target = self.shell.clone();
        let mut view = Draw {
            frame,
            target: &target,
            logos: &mut self.logos,
        };
        view.hero(&features, height, window, cx).into_any_element()
    }
}

/// What the shell holds that one draw of the header needs, read once.
struct Frame {
    /// The title shown, and the one fading out under it if a change is under way.
    current: usize,
    blend: hero::Blend,
    crossfading: bool,
    progress: f32,
    busy: bool,
    /// The pictures of each title, when they are here.
    backdrops: Vec<Option<Arc<RenderImage>>>,
    logos: Vec<Option<Arc<RenderImage>>>,
    font: Option<SharedString>,
}

/// One draw of the header: it holds what it read of the shell, and writes nothing to it.
struct Draw<'a> {
    frame: Frame,
    /// Where the clicks go.
    target: &'a WeakEntity<Shell>,
    logos: &'a mut hero::Logos,
}

impl Draw<'_> {
    fn hero(
        &mut self,
        features: &Rc<[Feature]>,
        height: f32,
        window: &mut Window,
        cx: &mut Context<HeroView>,
    ) -> impl IntoElement {
        let (current, blend) = (self.frame.current, self.frame.blend);
        if self.frame.crossfading {
            window.request_animation_frame();
        }
        let (text_out, text_in) = hero::text_levels(blend.level);

        let leaving = blend.leaving.filter(|at| *at < features.len());
        let backdrop = |at: usize, level: f32| {
            div()
                .absolute()
                .inset_0()
                .opacity(level)
                .child(hero_frame(self.frame.backdrops[at].clone(), height))
        };
        let mut layers: Vec<AnyElement> = Vec::new();
        if let Some(leaving) = leaving {
            layers.push(backdrop(leaving, 1.).into_any_element());
        }
        layers.push(backdrop(current, blend.level).into_any_element());
        if let Some(leaving) = leaving {
            layers.push(
                self.slide_text(features, leaving, text_out, false, window, cx)
                    .into_any_element(),
            );
        }
        layers.push(
            self.slide_text(features, current, text_in, true, window, cx)
                .into_any_element(),
        );

        let target = self.target.clone();
        let marks = (features.len() > 1).then(|| self.marks(features.len(), window, cx));
        div()
            .id("hero")
            .relative()
            .w_full()
            .h(px(height))
            .overflow_hidden()
            .on_hover(move |hovered: &bool, window, cx: &mut App| {
                let hovered = *hovered;
                target
                    .update(cx, |shell, cx| shell.hover_hero(hovered, window, cx))
                    .ok();
            })
            .children(layers)
            .children(marks)
            // The header comes in with a fade the first time it is drawn.
            .with_animation(
                "hero-in",
                Animation::new(motion::page()).with_easing(motion::ease_out),
                |header, t| header.opacity(t),
            )
    }

    /// The text of the `at`th title, `level` of the way in. Only the title on show answers a
    /// click, and a click names its title: the header may have changed by the time it comes.
    fn slide_text(
        &mut self,
        features: &[Feature],
        at: usize,
        level: f32,
        live: bool,
        window: &mut Window,
        cx: &mut Context<HeroView>,
    ) -> Div {
        let feature = &features[at];
        let headline = match hero::headline(feature) {
            Headline::Logo(logo) => self.logo_or_title(&logo, at, &feature.title, window),
            Headline::Text(title) => title_text(title).into_any_element(),
        };
        // One id for each button of each title and of each of its two states.
        let id = |name: &'static str| (name, at * 2 + usize::from(live));
        let play = theme::primary_button(
            id("play"),
            Some(IconName::Play),
            hero::play_label(feature),
            window,
            cx,
        )
        .when(live, |button| {
            let target = self.target.clone();
            let what = home_page::play_target(feature);
            button.on_click(move |_, window, cx| {
                let what = what.clone();
                target
                    .update(cx, |shell, cx| shell.play_from_home(what, window, cx))
                    .ok();
            })
        });
        let more = theme::ghost_button(id("more"), Some(IconName::Plus), "More info", window, cx)
            .when(live, |button| {
                let target = self.target.clone();
                let (kind, item) = (feature.kind, feature.id.clone());
                button.on_click(move |_, _, cx| {
                    let item = item.clone();
                    target
                        .update(cx, |shell, cx| shell.open_feature(kind, item, cx))
                        .ok();
                })
            });
        let busy = self.frame.busy;
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
            .child(meta_line(
                hero::meta(feature),
                self.frame.font.clone(),
                window,
            ))
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

    /// The logo, at the size it fits in the room given, once it is here, taking over from the
    /// title; the title in text until then, so that the header is never empty while the logo
    /// comes.
    fn logo_or_title(
        &mut self,
        logo: &ImageRef,
        at: usize,
        title: &str,
        window: &mut Window,
    ) -> AnyElement {
        let drawn = self.frame.logos[at].clone().and_then(|image| {
            let size = image.size(0);
            let fit = hero::logo_fit((size.width.0 as f32, size.height.0 as f32))?;
            Some((image, fit))
        });
        match drawn {
            // It fades in when it comes, once: not again each time its title comes back.
            Some((image, (w, h))) => {
                let level = self.logos.level(&logo.item, Instant::now());
                if level < 1. {
                    window.request_animation_frame();
                }
                logo_over_title(
                    img(image).w(px(w)).h(px(h)).object_fit(ObjectFit::Contain),
                    title_text(title.to_string()),
                    HERO_TEXT_WIDTH,
                    level,
                )
                .into_any_element()
            }
            None => title_text(title.to_string()).into_any_element(),
        }
    }

    /// A mark for each title under the header, in line with its text: a dot, and for the title
    /// shown a pill that fills as its time runs. A click on one shows its title.
    fn marks(&self, count: usize, window: &mut Window, cx: &mut Context<HeroView>) -> Div {
        let (current, progress, blend) =
            (self.frame.current, self.frame.progress, self.frame.blend);
        let shades = theme::Shades {
            rest: rgba(0xffffff40),
            hover: rgba(0xffffff80),
            press: rgba(0xffffffa6),
        };
        div()
            .absolute()
            .left(px(HERO_INSET - MARK_PAD))
            .bottom(px(30.))
            .flex()
            .items_center()
            .children((0..count).map(|index| {
                let width = hero::mark_width(index, current, blend);
                let mark = theme::eased(("hero-track", index), shades, window, cx)
                    .debug_selector(move || format!("hero-mark {index}"))
                    .w(px(width))
                    .h(px(hero::DOT))
                    .rounded_full()
                    .overflow_hidden()
                    .children((index == current).then(|| {
                        div()
                            .h_full()
                            .w(relative(progress))
                            .rounded_full()
                            .bg(theme::accent())
                    }));
                let target = self.target.clone();
                // The mark is small, its click is not.
                div()
                    .id(("hero-mark", index))
                    .flex_none()
                    .px(px(MARK_PAD))
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        target
                            .update(cx, |shell, cx| shell.jump_hero(index, cx))
                            .ok();
                    })
                    .child(mark)
            }))
    }
}

/// Around each mark of the header, part of its click.
const MARK_PAD: f32 = 5.;

fn title_text(title: String) -> Div {
    div()
        .text_size(px(60.))
        .line_height(px(64.))
        .font_weight(FontWeight::BOLD)
        .child(title)
}

/// `● Movie` over the title.
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
    see_all: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let list = spec.list.clone();
    let library = spec.caption.clone();
    let see_all_link = theme::eased(("see-all", index), theme::Shades::veil(), window, cx)
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
        .child("See all")
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
        .children((see_all && !matches!(spec.body, Body::Failed(_))).then_some(see_all_link));

    let body = match &spec.body {
        Body::Failed(why) => div().px(px(EDGE)).child(note(*why)).into_any_element(),
        Body::Wide(titles) => {
            let range = near_cards(scroll, width, WIDE, titles.len(), near);
            let name = SharedString::from(format!("home-wide-{index}"));
            let cards: Vec<_> = (0..titles.len())
                .map(|at| {
                    let id = (name.clone(), at);
                    let image = range
                        .contains(&at)
                        .then(|| shell.image(card_picture(&titles[at]), images::CARD, cx))
                        .flatten();
                    let pick = Pick::Row(Rc::clone(titles), at);
                    let offer = wide_offer(&titles[at], image, pick, WIDE);
                    card(shell, id.into(), offer, cx)
                })
                .collect();
            cards_row(index, scroll, WIDE * 9. / 16. + WIDE_CAPTION, cards).into_any_element()
        }
        Body::Posters(titles) => {
            let range = near_cards(scroll, width, ROW_POSTER, titles.len(), near);
            let name = SharedString::from(format!("home-poster-{index}"));
            let cards: Vec<AnyElement> = titles
                .iter()
                .enumerate()
                .map(|(at, title)| {
                    let id = (name.clone(), at);
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

/// The number of the suggestion row, apart from the Home's own rows.
const SUGGESTION: usize = 1000;

/// A row of the Home to watch instead, under a page that holds nothing: the full list `empty`
/// (never offered back), My list or Collections (`None`). It has no "See all": a full list does
/// not open over another.
pub(super) fn suggestion_row(
    shell: &mut Shell,
    empty: Option<&ListKind>,
    scroll: &ScrollHandle,
    width: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<Div> {
    shell.scrolls.push(scroll.clone());
    let mut specs = rows(shell);
    specs.retain(|spec| !matches!(spec.body, Body::Failed(_)));
    let kinds: Vec<ListKind> = specs.iter().map(|spec| spec.list.clone()).collect();
    let spec = specs.swap_remove(home_page::suggested(empty, &kinds)?);
    let heading = spec.list.heading();
    Some(
        titles_row(
            shell, &spec, SUGGESTION, scroll, true, width, false, window, cx,
        )
        .debug_selector(move || format!("suggested: {heading}")),
    )
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
            // The posters of the Home are the rows of Recently added.
            series_card(shell, series, ROW_POSTER, true, id, cx).into_any_element()
        }
        // Posters are for what a library gained: an episode never comes here.
        Title::Episode(_) => div().into_any_element(),
    }
}

/// Where a card's title is read from when it is clicked: the card holds no copy of it, which
/// would be made again at every frame.
#[derive(Clone)]
pub enum Pick {
    /// The `at`th title of a row of the Home.
    Row(Rc<[Title]>, usize),
    /// The `at`th title of the full list, which must still be `id`: the list may have been
    /// refreshed since the card was drawn.
    Listed(usize, ItemId),
}

impl Pick {
    pub fn title(&self, list: Option<&Grid<Title>>) -> Option<Title> {
        match self {
            Self::Row(titles, at) => titles.get(*at).cloned(),
            Self::Listed(at, id) => list?.film(*at).filter(|title| title.id() == id).cloned(),
        }
    }
}

/// The picture of a wide card: the backdrop, else the poster.
pub(super) fn card_picture(title: &Title) -> Option<&ImageRef> {
    match title {
        Title::Film(film) => film.backdrop.as_ref().or(film.poster.as_ref()),
        Title::Episode(episode) => episode
            .item
            .backdrop
            .as_ref()
            .or(episode.item.poster.as_ref()),
        Title::Series(series) => series.backdrop.as_ref().or(series.poster.as_ref()),
    }
}

/// A wide card: the picture, how far the viewer went, the title and a line; the pointer over the
/// picture brings a veil and a play button. A click opens the page of the title, the button
/// plays it. Neither reads the title from the card, which holds no copy of it: they read it
/// where `pick` says.
pub(super) fn wide_offer(
    title: &Title,
    image: Option<Arc<RenderImage>>,
    pick: Pick,
    width: f32,
) -> Offer {
    let height = width * 9. / 16.;
    let (heading, detail) = home_page::card_lines(title);
    let left = home_page::time_left(title);
    let progress = home_page::progress(title);
    let draw = move |layer: Stateful<Div>| {
        let picture_box = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .rounded(px(theme::RADIUS_LARGE))
            .overflow_hidden()
            .bg(theme::panel())
            .children(picture(image.clone(), theme::RADIUS_LARGE))
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
            .child(layer);
        div()
            .w(px(width))
            .flex()
            .flex_col()
            .gap(px(UNDER_WIDE))
            .child(picture_box)
            .child(
                div()
                    .debug_selector(|| "card-caption".into())
                    .flex()
                    .flex_col()
                    .gap(px(CAPTION_GAP))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                caption_text(WIDE_TITLE, heading.clone())
                                    .font_weight(FontWeight::SEMIBOLD),
                            )
                            .children(left.clone().map(|left| {
                                caption_text(CAPTION_DETAIL, left)
                                    .flex_none()
                                    .text_color(theme::muted())
                            })),
                    )
                    .children(detail.clone().map(|detail| {
                        caption_text(CAPTION_DETAIL, detail).text_color(theme::dim())
                    })),
            )
    };
    Offer {
        open: Open::Pick(pick.clone()),
        play: Play::Pick(pick),
        quick: None,
        draw: Rc::new(draw),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::films;
    use bobine::provider::Page;
    use gpui_kit::ElementId;

    fn titles(count: usize) -> Vec<Title> {
        films(count).into_iter().map(Title::Film).collect()
    }

    #[test]
    fn a_click_on_a_card_of_a_row_reads_its_title_from_the_row() {
        let row: Rc<[Title]> = Rc::from(titles(3));
        assert_eq!(
            Pick::Row(Rc::clone(&row), 1).title(None),
            Some(row[1].clone())
        );
        assert_eq!(Pick::Row(row, 3).title(None), None);
    }

    #[test]
    fn a_click_on_a_card_of_the_full_list_reads_its_title_from_the_list() {
        let mut grid: Grid<Title> = Grid::default();
        let all = titles(3);
        grid.loaded(
            grid.epoch(),
            0,
            Page {
                items: all.clone(),
                total: 3,
            },
        );
        let pick = |at: usize| Pick::Listed(at, all[at].id().clone());
        assert_eq!(pick(2).title(Some(&grid)), Some(all[2].clone()));
        assert_eq!(
            Pick::Listed(3, all[2].id().clone()).title(Some(&grid)),
            None
        );
        assert_eq!(pick(0).title(None), None);
    }

    #[test]
    fn a_click_on_a_card_whose_place_now_holds_another_title_opens_nothing() {
        let mut grid: Grid<Title> = Grid::default();
        let all = titles(3);
        grid.loaded(
            grid.epoch(),
            0,
            Page {
                items: all.clone(),
                total: 3,
            },
        );
        let drawn = Pick::Listed(1, all[2].id().clone());
        assert_eq!(drawn.title(Some(&grid)), None);
    }

    /// A card in a row of the Home, in the room the row keeps for it.
    struct InRow(Rc<dyn Fn() -> AnyElement>);

    impl Render for InRow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child((self.0)())
        }
    }

    /// Where the caption of `card` is drawn, and the row around it, `room` high.
    fn caption_in_room(
        room: f32,
        card: impl Fn() -> AnyElement + 'static,
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        gpui_kit::Bounds<gpui_kit::Pixels>,
        gpui_kit::Bounds<gpui_kit::Pixels>,
    ) {
        let card = Rc::new(card);
        let window = cx.add_window(move |_, _| {
            let card = Rc::clone(&card);
            InRow(Rc::new(move || {
                let card = div().id("card").flex_none().child(card());
                cards_row(0, &ScrollHandle::new(), room, vec![card])
                    .debug_selector(|| "room".into())
                    .into_any_element()
            }))
        });
        let mut visual = gpui_kit::VisualTestContext::from_window(window.into(), cx);
        visual.run_until_parked();
        let caption = visual.debug_bounds("card-caption").expect("the caption");
        let room = visual.debug_bounds("room").expect("the room");
        (caption, room)
    }

    /// The caption under a poster of a row, and under a wide card, sits whole in the room the
    /// row keeps: its two lines take the height of their text, descenders included.
    #[gpui_kit::test]
    fn no_caption_under_any_card_is_clipped(cx: &mut gpui_kit::TestAppContext) {
        let mut film = films(1).remove(0);
        film.year = Some(2019);
        let poster = film.clone();
        let (caption, room) = caption_in_room(
            ROW_POSTER * 1.5 + POSTER_CAPTION,
            move || {
                let name = ElementId::from(("poster", 0usize));
                super::super::film_look(&poster, None, ROW_POSTER, &name, false, false, false)
                    .draw(None)
                    .into_any_element()
            },
            cx,
        );
        assert!(
            caption.bottom() <= room.bottom(),
            "the poster's caption ends at {:?}, its room at {:?}",
            caption.bottom(),
            room.bottom()
        );
        let wide = Title::Film(film);
        let (caption, room) = caption_in_room(
            WIDE * 9. / 16. + WIDE_CAPTION,
            move || {
                let pick = Pick::Row(Rc::from(vec![wide.clone()]), 0);
                let offer = wide_offer(&wide, None, pick, WIDE);
                (offer.draw)(div().id("layer")).into_any_element()
            },
            cx,
        );
        assert!(
            caption.bottom() <= room.bottom(),
            "the wide card's caption ends at {:?}, its room at {:?}",
            caption.bottom(),
            room.bottom()
        );
    }
}
