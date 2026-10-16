//! The trickplay thumbnails of the film playing: which tile of them to fetch next, which to keep,
//! and where in a tile's picture a thumbnail is. No server and no window here: the shell fetches
//! and decodes, this decides, and the tests drive it with their own clock.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use bobine::provider::{Cell, ItemId, MediaSourceId, Trickplay};

/// At most this many tiles are fetched at once: a tile is some 300 KB and the viewer only needs
/// the one under the pointer and its neighbours.
const MAX_FETCHES: usize = 2;
/// Tiles are fetched this far on each side of the one under the pointer, nearest first.
const AHEAD: u32 = 2;
/// A tile that failed is asked again after this, when the pointer is still over the bar.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// Decoded tiles take four bytes a pixel, 23 MB for the usual 10 by 10 of 320 by 180: this keeps
/// five or six, the ones nearest the pointer.
pub const BUDGET: usize = 128 << 20;

/// The width a thumbnail is drawn at, whatever the size it was cut at.
pub const SHOWN_WIDTH: f32 = 240.;

/// The tiles of one film: the ones here, the ones on their way, the ones that failed.
pub struct Tiles<V> {
    trickplay: Trickplay,
    budget: usize,
    loaded: HashMap<u32, (V, usize)>,
    used: usize,
    in_flight: HashSet<u32>,
    failed: HashMap<u32, Instant>,
}

impl<V> Tiles<V> {
    pub fn new(trickplay: Trickplay, budget: usize) -> Self {
        Self {
            trickplay,
            budget,
            loaded: HashMap::new(),
            used: 0,
            in_flight: HashSet::new(),
            failed: HashMap::new(),
        }
    }

    pub fn trickplay(&self) -> &Trickplay {
        &self.trickplay
    }

    pub fn get(&self, tile: u32) -> Option<&V> {
        self.loaded.get(&tile).map(|(value, _)| value)
    }

    /// The next tile to fetch, if a fetch is free: the one under the pointer first, then those
    /// around it. Nothing while the bar is not hovered (`pointer` is `None`).
    pub fn next_fetch(&mut self, pointer: Option<u32>, now: Instant) -> Option<u32> {
        let pointer = pointer?;
        if self.in_flight.len() >= MAX_FETCHES {
            return None;
        }
        let tile = (0..=AHEAD)
            .flat_map(|distance| [pointer.checked_add(distance), pointer.checked_sub(distance)])
            .flatten()
            .filter(|tile| *tile < self.trickplay.tiles())
            .find(|tile| {
                !self.loaded.contains_key(tile)
                    && !self.in_flight.contains(tile)
                    && self
                        .failed
                        .get(tile)
                        .is_none_or(|at| now >= *at + RETRY_AFTER)
            })?;
        self.in_flight.insert(tile);
        Some(tile)
    }

    /// A fetched tile is here, `size` bytes of it. Gives back what had to go to make room: the
    /// tiles farthest from the pointer, never the new one.
    pub fn arrived(&mut self, tile: u32, value: V, size: usize, pointer: Option<u32>) -> Vec<V> {
        self.in_flight.remove(&tile);
        self.failed.remove(&tile);
        let mut gone = Vec::new();
        if let Some((old, old_size)) = self.loaded.remove(&tile) {
            self.used -= old_size;
            gone.push(old);
        }
        self.loaded.insert(tile, (value, size));
        self.used += size;
        let around = pointer.unwrap_or(tile);
        while self.used > self.budget {
            let Some(farthest) = self
                .loaded
                .keys()
                .copied()
                .filter(|other| *other != tile)
                .max_by_key(|other| other.abs_diff(around))
            else {
                break;
            };
            let (value, size) = self.loaded.remove(&farthest).expect("just found");
            self.used -= size;
            gone.push(value);
        }
        gone
    }

    /// The fetch of `tile` ended without a tile.
    pub fn failed(&mut self, tile: u32, now: Instant) {
        self.in_flight.remove(&tile);
        self.failed.insert(tile, now);
    }

    /// Every tile held, the cache left empty.
    pub fn drain(&mut self) -> Vec<V> {
        self.used = 0;
        self.loaded.drain().map(|(_, (value, _))| value).collect()
    }
}

/// Where a thumbnail sits in its tile's picture, and how big it shows, in logical pixels: the
/// picture is drawn `scale` times its size, at `-left`, `-top` inside a box of `width` by
/// `height` that clips it to the one thumbnail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crop {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub scale: f32,
}

