//! The preview over the player's seek bar: the time under the pointer and, when the server made
//! trickplay images, the thumbnail of that moment. This asks the server for the layout of the
//! thumbnails once the film plays, and for the tiles near the pointer while the bar is hovered;
//! every call runs off the window's thread.

use std::sync::Arc;
use std::time::Instant;

use bobine::provider::{ItemId, MediaSourceId, ProviderError, Trickplay};
use gpui_kit::{Context, Image, RenderImage, Window};

use super::Shell;
use crate::images;
use crate::motion::{self, Fade};
use crate::player::scrub::{self, Scrub};
use crate::player::thumbnails::{self, BUDGET, SHOWN_WIDTH, Target, Thumbnails, Tiles};
use crate::player::time;
use crate::screens::player::{EDGE, Preview, TIME_BOX_WIDTH, Thumb};

/// How far along the seek bar a pointer at `x` is, from 0 to 1 and beyond at either end; the
/// bar runs from `EDGE` to `EDGE` before the right of a window `window_width` wide.
pub fn fraction_at(x: f32, window_width: f32) -> f32 {
    (x - EDGE) / (window_width - 2. * EDGE).max(1.)
}

/// What the preview needs of one playback.
pub struct SeekPreview {
    pub scrub: Scrub,
    thumbnails: Thumbnails<Arc<RenderImage>>,
    /// The film and version the thumbnails are asked for.
    target: Option<Target>,
    /// The thumbnail's own fade: it comes in when its tile is here.
    thumb_fade: Fade,
    /// The last thumbnail drawn, so that it fades out rather than vanishes.
    last: Option<Thumb>,
}

impl SeekPreview {
    pub fn new() -> Self {
        Self {
            scrub: Scrub::new(),
            thumbnails: Thumbnails::Unknown,
            target: None,
            thumb_fade: Fade::new(motion::quick()),
            last: None,
        }
    }

    /// The tile under the pointer, while the bar is hovered or dragged.
    fn pointer_tile(&self, duration: f64) -> Option<u32> {
        let tiles = self.thumbnails.tiles()?;
        self.scrub
            .active()
            .then(|| tiles.trickplay().cell_at(self.scrub.seconds(duration)).tile)
    }

    /// Still on its way: the window needs another frame.
    pub fn moving(&self, now: Instant) -> bool {
        self.scrub.moving(now) || self.thumb_fade.moving(now)
    }

    /// The preview to draw now, or none when it is gone.
    pub fn draw(&mut self, duration: f64, now: Instant) -> Option<Preview> {
        let opacity = self.scrub.level(now);
        if opacity == 0. && !self.moving(now) {
            self.last = None;
            return None;
        }
        let seconds = self.scrub.seconds(duration);
        let thumb = self.thumbnails.tiles().and_then(|tiles| {
            let trickplay = tiles.trickplay();
            let cell = trickplay.cell_at(seconds);
            let image = tiles.get(cell.tile)?;
            let size = image.size(0);
            let picture = (size.width.0.max(0) as u32, size.height.0.max(0) as u32);
            let crop = thumbnails::crop(trickplay, cell, picture, SHOWN_WIDTH)?;
            Some(Thumb {
                image: image.clone(),
                crop,
                picture: (picture.0 as f32 * crop.scale, picture.1 as f32 * crop.scale),
            })
        });
        self.thumb_fade.set(thumb.is_some(), now);
        if thumb.is_some() {
            self.last = thumb;
        }
        let frame = self.thumbnails.frame();
        Some(Preview {
            opacity,
            left: self.scrub.left(now),
            time: time::clock(seconds),
            frame,
            thumb: self
                .last
                .clone()
                .filter(|_| frame.is_some())
                .map(|thumb| (thumb, self.thumb_fade.level(now))),
        })
    }

    /// The width of the box the preview draws, for keeping it inside the window.
    fn box_width(&self) -> f32 {
        self.thumbnails
            .frame()
            .map_or(TIME_BOX_WIDTH, |frame| frame.0)
    }
}

