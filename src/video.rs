//! The display port: the surface the video is drawn into, under the window, and where it goes.
//! On Wayland it is a subsurface under GPUI's surface (`subsurface`); in the tests, a fake.

pub mod blackout;
mod gl;
pub mod subsurface;

/// Where the video subsurface goes: its rectangle in the window's surface, in logical pixels
/// (the subsurface position and the viewport destination), and the size of the buffer mpv draws
/// into, in physical pixels (the viewport maps one onto the other, so the picture stays sharp at
/// any scale).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub buffer_width: i32,
    pub buffer_height: i32,
}

/// A rectangle in logical pixels: origin and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// The placement of a video drawn in `video`, inside a window of `window` logical pixels shown
/// at `scale`. What falls outside the window is cut, and nothing is ever empty: Wayland refuses
/// a zero-sized viewport.
pub fn placement(window: (f32, f32), video: Rect, scale: f32) -> Placement {
    let left = video.x.clamp(0., window.0);
    let top = video.y.clamp(0., window.1);
    let width = (video.x + video.width).min(window.0) - left;
    let height = (video.y + video.height).min(window.1) - top;
    let pixels = |length: f32| (length.round() as i32).max(1);
    Placement {
        x: left.round() as i32,
        y: top.round() as i32,
        width: pixels(width),
        height: pixels(height),
        buffer_width: pixels(width * scale),
        buffer_height: pixels(height * scale),
    }
}

pub trait VideoDisplay {
    /// Moves and sizes the video. Called on every layout: the same placement twice is cheap.
    fn place(&mut self, placement: Placement);
    /// Hides the pointer over the window, or brings it back. Said again on every update: the
    /// display acts when the wish changes or when it first can.
    fn set_cursor_visible(&mut self, visible: bool);
    /// A frame of the film is on the window. Read at every frame of the window: it waits for
    /// nothing.
    fn picture_shown(&self) -> bool;
}

#[cfg(test)]
pub mod fake {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    /// Records where the video was put and whether the cursor shows.
    #[derive(Default)]
    pub struct FakeDisplay {
        pub placements: Rc<RefCell<Vec<Placement>>>,
        pub cursor: Rc<RefCell<Vec<bool>>>,
    }

    impl VideoDisplay for FakeDisplay {
        fn place(&mut self, placement: Placement) {
            self.placements.borrow_mut().push(placement);
        }
        fn set_cursor_visible(&mut self, visible: bool) {
            self.cursor.borrow_mut().push(visible);
        }
        fn picture_shown(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(window: (f32, f32)) -> Rect {
        Rect {
            x: 0.,
            y: 0.,
            width: window.0,
            height: window.1,
        }
    }

    #[test]
    fn a_windowed_player_fills_the_window() {
        assert_eq!(
            placement((1440., 810.), whole((1440., 810.)), 1.),
            Placement {
                x: 0,
                y: 0,
                width: 1440,
                height: 810,
                buffer_width: 1440,
                buffer_height: 810,
            }
        );
    }

    #[test]
    fn a_resized_window_resizes_the_video() {
        let p = placement((1000., 600.), whole((1000., 600.)), 1.);
        assert_eq!((p.width, p.height), (1000, 600));
        assert_eq!((p.buffer_width, p.buffer_height), (1000, 600));
    }

    #[test]
    fn a_fullscreen_window_gets_a_fullscreen_video() {
        let p = placement((1920., 1080.), whole((1920., 1080.)), 1.);
        assert_eq!((p.x, p.y, p.width, p.height), (0, 0, 1920, 1080));
        assert_eq!((p.buffer_width, p.buffer_height), (1920, 1080));
    }

    #[test]
    fn at_scale_two_the_buffer_has_twice_the_pixels_of_the_viewport() {
        let p = placement((1920., 1080.), whole((1920., 1080.)), 2.);
        assert_eq!((p.width, p.height), (1920, 1080));
        assert_eq!((p.buffer_width, p.buffer_height), (3840, 2160));
    }

    #[test]
    fn a_fractional_scale_rounds_the_buffer() {
        let p = placement((1280., 720.), whole((1280., 720.)), 1.25);
        assert_eq!((p.buffer_width, p.buffer_height), (1600, 900));
    }

    #[test]
    fn a_video_inside_the_window_keeps_its_position() {
        let video = Rect {
            x: 10.4,
            y: 48.,
            width: 800.,
            height: 450.,
        };
        let p = placement((1440., 810.), video, 1.);
        assert_eq!((p.x, p.y, p.width, p.height), (10, 48, 800, 450));
    }

    #[test]
    fn what_falls_outside_the_window_is_cut() {
        let video = Rect {
            x: 400.,
            y: 0.,
            width: 2000.,
            height: 810.,
        };
        let p = placement((1440., 810.), video, 2.);
        assert_eq!((p.x, p.width), (400, 1040));
        assert_eq!(p.buffer_width, 2080);
    }

    #[test]
    fn the_video_is_never_empty() {
        let p = placement((0., 0.), whole((0., 0.)), 1.);
        assert_eq!((p.width, p.height), (1, 1));
        assert_eq!((p.buffer_width, p.buffer_height), (1, 1));
    }
}
