//! When the controls show over the picture, and what a key does. No window here: the view reads
//! this and the tests drive it with their own clock.

use std::time::{Duration, Instant};

/// Without a mouse movement for this long, the controls and the cursor go.
pub const HIDE_AFTER: Duration = Duration::from_secs(3);
/// What the arrows and the `−10 s` / `+10 s` buttons skip.
pub const SKIP_SECONDS: f64 = 10.;

/// Nothing over the picture until the mouse moves.
#[derive(Debug, Default)]
pub struct Controls {
    last_move: Option<Instant>,
}

impl Controls {
    pub fn mouse_moved(&mut self, now: Instant) {
        self.last_move = Some(now);
    }

    pub fn visible(&self, now: Instant) -> bool {
        self.last_move
            .is_some_and(|moved| now.saturating_duration_since(moved) < HIDE_AFTER)
    }
}

/// How high the bottom controls reach from the bottom of the window, in logical pixels: the
/// margin (40), the seek bar (15), the gap (14) and the buttons (52) of `screens::player`.
pub const BOTTOM_CONTROLS_HEIGHT: f32 = 121.;

/// mpv's `sub-pos` for a window `window_height` high: 100 is the subtitles' own place, above
/// the bottom; while the controls show, they rise by the controls' height.
pub fn subtitle_position(window_height: f32, controls_shown: bool) -> f64 {
    // Before the first layout there is no height to rise in.
    if !controls_shown || window_height <= 0. {
        return 100.;
    }
    let lifted = 100. * f64::from(BOTTOM_CONTROLS_HEIGHT) / f64::from(window_height);
    (100. - lifted).round().max(0.)
}

/// How far down from the top of the window the title bar reaches, with its buttons.
pub const TOP_CONTROLS_HEIGHT: f32 = 96.;

/// Whether a click `y` pixels from the top of a window `window_height` high falls on the
/// picture: anywhere while the controls hide, between the two bars while they show.
pub fn on_picture(y: f32, window_height: f32, controls_shown: bool) -> bool {
    !controls_shown || (TOP_CONTROLS_HEIGHT..window_height - BOTTOM_CONTROLS_HEIGHT).contains(&y)
}

/// What a key of the player does; the keys themselves are in the keymap (`crate::keys`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyAction {
    TogglePause,
    SeekBy(f64),
    /// Louder by this many points, quieter when negative.
    Volume(f64),
    ToggleMute,
    ToggleFullscreen,
    CycleSubtitles,
    CycleAudio,
    /// The next episode, or the next title of the playlist.
    Next,
    /// The skip button, while it shows.
    PressSkip,
    /// Later by tenths of a second, earlier when negative.
    ShiftSubtitles(i32),
    /// One speed faster, slower when negative.
    Speed(i32),
    /// Leaves fullscreen first, then the player.
    Escape,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, seconds: f64) -> Instant {
        start + Duration::from_secs_f64(seconds)
    }

    #[test]
    fn nothing_is_over_the_picture_until_the_mouse_moves() {
        let start = Instant::now();
        let controls = Controls::default();
        assert!(!controls.visible(start));
        assert!(!controls.visible(at(start, 60.)));
    }

    #[test]
    fn the_controls_show_when_the_mouse_moves() {
        let start = Instant::now();
        let mut controls = Controls::default();
        controls.mouse_moved(start);
        assert!(controls.visible(start));
        assert!(controls.visible(at(start, 2.9)));
    }

    #[test]
    fn the_controls_hide_after_three_seconds_without_movement() {
        let start = Instant::now();
        let mut controls = Controls::default();
        controls.mouse_moved(start);
        assert!(!controls.visible(at(start, 3.)));
    }

    #[test]
    fn each_movement_restarts_the_three_seconds() {
        let start = Instant::now();
        let mut controls = Controls::default();
        controls.mouse_moved(start);
        controls.mouse_moved(at(start, 2.));
        assert!(controls.visible(at(start, 4.5)));
        assert!(!controls.visible(at(start, 5.)));
    }

    #[test]
    fn a_click_between_the_bars_is_on_the_picture() {
        assert!(on_picture(400., 810., true));
        assert!(!on_picture(50., 810., true), "the title bar");
        assert!(!on_picture(750., 810., true), "the bottom controls");
    }

    #[test]
    fn with_the_controls_hidden_the_whole_window_is_the_picture() {
        assert!(on_picture(50., 810., false));
        assert!(on_picture(750., 810., false));
    }

    #[test]
    fn the_subtitles_keep_their_place_while_the_controls_hide() {
        assert_eq!(subtitle_position(810., false), 100.);
        assert_eq!(subtitle_position(1080., false), 100.);
    }

    #[test]
    fn the_subtitles_rise_above_the_shown_controls() {
        // 121 px of controls: 15 % of 810, 11 % of 1080.
        assert_eq!(subtitle_position(810., true), 85.);
        assert_eq!(subtitle_position(1080., true), 89.);
    }

    #[test]
    fn in_a_tiny_window_the_subtitles_go_no_higher_than_the_top() {
        assert_eq!(subtitle_position(100., true), 0.);
        assert_eq!(subtitle_position(0., true), 100.);
    }
}
