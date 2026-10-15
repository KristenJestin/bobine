//! The menus of the film page's chips (version, audio, subtitles): one open at a time, right under
//! its chip, closed by a click anywhere else or by Escape before Escape leaves the page.

use gpui_kit::{Bounds, Pixels, Point, point, px};

/// Between a chip and its menu.
pub const GAP: f32 = 6.;
/// Kept free between a menu and the window's edge.
pub const MARGIN: f32 = 8.;
/// One choice of a menu.
pub const LINE: f32 = 34.;

/// The height of a menu of `lines` choices: the lines, the padding and the border around them.
pub fn height(lines: usize) -> f32 {
    lines as f32 * LINE + 2. * 6. + 2.
}

/// Where a menu `height` tall goes, in the window, for a chip drawn at `chip`: its top left, and
/// the height it gets. Under the chip when it fits there, else above it; a menu too long for
/// either side takes the larger one and scrolls.
pub fn place(chip: Bounds<Pixels>, height: f32, window_height: f32) -> (Point<Pixels>, f32) {
    let (top, bottom) = (f32::from(chip.top()), f32::from(chip.bottom()));
    let below = window_height - bottom - GAP - MARGIN;
    let above = top - GAP - MARGIN;
    let under = height <= below || (height > above && below >= above);
    let height = height.min(if under { below } else { above }).max(0.);
    let y = if under {
        bottom + GAP
    } else {
        top - GAP - height
    };
    (point(chip.left(), px(y)), height)
}

pub struct Menus<K> {
    open: Option<K>,
}

impl<K> Default for Menus<K> {
    fn default() -> Self {
        Self { open: None }
    }
}

impl<K: Copy + PartialEq> Menus<K> {
    pub fn open(&self) -> Option<K> {
        self.open
    }

    /// A click on a chip: its menu opens, or closes when it was the one open.
    pub fn toggle(&mut self, chip: K) {
        self.open = (self.open != Some(chip)).then_some(chip);
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// Escape: closes the open menu and says so; with none open, Escape is for the page.
    pub fn escape(&mut self) -> bool {
        self.open.take().is_some()
    }

    /// The mouse went down outside the open menu, on the chip `on` if any. On the menu's own
    /// chip, the chip's click closes it: closing here too would open it again at once.
    pub fn pressed_outside(&mut self, on: Option<K>) {
        if on.is_none() || on != self.open {
            self.open = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::size;

    fn chip_at(top: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(372.), px(top)), size(px(180.), px(32.)))
    }

    #[test]
    fn a_chip_menu_opens_right_under_its_chip() {
        assert_eq!(
            place(chip_at(640.), 80., 810.),
            (point(px(372.), px(678.)), 80.)
        );
    }

    #[test]
    fn a_menu_without_room_below_opens_right_above_its_chip() {
        // 810 - 672 - 6 - 8 = 124 pixels below: too few for 216.
        assert_eq!(
            place(chip_at(640.), 216., 810.),
            (point(px(372.), px(418.)), 216.)
        );
    }

    #[test]
    fn a_menu_too_long_for_either_side_takes_the_larger_one_and_scrolls() {
        // 626 pixels above, 124 below.
        assert_eq!(
            place(chip_at(640.), 700., 810.),
            (point(px(372.), px(8.)), 626.)
        );
        // 46 above, 704 below.
        assert_eq!(
            place(chip_at(60.), 900., 810.),
            (point(px(372.), px(98.)), 704.)
        );
    }

    #[test]
    fn escape_closes_an_open_menu_before_going_back() {
        let mut menus = Menus::default();
        menus.toggle('a');
        assert!(menus.escape());
        assert_eq!(menus.open(), None);
        assert!(!menus.escape());
    }

    #[test]
    fn a_click_outside_closes_the_menu() {
        let mut menus = Menus::default();
        menus.toggle('a');
        menus.pressed_outside(None);
        assert_eq!(menus.open(), None);
    }

    #[test]
    fn a_click_on_the_open_chip_closes_its_menu() {
        let mut menus = Menus::default();
        menus.toggle('a');
        // The press reaches the menu's outside first, then the chip's click.
        menus.pressed_outside(Some('a'));
        menus.toggle('a');
        assert_eq!(menus.open(), None);
    }

    #[test]
    fn a_click_on_another_chip_opens_its_menu_instead() {
        let mut menus = Menus::default();
        menus.toggle('a');
        menus.pressed_outside(Some('b'));
        menus.toggle('b');
        assert_eq!(menus.open(), Some('b'));
    }
}
