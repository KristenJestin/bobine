//! The way into the player and back out of it. The video is a subsurface under the window, which
//! GPUI cannot move, scale nor fade: the move is all in what GPUI draws over it. Opening, an
//! opaque cover hides the video: the picture that was clicked grows from where it was to the
//! whole window while the page fades out over it and the player's controls fade in; the cover
//! fades out onto the video once its first frame is there, never before, never later. Closing,
//! the video is gone at once: the picture shrinks back from the whole window to its place on the
//! page, which is already there; when the page no longer shows it, it fades out instead.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobine::provider::ItemId;
use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, ElementId, IntoElement, Pixels, RenderImage, Size, canvas, point, px, size,
};

use crate::motion;

/// A title's picture where a page drew it: the way into the player grows from it.
#[derive(Clone)]
pub struct Picture {
    /// Where it is drawn: the way out looks for it there.
    pub at: ElementId,
    pub item: ItemId,
    pub image: Arc<RenderImage>,
    pub bounds: Bounds<Pixels>,
}

/// Where the page's own title has its picture: a film's or a series' backdrop.
pub const PAGE: &str = "curtain-page";

/// The pictures the last frame drew that the way into the player may grow from, by where they
/// are drawn: the page's title, the Home's header, each card, each line of a playlist.
pub type Drawn = Rc<RefCell<HashMap<ElementId, Picture>>>;

