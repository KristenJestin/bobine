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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyAction {
    TogglePause,
    SeekBy(f64),
    ToggleFullscreen,
    ExitFullscreen,
    Leave,
}

/// The action of a key, by GPUI's name for it. Escape leaves fullscreen first, then the player.
pub fn key_action(key: &str, fullscreen: bool) -> Option<KeyAction> {
    match key {
        "space" => Some(KeyAction::TogglePause),
        "left" => Some(KeyAction::SeekBy(-SKIP_SECONDS)),
        "right" => Some(KeyAction::SeekBy(SKIP_SECONDS)),
        "f" => Some(KeyAction::ToggleFullscreen),
        "escape" if fullscreen => Some(KeyAction::ExitFullscreen),
        "escape" => Some(KeyAction::Leave),
        _ => None,
    }
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
    fn space_plays_and_pauses() {
        assert_eq!(key_action("space", false), Some(KeyAction::TogglePause));
    }

    #[test]
    fn left_and_right_skip_ten_seconds() {
        assert_eq!(key_action("left", false), Some(KeyAction::SeekBy(-10.)));
        assert_eq!(key_action("right", true), Some(KeyAction::SeekBy(10.)));
    }

    #[test]
    fn f_toggles_fullscreen() {
        assert_eq!(key_action("f", false), Some(KeyAction::ToggleFullscreen));
        assert_eq!(key_action("f", true), Some(KeyAction::ToggleFullscreen));
    }

    #[test]
    fn escape_leaves_fullscreen_first_then_the_player() {
        assert_eq!(key_action("escape", true), Some(KeyAction::ExitFullscreen));
        assert_eq!(key_action("escape", false), Some(KeyAction::Leave));
    }

    #[test]
    fn other_keys_do_nothing() {
        assert_eq!(key_action("a", false), None);
        assert_eq!(key_action("enter", false), None);
    }
}
