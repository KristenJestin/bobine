//! Trickplay: the small pictures a server cuts a film into, one every few seconds, laid out in
//! tiles (grids of pictures, row by row). The seek bar shows the one under the pointer.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The width the thumbnail is shown at, about: the smallest size that is at least this wide
/// is taken, else the largest there is.
pub const WANTED_WIDTH: u32 = 320;

/// How the thumbnails of one version of a film are laid out, at one size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trickplay {
    /// The size of one thumbnail.
    pub width: u32,
    pub height: u32,
    /// How many thumbnails a tile holds across and down.
    pub columns: u32,
    pub rows: u32,
    /// How many thumbnails the film has in all.
    pub count: u32,
    /// The time between two thumbnails.
    pub interval: Duration,
}

/// Where one thumbnail is: the tile it is in, and its place in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub tile: u32,
    pub column: u32,
    pub row: u32,
}

impl Trickplay {
    /// Of the sizes a server cut a version into, the one to show: the smallest at least
    /// `WANTED_WIDTH` wide, else the widest. Sizes that cannot be read (no thumbnail, no
    /// interval, an empty grid) are left out.
    pub fn choose(sizes: impl IntoIterator<Item = Trickplay>) -> Option<Trickplay> {
        let usable: Vec<Trickplay> = sizes.into_iter().filter(Trickplay::usable).collect();
        usable
            .iter()
            .filter(|size| size.width >= WANTED_WIDTH)
            .min_by_key(|size| size.width)
            .or_else(|| usable.iter().max_by_key(|size| size.width))
            .copied()
    }

    fn usable(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.columns > 0
            && self.rows > 0
            && self.count > 0
            && !self.interval.is_zero()
    }

    fn per_tile(&self) -> u32 {
        self.columns * self.rows
    }

    /// How many tiles hold the thumbnails: the last one is rarely full.
    pub fn tiles(&self) -> u32 {
        self.count.div_ceil(self.per_tile())
    }

    /// How many thumbnails `tile` holds: all of them, but for the last tile.
    pub fn cells_in(&self, tile: u32) -> u32 {
        self.count
            .saturating_sub(tile * self.per_tile())
            .min(self.per_tile())
    }

    /// The thumbnail of the moment `seconds` into the film: the one of the interval that holds
    /// it, the first before the start and the last past the end.
    pub fn cell_at(&self, seconds: f64) -> Cell {
        // `max` also turns NaN into 0.
        let index = (seconds.max(0.) / self.interval.as_secs_f64()) as u64;
        let index = index.min(u64::from(self.count.saturating_sub(1))) as u32;
        let place = index % self.per_tile();
        Cell {
            tile: index / self.per_tile(),
            column: place % self.columns,
            row: place / self.columns,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Jellyfin's usual cut: 320 wide, 10 by 10 to a tile, one every ten seconds; 725
    /// thumbnails, so the film runs a little over two hours and the last tile holds 25.
    fn usual() -> Trickplay {
        sized(320, 725)
    }

    fn sized(width: u32, count: u32) -> Trickplay {
        Trickplay {
            width,
            height: width * 9 / 16,
            columns: 10,
            rows: 10,
            count,
            interval: Duration::from_secs(10),
        }
    }

    fn cell(tile: u32, column: u32, row: u32) -> Cell {
        Cell { tile, column, row }
    }

    #[test]
    fn the_start_of_the_film_is_the_first_cell_of_the_first_tile() {
        assert_eq!(usual().cell_at(0.), cell(0, 0, 0));
        assert_eq!(usual().cell_at(9.9), cell(0, 0, 0));
    }

    #[test]
    fn a_time_picks_the_cell_of_the_interval_that_holds_it() {
        // 1:12:04 is 4324 s: the 432nd interval, in tile 4 at its 32nd place (row 3, column 2).
        assert_eq!(usual().cell_at(4324.), cell(4, 2, 3));
        // The next interval starts at the tenth second.
        assert_eq!(usual().cell_at(10.), cell(0, 1, 0));
        // A row ends at the hundredth second, the tile at the thousandth.
        assert_eq!(usual().cell_at(100.), cell(0, 0, 1));
        assert_eq!(usual().cell_at(1000.), cell(1, 0, 0));
        assert_eq!(usual().cell_at(999.9), cell(0, 9, 9));
    }

    #[test]
    fn the_last_tile_holds_only_what_is_left() {
        let trickplay = usual();
        assert_eq!(trickplay.tiles(), 8);
        assert_eq!(trickplay.cells_in(0), 100);
        assert_eq!(trickplay.cells_in(6), 100);
        assert_eq!(trickplay.cells_in(7), 25);
        assert_eq!(trickplay.cells_in(8), 0);
        // The 725th thumbnail is the 25th of the last tile: row 2, column 4.
        assert_eq!(trickplay.cell_at(7240.), cell(7, 4, 2));
    }

    #[test]
    fn a_full_last_tile_is_not_followed_by_an_empty_one() {
        let trickplay = sized(320, 200);
        assert_eq!(trickplay.tiles(), 2);
        assert_eq!(trickplay.cells_in(1), 100);
    }

    #[test]
    fn a_time_past_the_end_is_the_last_cell_and_one_before_the_start_the_first() {
        let trickplay = usual();
        assert_eq!(trickplay.cell_at(1e9), cell(7, 4, 2));
        assert_eq!(trickplay.cell_at(7250.), cell(7, 4, 2));
        assert_eq!(trickplay.cell_at(-5.), cell(0, 0, 0));
        assert_eq!(trickplay.cell_at(f64::NAN), cell(0, 0, 0));
        assert_eq!(trickplay.cell_at(f64::INFINITY), cell(7, 4, 2));
    }

    #[test]
    fn a_film_of_one_thumbnail_has_only_that_cell() {
        let trickplay = sized(320, 1);
        assert_eq!(trickplay.tiles(), 1);
        assert_eq!(trickplay.cell_at(0.), cell(0, 0, 0));
        assert_eq!(trickplay.cell_at(3600.), cell(0, 0, 0));
    }

    #[test]
    fn the_size_shown_is_the_smallest_that_is_wide_enough() {
        let sizes = [sized(160, 725), sized(640, 725), sized(320, 725)];
        assert_eq!(Trickplay::choose(sizes).unwrap().width, 320);
        let sizes = [sized(640, 725), sized(480, 725)];
        assert_eq!(Trickplay::choose(sizes).unwrap().width, 480);
    }

    #[test]
    fn when_every_size_is_too_small_the_widest_is_shown() {
        let sizes = [sized(120, 725), sized(240, 725)];
        assert_eq!(Trickplay::choose(sizes).unwrap().width, 240);
    }

    #[test]
    fn a_size_that_cannot_be_read_is_left_out() {
        let broken = Trickplay {
            interval: Duration::ZERO,
            ..sized(320, 725)
        };
        let empty = sized(480, 0);
        assert_eq!(Trickplay::choose([broken, empty]), None);
        assert_eq!(
            Trickplay::choose([broken, sized(640, 725)]).unwrap().width,
            640
        );
        assert_eq!(Trickplay::choose([]), None);
    }
}