/// Laid over the picture of `item` drawn at `at`: where it is, kept at each frame.
pub fn mark(
    drawn: &Drawn,
    at: ElementId,
    item: &ItemId,
    image: Option<Arc<RenderImage>>,
) -> impl IntoElement + use<> {
    let (drawn, item) = (drawn.clone(), item.clone());
    canvas(
        move |bounds, _, _| {
            if let Some(image) = image {
                let picture = Picture {
                    at: at.clone(),
                    item,
                    image,
                    bounds,
                };
                drawn.borrow_mut().insert(at, picture);
            }
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

/// Some of `bounds` shows in a window of `window`.
pub fn in_sight(bounds: Bounds<Pixels>, window: Size<Pixels>) -> bool {
    bounds.bottom() > px(0.)
        && bounds.top() < window.height
        && bounds.right() > px(0.)
        && bounds.left() < window.width
}

/// Where the way out draws its picture `grown` of the way, and how much of it shows: shrinking
/// back to `place`, the picture's place on the page in sight; with none, fading out where it
/// covers the whole window.
pub fn back(
    place: Option<Bounds<Pixels>>,
    whole: Bounds<Pixels>,
    grown: f32,
) -> (Bounds<Pixels>, f32) {
    match place {
        Some(place) => (between(place, whole, grown), 1.),
        None => (whole, grown),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    Opening,
    Closing,
}

/// What a frame of the move draws, each from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// How far the picture went from its place on the page (0) to the whole window (1).
    pub grown: f32,
    /// How much of the page shows.
    pub page: f32,
    /// How much of the player's controls show.
    pub player: f32,
    /// How much of the cover (the dark ground and the picture) shows over the video.
    pub cover: f32,
}

/// One way in or out of the player; `P` is the picture that moves, if there is one.
pub struct Curtain<P> {
    way: Way,
    /// When it was asked, then when its first frame was drawn: what held the window before
    /// (mpv opening or stopping) is not taken from the move.
    since: Instant,
    drawn: bool,
    length: Duration,
    reveal: Duration,
    first_frame: Option<Instant>,
    /// Closing: the page has shown the picture's place, then no longer shows it.
    placed: bool,
    lost: bool,
    pub picture: Option<P>,
}

impl<P> Curtain<P> {
    pub fn opening(picture: Option<P>, now: Instant) -> Self {
        Self::new(Way::Opening, picture, now)
    }

    pub fn closing(picture: Option<P>, now: Instant) -> Self {
        Self::new(Way::Closing, picture, now)
    }

    fn new(way: Way, picture: Option<P>, now: Instant) -> Self {
        Self {
            way,
            since: now,
            drawn: false,
            length: motion::curtain(),
            reveal: motion::quick(),
            first_frame: None,
            placed: false,
            lost: false,
            picture,
        }
    }

    pub fn way(&self) -> Way {
        self.way
    }

    /// A test sets where the move stands against the real clock, which it cannot wait for.
    #[cfg(test)]
    pub fn started_at(&mut self, at: Instant) {
        self.since = at;
    }

    /// A test makes the move last long enough to look at any stage of it.
    #[cfg(test)]
    pub fn lasting(&mut self, length: Duration) {
        self.length = length;
    }

    /// Closing: where the picture shrinks back to, given where the page draws it now. It shrinks
    /// while the page shows it; once the page has shown it and no longer does, it fades out to
    /// the end, even if the page brings it back into sight meanwhile. Before the page is drawn
    /// there is nothing to lose yet.
    pub fn place(&mut self, found: Option<Bounds<Pixels>>) -> Option<Bounds<Pixels>> {
        match found {
            Some(_) if self.lost => None,
            Some(_) => {
                self.placed = true;
                found
            }
            None => {
                self.lost = self.placed;
                None
            }
        }
    }

    /// The video's first frame is on the window: the cover goes. Only the first one counts.
    pub fn first_frame(&mut self, now: Instant) {
        if self.way == Way::Opening {
            self.first_frame.get_or_insert(now);
        }
    }

    /// Opening, the video's first frame is still awaited.
    pub fn waiting(&self) -> bool {
        self.way == Way::Opening && self.first_frame.is_none()
    }

    /// How far the move is, from 0 to 1, eased.
    fn done(&self, now: Instant) -> f32 {
        let t = now.saturating_duration_since(self.since).as_secs_f32();
        motion::ease_out(t / self.length.as_secs_f32())
    }

    /// The frame drawn at `now`.
    pub fn draw(&mut self, now: Instant) -> Frame {
        if !self.drawn {
            self.drawn = true;
            self.since = now;
        }
        self.at(now)
    }

    pub fn at(&self, now: Instant) -> Frame {
        let done = self.done(now);
        match self.way {
            Way::Opening => Frame {
                grown: done,
                page: 1. - done,
                player: done,
                // From the first frame on, whether the picture is still growing or not: the
                // video is never kept waiting under the cover.
                cover: self.first_frame.map_or(1., |shown| {
                    let t = now.saturating_duration_since(shown).as_secs_f32();
                    1. - motion::ease_out(t / self.reveal.as_secs_f32())
                }),
            },
            Way::Closing => Frame {
                grown: 1. - done,
                // The page is the screen already: the picture shrinks back over it.
                page: 1.,
                player: 0.,
                cover: 1.,
            },
        }
    }

    /// The window needs another frame for it.
    pub fn moving(&self, now: Instant) -> bool {
        // Grown, the cover waits for the video without drawing anything.
        !self.over(now) && (now < self.since + self.length || self.first_frame.is_some())
    }

    /// Drawn and on its way: the window must not be held meanwhile (mpv answers slowly while
    /// it opens a file, and asking it then would cost frames).
    pub fn animating(&self, now: Instant) -> bool {
        self.drawn && self.moving(now)
    }

    /// Nothing of it is left to draw.
    pub fn over(&self, now: Instant) -> bool {
        let moved = now >= self.since + self.length;
        match self.way {
            Way::Opening => {
                moved
                    && self
                        .first_frame
                        .is_some_and(|shown| now >= shown + self.reveal)
            }
            Way::Closing => moved,
        }
    }
}

/// `t` of the way from `from` to `to`.
pub fn between(from: Bounds<Pixels>, to: Bounds<Pixels>, t: f32) -> Bounds<Pixels> {
    let at = |a: Pixels, b: Pixels| a + (b - a) * t;
    Bounds::new(
        point(
            at(from.origin.x, to.origin.x),
            at(from.origin.y, to.origin.y),
        ),
        size(
            at(from.size.width, to.size.width),
            at(from.size.height, to.size.height),
        ),
    )
}

#[cfg(test)]
mod tests {
    use gpui_kit::px;

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    fn opening(start: Instant) -> Curtain<()> {
        Curtain::opening(Some(()), start)
    }

    #[test]
    fn the_transition_is_about_300_ms_and_eased() {
        let start = Instant::now();
        let curtain = opening(start);
        let half = curtain.at(start + 150 * MS).grown;
        assert!(half > 0.5 && half < 1., "eased out: {half}");
        assert_eq!(curtain.at(start + 300 * MS).grown, 1.);
        assert_eq!(curtain.at(start).grown, 0.);
    }

    #[test]
    fn opening_grows_the_picture_fades_the_page_and_keeps_the_video_covered() {
        let start = Instant::now();
        let curtain = opening(start);
        let first = curtain.at(start);
        assert_eq!((first.page, first.player, first.cover), (1., 0., 1.));
        let later = curtain.at(start + 150 * MS);
        assert!(later.page < 1. && later.player > 0.);
        assert!((later.page + later.player - 1.).abs() < 1e-6);
        assert_eq!(
            later.cover, 1.,
            "nothing of the video shows before its first frame"
        );
        assert!(curtain.waiting());
        assert!(curtain.moving(start + 150 * MS));
    }

    #[test]
    fn a_first_frame_before_the_end_finishes_the_transition_on_the_video() {
        let start = Instant::now();
        let mut curtain = opening(start);
        curtain.first_frame(start + 60 * MS);
        assert!(!curtain.waiting());
        // The cover goes from the first frame on, while the picture still grows.
        let fading = curtain.at(start + 120 * MS);
        assert!(fading.cover < 1. && fading.grown < 1.);
        assert_eq!(curtain.at(start + 60 * MS + motion::quick()).cover, 0.);
        assert!(curtain.over(start + 300 * MS));
        assert!(!curtain.moving(start + 300 * MS));
    }

    #[test]
    fn a_first_frame_after_the_end_is_awaited_under_the_cover_then_faded_into() {
        let start = Instant::now();
        let mut curtain = opening(start);
        let grown = curtain.at(start + 400 * MS);
        assert_eq!((grown.grown, grown.page, grown.player), (1., 0., 1.));
        assert_eq!(grown.cover, 1., "the cover holds while the video is late");
        assert!(!curtain.over(start + 2000 * MS));
        // Nothing moves meanwhile: an idle window draws nothing.
        assert!(!curtain.moving(start + 2000 * MS));
        curtain.first_frame(start + 2000 * MS);
        assert!(curtain.moving(start + 2010 * MS));
        assert!(curtain.at(start + 2050 * MS).cover < 1.);
        let end = start + 2000 * MS + motion::quick();
        assert_eq!(curtain.at(end).cover, 0.);
        assert!(curtain.over(end));
    }

    #[test]
    fn only_the_first_frame_counts() {
        let start = Instant::now();
        let mut curtain = opening(start);
        curtain.first_frame(start + 400 * MS);
        curtain.first_frame(start + 500 * MS);
        assert!(curtain.over(start + 400 * MS + motion::quick()));
    }

    #[test]
    fn closing_shrinks_the_picture_back_to_its_place_on_the_page() {
        let start = Instant::now();
        let curtain: Curtain<()> = Curtain::closing(Some(()), start);
        assert_eq!(curtain.way(), Way::Closing);
        let first = curtain.at(start);
        assert_eq!((first.grown, first.page, first.player), (1., 1., 0.));
        let half = curtain.at(start + 150 * MS);
        assert!(half.grown < 0.5, "{half:?}");
        assert_eq!(half.page, 1., "the page is the screen: it is not faded in");
        assert!(!curtain.waiting(), "closing awaits no video");
        let end = curtain.at(start + 300 * MS);
        assert_eq!((end.grown, end.page), (0., 1.));
        assert!(curtain.over(start + 300 * MS));
        assert!(curtain.moving(start + 150 * MS));
    }

    #[test]
    fn a_move_starts_at_its_first_frame_not_when_it_was_asked() {
        // mpv stopping holds the window for a moment after the way out is asked.
        let asked = Instant::now();
        let mut curtain: Curtain<()> = Curtain::closing(Some(()), asked);
        let drawn = asked + 400 * MS;
        assert_eq!(
            curtain.draw(drawn).grown,
            1.,
            "the whole move is still to come"
        );
        assert!(!curtain.over(drawn + 100 * MS));
        assert!(curtain.moving(drawn + 100 * MS));
        assert!(curtain.over(drawn + 300 * MS));
    }

    #[test]
    fn the_player_is_left_alone_only_while_the_move_is_drawn() {
        let start = Instant::now();
        let mut curtain = opening(start);
        assert!(
            !curtain.animating(start),
            "not drawn yet: nothing to keep smooth"
        );
        curtain.draw(start + 50 * MS);
        assert!(curtain.animating(start + 100 * MS));
        // Grown, the cover waits for the video without moving: the player may be asked.
        assert!(!curtain.animating(start + 400 * MS));
        curtain.first_frame(start + 400 * MS);
        assert!(curtain.animating(start + 450 * MS), "the cover fades out");
        assert!(!curtain.animating(start + 400 * MS + motion::quick()));
    }

    #[test]
    fn the_way_out_shrinks_to_a_place_in_sight_and_fades_out_without_one() {
        let window = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(600.)));
        let card = Bounds::new(point(px(100.), px(400.)), size(px(200.), px(100.)));
        assert!(in_sight(card, window.size));
        assert_eq!(
            back(Some(card), window, 0.5),
            (between(card, window, 0.5), 1.)
        );
        let under = Bounds::new(point(px(100.), px(600.)), size(px(200.), px(100.)));
        assert!(!in_sight(under, window.size));
        let aside = Bounds::new(point(px(-200.), px(100.)), size(px(200.), px(100.)));
        assert!(!in_sight(aside, window.size));
        assert_eq!(back(None, window, 1.), (window, 1.));
        assert_eq!(
            back(None, window, 0.25),
            (window, 0.25),
            "it fades where it was grown"
        );
        assert_eq!(back(None, window, 0.).1, 0.);
    }

    #[test]
    fn a_picture_that_loses_its_place_fades_out_to_the_end_without_shrinking_back() {
        let card = Bounds::new(point(px(100.), px(400.)), size(px(200.), px(100.)));
        let mut curtain: Curtain<()> = Curtain::closing(Some(()), Instant::now());
        // The first frame comes before the page is drawn: it has not lost anything yet.
        assert_eq!(curtain.place(None), None);
        assert_eq!(curtain.place(Some(card)), Some(card));
        assert_eq!(curtain.place(Some(card)), Some(card));
        assert_eq!(curtain.place(None), None);
        assert_eq!(curtain.place(Some(card)), None, "it does not come back");
        assert_eq!(curtain.place(None), None);
    }

    #[test]
    fn the_picture_moves_from_its_place_to_the_whole_window() {
        let place = Bounds::new(point(px(0.), px(-100.)), size(px(1000.), px(500.)));
        let window = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(600.)));
        assert_eq!(between(place, window, 0.), place);
        assert_eq!(between(place, window, 1.), window);
        let half = between(place, window, 0.5);
        assert_eq!(half.origin.y, px(-50.));
        assert_eq!(half.size.height, px(550.));
    }
}
