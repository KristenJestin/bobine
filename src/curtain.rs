//! The way into the player and back out of it. The video is a subsurface under the window, which
//! GPUI cannot move, scale nor fade: the move is all in what GPUI draws over it. Opening, an
//! opaque cover hides the video: the picture that was clicked grows from where it was to the
//! whole window while the page fades out over it and the player's controls fade in; the cover
//! fades out onto the video once its first frame is there, never before, never later. Closing,
//! the video is gone at once: the picture shrinks back from the whole window to its place while
//! the page comes back.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobine::provider::ItemId;
use gpui_kit::prelude::*;
use gpui_kit::{Bounds, IntoElement, Pixels, RenderImage, canvas, point, size};

use crate::motion;

/// A title's picture where a page drew it: the way into the player grows from it.
#[derive(Clone)]
pub struct Picture {
    pub item: ItemId,
    pub image: Arc<RenderImage>,
    pub bounds: Bounds<Pixels>,
}

/// The picture the last frame drew of the page's title.
pub type Drawn = Rc<RefCell<Option<Picture>>>;

/// Laid over the picture of `item` a page draws: where it is, kept at each frame.
pub fn mark(drawn: &Drawn, item: &ItemId, image: Option<Arc<RenderImage>>) -> impl IntoElement {
    let (drawn, item) = (drawn.clone(), item.clone());
    canvas(
        move |bounds, _, _| {
            *drawn.borrow_mut() = image.map(|image| Picture {
                item,
                image,
                bounds,
            });
        },
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
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
                page: done,
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
        assert_eq!(later.page + later.player, 1.);
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
    fn closing_shrinks_the_picture_back_while_the_page_comes_back() {
        let start = Instant::now();
        let curtain: Curtain<()> = Curtain::closing(Some(()), start);
        assert_eq!(curtain.way(), Way::Closing);
        let first = curtain.at(start);
        assert_eq!((first.grown, first.page, first.player), (1., 0., 0.));
        let half = curtain.at(start + 150 * MS);
        assert!(half.grown < 0.5 && half.page > 0.5, "{half:?}");
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
