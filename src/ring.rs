//! The focus ring of the keyboard: which card the arrows go to, read from where the cards were
//! drawn, so that rows, grids and pages of any shape follow the same rule. No window here.

use gpui_kit::{Bounds, ElementId, Pixels};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub fn horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// Whether it goes towards the end of its axis (right, down).
    pub fn forward(self) -> bool {
        matches!(self, Self::Right | Self::Down)
    }
}

/// The card the ring is on, and whether it shows: only after a key, until the mouse moves.
#[derive(Default)]
pub struct Ring {
    card: Option<ElementId>,
    shown: bool,
    /// An arrow found no card where the cards were drawn: the page moved on for it, and it is
    /// tried again once the cards moved are drawn.
    pub pending: Option<Direction>,
}

impl Ring {
    /// Whether the ring shows on the card `id`.
    pub fn on(&self, id: &ElementId) -> bool {
        self.shown && self.card.as_ref() == Some(id)
    }

    /// The card the ring shows on.
    pub fn shown_on(&self) -> Option<&ElementId> {
        self.card.as_ref().filter(|_| self.shown)
    }

    /// The card the ring was last on, shown or not.
    pub fn card(&self) -> Option<&ElementId> {
        self.card.as_ref()
    }

    pub fn show_on(&mut self, id: ElementId) {
        self.card = Some(id);
        self.shown = true;
    }

    /// The mouse moved; whether the ring was showing.
    pub fn hide(&mut self) -> bool {
        self.pending = None;
        std::mem::replace(&mut self.shown, false)
    }
}

/// Cards whose tops are this close stand on the same line.
const SAME_LINE: f32 = 8.;

/// A card's place: left, top, right, bottom, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Place {
    pub fn of(bounds: Bounds<Pixels>) -> Self {
        Self {
            left: f32::from(bounds.left()),
            top: f32::from(bounds.top()),
            right: f32::from(bounds.right()),
            bottom: f32::from(bounds.bottom()),
        }
    }

    fn center(self) -> (f32, f32) {
        ((self.left + self.right) / 2., (self.top + self.bottom) / 2.)
    }

    fn start(self, horizontal: bool) -> f32 {
        if horizontal { self.left } else { self.top }
    }

    fn end(self, horizontal: bool) -> f32 {
        if horizontal { self.right } else { self.bottom }
    }
}

/// The card `direction` goes to from `from`: along its line for Left and Right, on the nearest
/// line above or below, the nearest across, for Up and Down. `None` at an edge.
pub fn next<K: Clone>(from: Place, cards: &[(K, Place)], direction: Direction) -> Option<K> {
    let x = from.center().0;
    let across = |place: &Place| (place.center().0 - x).abs();
    let pick = |candidates: Vec<&(K, Place)>, distance: &dyn Fn(&Place) -> f32| {
        candidates
            .into_iter()
            // The cards come in no set order: as near, the leftmost, then the highest.
            .min_by(|a, b| {
                (distance(&a.1).total_cmp(&distance(&b.1)))
                    .then(a.1.left.total_cmp(&b.1.left))
                    .then(a.1.top.total_cmp(&b.1.top))
            })
            .map(|(key, _)| key.clone())
    };
    match direction {
        Direction::Left | Direction::Right => {
            let right = direction == Direction::Right;
            let candidates = cards
                .iter()
                .filter(|(_, place)| (from.top..=from.bottom).contains(&place.center().1))
                .filter(|(_, place)| {
                    let cx = place.center().0;
                    if right { cx > x + 1. } else { cx < x - 1. }
                })
                .collect();
            pick(candidates, &across)
        }
        Direction::Down => {
            let below: Vec<_> = cards
                .iter()
                .filter(|(_, place)| place.center().1 > from.bottom)
                .collect();
            let line = below
                .iter()
                .map(|(_, place)| place.top)
                .min_by(f32::total_cmp)?;
            let on_line = below
                .into_iter()
                .filter(|(_, place)| place.top - line < SAME_LINE)
                .collect();
            pick(on_line, &across)
        }
        Direction::Up => {
            let above: Vec<_> = cards
                .iter()
                .filter(|(_, place)| place.center().1 < from.top)
                .collect();
            let line = above
                .iter()
                .map(|(_, place)| place.bottom)
                .max_by(f32::total_cmp)?;
            let on_line = above
                .into_iter()
                .filter(|(_, place)| line - place.bottom < SAME_LINE)
                .collect();
            pick(on_line, &across)
        }
    }
}

/// The card the ring starts on: the first in view, top line first, then from the left.
pub fn first<K: Clone>(cards: &[(K, Place)], view: Place) -> Option<K> {
    cards
        .iter()
        .filter(|(_, place)| {
            let (x, y) = place.center();
            (view.left..view.right).contains(&x) && (view.top..view.bottom).contains(&y)
        })
        .min_by(|a, b| {
            let line = |place: &Place| (place.top / SAME_LINE).round();
            line(&a.1)
                .total_cmp(&line(&b.1))
                .then(a.1.left.total_cmp(&b.1.left))
        })
        .map(|(key, _)| key.clone())
}