/// The crop of `cell` for a thumbnail shown `shown_width` wide, or none when the tile's picture
/// (`picture`, in pixels) does not reach that far (a last tile cut short).
pub fn crop(
    trickplay: &Trickplay,
    cell: Cell,
    picture: (u32, u32),
    shown_width: f32,
) -> Option<Crop> {
    if picture.0 < (cell.column + 1) * trickplay.width
        || picture.1 < (cell.row + 1) * trickplay.height
    {
        return None;
    }
    let scale = shown_width / trickplay.width as f32;
    let (width, height) = (
        trickplay.width as f32 * scale,
        trickplay.height as f32 * scale,
    );
    Some(Crop {
        left: cell.column as f32 * width,
        top: cell.row as f32 * height,
        width,
        height,
        scale,
    })
}

/// What the player knows of the thumbnails: asked of the server, there or not.
pub enum Thumbnails<V> {
    /// Nothing asked yet.
    Unknown,
    /// The server is being asked how the thumbnails are laid out.
    Asking,
    /// The server made none for this version, or could not say.
    None,
    Ready(Tiles<V>),
}

impl<V> Thumbnails<V> {
    pub fn tiles(&self) -> Option<&Tiles<V>> {
        match self {
            Self::Ready(tiles) => Some(tiles),
            _ => None,
        }
    }

    pub fn tiles_mut(&mut self) -> Option<&mut Tiles<V>> {
        match self {
            Self::Ready(tiles) => Some(tiles),
            _ => None,
        }
    }

    /// The size of the frame the thumbnail shows in, when the version has trickplay: kept once
    /// known, so the frame does not jump when a tile comes.
    pub fn frame(&self) -> Option<(f32, f32)> {
        let trickplay = self.tiles()?.trickplay();
        Some((
            SHOWN_WIDTH,
            (SHOWN_WIDTH * trickplay.height as f32 / trickplay.width as f32).round(),
        ))
    }
}