impl Shell {
    /// The film plays: the server is asked how its thumbnails are laid out. An item without
    /// trickplay, or a server that does not answer, leaves the time alone.
    pub(super) fn load_trickplay(
        &mut self,
        film: ItemId,
        source: MediaSourceId,
        cx: &mut Context<Self>,
    ) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let target = Target {
            film: film.clone(),
            source: source.clone(),
        };
        playback.seek.target = Some(target.clone());
        playback.seek.thumbnails = Thumbnails::Asking;
        self.ask(
            cx,
            move |server| server.trickplay(&film, &source),
            move |shell, answer, cx| shell.trickplay_known(&target, answer, cx),
        );
    }

    fn trickplay_known(
        &mut self,
        target: &Target,
        answer: Result<Option<Trickplay>, ProviderError>,
        cx: &mut Context<Self>,
    ) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        // Another film plays by now.
        if playback.seek.target.as_ref() != Some(target) {
            return;
        }
        playback.seek.thumbnails = match answer {
            Ok(Some(trickplay)) => Thumbnails::Ready(Tiles::new(trickplay, BUDGET)),
            _ => Thumbnails::None,
        };
        self.fetch_tiles(cx);
    }

    /// The pointer is over the seek bar at `x`.
    pub fn seek_hover(&mut self, x: f32, window: &Window, cx: &mut Context<Self>) {
        self.seek_pointer(x, window, cx);
    }

    /// The pointer left the seek bar.
    pub fn seek_left(&mut self, cx: &mut Context<Self>) {
        if let Some(playback) = &mut self.playback {
            playback.seek.scrub.left_bar();
            cx.notify();
        }
    }

    /// A press on the seek bar: the film goes there, and the preview follows the pointer until
    /// the button is let go.
    pub fn seek_press(&mut self, x: f32, window: &Window, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        playback.seek.scrub.press();
        playback
            .session
            .seek_to_fraction(fraction_at(x, f32::from(window.viewport_size().width)));
        self.seek_pointer(x, window, cx);
    }

    /// The mouse moved anywhere in the player: a press that began on the bar goes on along it.
    pub fn seek_drag(
        &mut self,
        x: f32,
        button_down: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        if !playback.seek.scrub.dragging() {
            return;
        }
        // The button was let go where no one heard it.
        if !button_down {
            playback.seek.scrub.release();
            cx.notify();
            return;
        }
        playback
            .session
            .seek_to_fraction(fraction_at(x, f32::from(window.viewport_size().width)));
        self.seek_pointer(x, window, cx);
    }

    pub fn seek_release(&mut self, cx: &mut Context<Self>) {
        if let Some(playback) = &mut self.playback {
            playback.seek.scrub.release();
            cx.notify();
        }
    }

    fn seek_pointer(&mut self, x: f32, window: &Window, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let window_width = f32::from(window.viewport_size().width);
        let seek = &mut playback.seek;
        let left = scrub::left_edge(x, seek.box_width(), window_width);
        seek.scrub
            .moved(fraction_at(x, window_width), left, Instant::now());
        self.fetch_tiles(cx);
        cx.notify();
    }

    /// Starts the tile fetches that may: the ones near the pointer, while the bar is hovered.
    fn fetch_tiles(&mut self, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let (Some(target), Some(pointer)) = (
            playback.seek.target.clone(),
            playback.seek.pointer_tile(playback.session.duration),
        ) else {
            return;
        };
        let Some(tiles) = playback.seek.thumbnails.tiles_mut() else {
            return;
        };
        let trickplay = *tiles.trickplay();
        let now = Instant::now();
        let asked: Vec<u32> = std::iter::from_fn(|| tiles.next_fetch(Some(pointer), now)).collect();
        for tile in asked {
            let svg = cx.svg_renderer();
            let (film, source) = (target.film.clone(), target.source.clone());
            let target = target.clone();
            self.ask(
                cx,
                move |server| {
                    let bytes = server
                        .trickplay_tile(&film, &source, &trickplay, tile)
                        .ok()?;
                    let format = images::format(&bytes)?;
                    Arc::new(Image::from_bytes(format, bytes))
                        .to_image_data(svg)
                        .ok()
                },
                move |shell, decoded, cx| shell.tile_arrived(&target, tile, decoded, cx),
            );
        }
    }

    fn tile_arrived(
        &mut self,
        target: &Target,
        tile: u32,
        decoded: Option<Arc<RenderImage>>,
        cx: &mut Context<Self>,
    ) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        if playback.seek.target.as_ref() != Some(target) {
            return;
        }
        let pointer = playback.seek.pointer_tile(playback.session.duration);
        if let Some(tiles) = playback.seek.thumbnails.tiles_mut() {
            match decoded {
                Some(image) => {
                    let size = image.size(0);
                    let bytes = (size.width.0 * size.height.0 * 4).max(0) as usize;
                    for gone in tiles.arrived(tile, image, bytes, pointer) {
                        cx.drop_image(gone, None);
                    }
                }
                None => tiles.failed(tile, Instant::now()),
            }
        }
        self.fetch_tiles(cx);
    }

    /// The film is left: its tiles give their pictures back. This runs inside window code, where
    /// GPUI has taken the window out of its list: it is named, or its card would keep them.
    pub(super) fn release_tiles(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tiles) = self
            .playback
            .as_mut()
            .and_then(|playback| playback.seek.thumbnails.tiles_mut())
        {
            for image in tiles.drain() {
                cx.drop_image(image, Some(window));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_runs_from_the_edge_to_the_edge() {
        // A 1440 wide window: the bar is 1360 wide from 40.
        assert_eq!(fraction_at(40., 1440.), 0.);
        assert_eq!(fraction_at(720., 1440.), 0.5);
        assert_eq!(fraction_at(1400., 1440.), 1.);
        assert!(fraction_at(10., 1440.) < 0.);
    }

    #[test]
    fn a_draw_before_anything_is_hovered_has_nothing_to_show() {
        let mut seek = SeekPreview::new();
        assert!(seek.draw(7860., Instant::now()).is_none());
    }

    fn trickplay() -> Trickplay {
        Trickplay {
            width: 320,
            height: 180,
            columns: 10,
            rows: 10,
            count: 660,
            interval: std::time::Duration::from_secs(10),
        }
    }

    /// A preview whose pointer is `fraction` along the bar of a film of 7860 s.
    fn hovered(thumbnails: Thumbnails<Arc<RenderImage>>, fraction: f32, t: Instant) -> SeekPreview {
        let mut seek = SeekPreview::new();
        seek.thumbnails = thumbnails;
        seek.draw(7860., t);
        seek.scrub.moved(fraction, 300., t);
        seek
    }

    #[test]
    fn an_item_without_trickplay_shows_the_time_alone() {
        let t = Instant::now();
        let mut seek = hovered(Thumbnails::None, 0.55, t);
        let preview = seek
            .draw(7860., t + std::time::Duration::from_millis(400))
            .unwrap();
        // 55 % of 2 h 11 min: the time as a clock reads it, and no frame for a picture.
        assert_eq!(preview.time, "1:12:03");
        assert!(preview.frame.is_none() && preview.thumb.is_none());
    }

    #[test]
    fn a_thumbnail_not_loaded_yet_shows_the_time_in_its_empty_frame() {
        let t = Instant::now();
        let ready = Thumbnails::Ready(Tiles::new(trickplay(), BUDGET));
        let mut seek = hovered(ready, 0.5, t);
        let preview = seek
            .draw(7860., t + std::time::Duration::from_millis(400))
            .unwrap();
        assert_eq!(preview.time, "1:05:30");
        // The frame is there so that the box does not jump when the tile comes.
        assert_eq!(preview.frame, Some((240., 135.)));
        assert!(preview.thumb.is_none());
    }

    #[test]
    fn tiles_are_wanted_only_while_the_bar_is_hovered() {
        let t = Instant::now();
        let ready = Thumbnails::Ready(Tiles::new(trickplay(), BUDGET));
        let mut seek = hovered(ready, 0.5, t);
        // Half of 7860 s is 3930 s: the 393rd thumbnail, in the fourth tile (3).
        assert_eq!(seek.pointer_tile(7860.), Some(3));
        seek.scrub.left_bar();
        assert_eq!(seek.pointer_tile(7860.), None);
    }

    #[test]
    fn a_film_without_trickplay_wants_no_tile_at_all() {
        let t = Instant::now();
        let seek = hovered(Thumbnails::None, 0.5, t);
        assert_eq!(seek.pointer_tile(7860.), None);
    }

    #[test]
    fn the_preview_stays_while_the_pointer_is_over_the_bar_without_asking_for_frames() {
        let t = Instant::now();
        let mut seek = hovered(Thumbnails::None, 0.5, t);
        seek.draw(7860., t + std::time::Duration::from_millis(400));
        let later = t + std::time::Duration::from_secs(2);
        assert!(seek.draw(7860., later).is_some());
        assert!(!seek.moving(later));
    }
}