/// How far a scroll offset moves (more negative goes on) so that `card` is inside `area`,
/// `room` from its start and `end_room` from its end, along one axis. 0 when it already is.
pub fn into_view(card: Place, area: Place, horizontal: bool, room: f32, end_room: f32) -> f32 {
    let (start, end) = (card.start(horizontal), card.end(horizontal));
    let (low, high) = (
        area.start(horizontal) + room,
        area.end(horizontal) - end_room,
    );
    if start < low {
        low - start
    } else if end > high {
        // A card taller than the room shows its start.
        (high - end).max(low - start)
    } else {
        0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(left: f32, top: f32) -> Place {
        Place {
            left,
            top,
            right: left + 100.,
            bottom: top + 150.,
        }
    }

    /// Two rows of four, the second scrolled sideways by half a card.
    fn rows() -> Vec<(&'static str, Place)> {
        vec![
            ("a0", at(40., 100.)),
            ("a1", at(156., 100.)),
            ("a2", at(272., 100.)),
            ("a3", at(388., 100.)),
            ("b0", at(-10., 300.)),
            ("b1", at(106., 300.)),
            ("b2", at(222., 300.)),
            ("b3", at(338., 300.)),
        ]
    }

    #[test]
    fn two_cards_as_near_go_to_the_left_one_whatever_their_order() {
        // From the middle of a card, two below it half a card to either side.
        let from = at(100., 100.);
        let mut cards = vec![("right", at(150., 300.)), ("left", at(50., 300.))];
        assert_eq!(next(from, &cards, Direction::Down), Some("left"));
        cards.reverse();
        assert_eq!(next(from, &cards, Direction::Down), Some("left"));
    }

    #[test]
    fn the_arrows_move_along_a_row_and_stop_at_its_ends() {
        let cards = rows();
        assert_eq!(next(at(156., 100.), &cards, Direction::Right), Some("a2"));
        assert_eq!(next(at(156., 100.), &cards, Direction::Left), Some("a0"));
        assert_eq!(next(at(388., 100.), &cards, Direction::Right), None);
        assert_eq!(next(at(40., 100.), &cards, Direction::Left), None);
    }

    #[test]
    fn up_and_down_go_to_the_nearest_card_of_the_next_row() {
        let cards = rows();
        // a1's middle is at 206: b1's is 50 off, b2's 66.
        assert_eq!(next(at(156., 100.), &cards, Direction::Down), Some("b1"));
        assert_eq!(next(at(222., 300.), &cards, Direction::Up), Some("a2"));
        assert_eq!(next(at(40., 100.), &cards, Direction::Up), None);
        assert_eq!(next(at(-10., 300.), &cards, Direction::Down), None);
    }

    #[test]
    fn down_in_a_grid_goes_to_the_line_under_and_not_beyond() {
        let cards = vec![
            ("0", at(40., 100.)),
            ("1", at(156., 100.)),
            ("2", at(40., 270.)),
            ("3", at(156., 270.)),
            ("4", at(40., 440.)),
        ];
        assert_eq!(next(at(156., 100.), &cards, Direction::Down), Some("3"));
        assert_eq!(next(at(156., 270.), &cards, Direction::Down), Some("4"));
    }

    #[test]
    fn the_ring_starts_on_the_first_card_in_view() {
        let cards = rows();
        let view = Place {
            left: 0.,
            top: 0.,
            right: 1000.,
            bottom: 800.,
        };
        assert_eq!(first(&cards, view), Some("a0"));
        let lower = Place { top: 260., ..view };
        assert_eq!(first(&cards, lower), Some("b0"));
        let narrower = Place { left: 100., ..view };
        assert_eq!(first(&cards, narrower), Some("a1"), "a0's middle is off it");
    }

    #[test]
    fn a_card_past_the_end_of_its_row_is_scrolled_into_view() {
        let row = Place {
            left: 0.,
            top: 100.,
            right: 1000.,
            bottom: 250.,
        };
        assert_eq!(into_view(at(950., 100.), row, true, 40., 40.), -90.);
        assert_eq!(into_view(at(-60., 100.), row, true, 40., 40.), 100.);
        assert_eq!(into_view(at(400., 100.), row, true, 40., 40.), 0.);
    }

    #[test]
    fn a_card_under_the_bar_or_below_the_window_is_scrolled_into_view() {
        let page = Place {
            left: 0.,
            top: 0.,
            right: 1000.,
            bottom: 800.,
        };
        assert_eq!(into_view(at(40., 50.), page, false, 104., 24.), 54.);
        assert_eq!(into_view(at(40., 700.), page, false, 104., 24.), -74.);
    }
}