/// The film and version the thumbnails are of: an answer for another one is not for this
/// playback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub film: ItemId,
    pub source: MediaSourceId,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 660 thumbnails, 10 by 10 to a tile: 7 tiles, 0 to 6.
    fn trickplay() -> Trickplay {
        Trickplay {
            width: 320,
            height: 180,
            columns: 10,
            rows: 10,
            count: 660,
            interval: Duration::from_secs(10),
        }
    }

    fn tiles() -> Tiles<char> {
        Tiles::new(trickplay(), BUDGET)
    }

    fn fetches(tiles: &mut Tiles<char>, pointer: u32, now: Instant) -> Vec<u32> {
        std::iter::from_fn(|| tiles.next_fetch(Some(pointer), now)).collect()
    }

    #[test]
    fn the_tile_under_the_pointer_is_fetched_before_its_neighbours() {
        let (mut tiles, now) = (tiles(), Instant::now());
        assert_eq!(tiles.next_fetch(Some(3), now), Some(3));
        assert_eq!(tiles.next_fetch(Some(3), now), Some(4));
        // Two are on their way: the cap holds the others back.
        assert_eq!(tiles.next_fetch(Some(3), now), None);
        tiles.arrived(3, 'a', 10, Some(3));
        assert_eq!(tiles.next_fetch(Some(3), now), Some(2));
        tiles.arrived(4, 'b', 10, Some(3));
        assert_eq!(tiles.next_fetch(Some(3), now), Some(5));
    }

    #[test]
    fn nothing_is_fetched_while_the_bar_is_not_hovered() {
        let (mut tiles, now) = (tiles(), Instant::now());
        assert_eq!(tiles.next_fetch(None, now), None);
        // And once hovered, it starts.
        assert_eq!(tiles.next_fetch(Some(0), now), Some(0));
    }

    #[test]
    fn no_more_than_two_tiles_are_fetched_at_once() {
        let (mut tiles, now) = (tiles(), Instant::now());
        assert_eq!(fetches(&mut tiles, 3, now).len(), 2);
    }

    #[test]
    fn a_tile_is_fetched_once_while_it_is_here_or_on_its_way() {
        let (mut tiles, now) = (tiles(), Instant::now());
        assert_eq!(fetches(&mut tiles, 0, now), [0, 1]);
        tiles.arrived(0, 'a', 10, Some(0));
        // 1 is still on its way: the next one is 2, never 0 or 1 again.
        assert_eq!(fetches(&mut tiles, 0, now), [2]);
        tiles.arrived(1, 'b', 10, Some(0));
        tiles.arrived(2, 'c', 10, Some(0));
        assert_eq!(fetches(&mut tiles, 0, now), Vec::<u32>::new());
    }

    #[test]
    fn only_the_tiles_around_the_pointer_are_fetched() {
        let (mut tiles, now) = (tiles(), Instant::now());
        let mut all = Vec::new();
        for _ in 0..10 {
            for tile in fetches(&mut tiles, 3, now) {
                tiles.arrived(tile, 'x', 1, Some(3));
                all.push(tile);
            }
        }
        all.sort();
        assert_eq!(all, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn tiles_past_the_last_are_never_asked() {
        let (mut tiles, now) = (tiles(), Instant::now());
        let mut all = Vec::new();
        for _ in 0..10 {
            for tile in fetches(&mut tiles, 6, now) {
                tiles.arrived(tile, 'x', 1, Some(6));
                all.push(tile);
            }
        }
        all.sort();
        assert_eq!(all, [4, 5, 6]);
    }

    #[test]
    fn a_failed_tile_is_asked_again_only_after_a_wait() {
        let (mut tiles, now) = (tiles(), Instant::now());
        assert_eq!(tiles.next_fetch(Some(0), now), Some(0));
        tiles.failed(0, now);
        // The neighbours go on; the failed one waits.
        assert_eq!(fetches(&mut tiles, 0, now), [1, 2]);
        tiles.failed(1, now);
        tiles.failed(2, now);
        assert_eq!(fetches(&mut tiles, 0, now), Vec::<u32>::new());
        let later = now + RETRY_AFTER;
        assert_eq!(tiles.next_fetch(Some(0), later), Some(0));
    }

    #[test]
    fn the_tiles_farthest_from_the_pointer_go_first_beyond_the_budget() {
        let mut tiles: Tiles<char> = Tiles::new(trickplay(), 30);
        assert!(tiles.arrived(0, 'a', 10, Some(0)).is_empty());
        assert!(tiles.arrived(5, 'f', 10, Some(5)).is_empty());
        assert!(tiles.arrived(6, 'g', 10, Some(6)).is_empty());
        // The pointer is at 6: tile 0 is the farthest.
        assert_eq!(tiles.arrived(4, 'e', 10, Some(6)), ['a']);
        assert!(tiles.get(0).is_none());
        assert_eq!(tiles.get(5), Some(&'f'));
        // The tile just arrived stays even when it is the farthest.
        assert_eq!(tiles.arrived(1, 'b', 10, Some(6)), ['e']);
        assert_eq!(tiles.get(1), Some(&'b'));
    }

    #[test]
    fn a_tile_that_is_alone_over_the_budget_is_kept() {
        let mut tiles: Tiles<char> = Tiles::new(trickplay(), 5);
        assert!(tiles.arrived(2, 'c', 10, Some(2)).is_empty());
        assert_eq!(tiles.get(2), Some(&'c'));
    }

    #[test]
    fn a_tile_that_comes_again_replaces_the_old_one() {
        let mut tiles = tiles();
        tiles.arrived(2, 'c', 10, Some(2));
        assert_eq!(tiles.arrived(2, 'd', 10, Some(2)), ['c']);
        assert_eq!(tiles.get(2), Some(&'d'));
    }

    fn cell(tile: u32, column: u32, row: u32) -> Cell {
        Cell { tile, column, row }
    }

    #[test]
    fn a_thumbnail_is_cut_from_its_place_in_the_tile_picture() {
        // Shown at 240 wide, three quarters of 320: a cell is 240 by 135.
        let crop = crop(&trickplay(), cell(4, 2, 3), (3200, 1800), 240.).unwrap();
        assert_eq!(
            crop,
            Crop {
                left: 480.,
                top: 405.,
                width: 240.,
                height: 135.,
                scale: 0.75,
            }
        );
    }

    #[test]
    fn a_thumbnail_the_tile_picture_does_not_reach_is_not_cut() {
        // A last tile whose picture stops after its third row.
        let short = (3200, 540);
        assert!(crop(&trickplay(), cell(6, 4, 2), short, 240.).is_some());
        assert!(crop(&trickplay(), cell(6, 0, 3), short, 240.).is_none());
        assert!(crop(&trickplay(), cell(6, 10, 0), short, 240.).is_none());
    }

    #[test]
    fn the_frame_is_known_as_soon_as_the_layout_is() {
        let mut thumbnails: Thumbnails<char> = Thumbnails::Unknown;
        assert_eq!(thumbnails.frame(), None);
        thumbnails = Thumbnails::Asking;
        assert_eq!(thumbnails.frame(), None);
        thumbnails = Thumbnails::None;
        assert_eq!(thumbnails.frame(), None);
        thumbnails = Thumbnails::Ready(tiles());
        assert_eq!(thumbnails.frame(), Some((240., 135.)));
    }
}
