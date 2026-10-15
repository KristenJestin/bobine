//! The menus of the film page's chips (version, audio, subtitles): one open at a time, right under
//! its chip, closed by a click anywhere else or by Escape before Escape leaves the page.

use gpui_kit::{Bounds, Pixels, Point, point, px};

/// Between the bottom of a chip and the top of its menu.
pub const GAP: f32 = 6.;

/// Where a menu's top left goes, in the window, for a chip drawn at `chip`.
pub fn position(chip: Bounds<Pixels>) -> Point<Pixels> {
    point(chip.left(), chip.bottom() + px(GAP))
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

    #[test]
    fn a_chip_menu_opens_right_under_its_chip() {
        let chip = Bounds::new(point(px(372.), px(640.)), size(px(180.), px(32.)));
        assert_eq!(position(chip), point(px(372.), px(678.)));
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
