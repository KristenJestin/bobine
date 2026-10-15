//! Posters, backdrops and photos: asked of the server only when a screen draws them, decoded off
//! the window's thread, kept in memory up to a budget, the least recently drawn going first.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobine::provider::{ImageKind, ImageRef};
use gpui_kit::{ImageFormat, RenderImage};

/// Decoded images take four bytes a pixel: 256 MB is some 500 posters and a few backdrops.
const BUDGET: usize = 256 << 20;

/// At most this many images are fetched at once: fast scrolling must not queue hundreds of
/// blocking requests on the background pool ahead of a click on a film.
const MAX_FETCHES: usize = 6;
/// An image that failed is not asked again before this.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// The widths images are asked at, whatever the window's size, so one image serves every size.
pub const POSTER: u32 = 300;
pub const PHOTO: u32 = 200;
pub const CARD: u32 = 720;
pub const BACKDROP: u32 = 1920;

/// The least recently used values go first once their sizes add up beyond the budget.
pub struct Lru<K, V> {
    budget: usize,
    used: usize,
    clock: u64,
    entries: HashMap<K, (V, usize, u64)>,
}

impl<K: Hash + Eq + Clone, V> Lru<K, V> {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            used: 0,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.clock += 1;
        let entry = self.entries.get_mut(key)?;
        entry.2 = self.clock;
        Some(&entry.0)
    }

    /// Every value, the cache left empty.
    pub fn drain(&mut self) -> Vec<V> {
        self.used = 0;
        self.entries.drain().map(|(_, (value, ..))| value).collect()
    }

    /// Keeps `value`; gives back what had to go to make room.
    pub fn insert(&mut self, key: K, value: V, size: usize) -> Vec<V> {
        self.clock += 1;
        let mut gone = Vec::new();
        if let Some((old, old_size, _)) = self.entries.remove(&key) {
            self.used -= old_size;
            gone.push(old);
        }
        while self.used + size > self.budget {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            let (value, size, _) = self.entries.remove(&oldest).expect("just found");
            self.used -= size;
            gone.push(value);
        }
        self.used += size;
        self.entries.insert(key, (value, size, self.clock));
        gone
    }
}

/// An image at one width.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    item: String,
    backdrop: bool,
    tag: String,
    width: u32,
}

impl Key {
    pub fn new(image: &ImageRef, width: u32) -> Self {
        Self {
            item: image.item.0.clone(),
            backdrop: image.kind == ImageKind::Backdrop,
            tag: image.tag.clone(),
            width,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }
}

pub struct Images {
    pub cache: Lru<Key, Arc<RenderImage>>,
    /// Which draw of the window it is: an image not drawn since the last one is out of view.
    frame: u64,
    /// Asked of the server and not back yet.
    in_flight: HashSet<Key>,
    /// Drawn and waiting for a free fetch, oldest first, with the draw that last wanted them.
    waiting: VecDeque<Key>,
    wanted: HashMap<Key, (ImageRef, u64)>,
    /// When the server could not give them: not asked again before `RETRY_AFTER`.
    failed: HashMap<Key, Instant>,
}

impl Default for Images {
    fn default() -> Self {
        Self {
            cache: Lru::new(BUDGET),
            frame: 0,
            in_flight: HashSet::new(),
            waiting: VecDeque::new(),
            wanted: HashMap::new(),
            failed: HashMap::new(),
        }
    }
}

impl Images {
    /// The window starts drawing.
    pub fn new_frame(&mut self) {
        self.frame += 1;
    }

    /// `image` is drawn and not in the cache: it waits for a free fetch.
    pub fn want(&mut self, key: &Key, image: &ImageRef, now: Instant) {
        if self.in_flight.contains(key) {
            return;
        }
        if let Some(&at) = self.failed.get(key) {
            if now < at + RETRY_AFTER {
                return;
            }
            self.failed.remove(key);
        }
        match self.wanted.get_mut(key) {
            Some(wanted) => wanted.1 = self.frame,
            None => {
                self.wanted.insert(key.clone(), (image.clone(), self.frame));
                self.waiting.push_back(key.clone());
            }
        }
    }

    /// The next image to fetch, if a fetch is free; those no longer drawn are let go.
    pub fn next_fetch(&mut self) -> Option<(Key, ImageRef)> {
        while self.in_flight.len() < MAX_FETCHES {
            let key = self.waiting.pop_front()?;
            let Some((image, drawn)) = self.wanted.remove(&key) else {
                continue;
            };
            if drawn < self.frame {
                continue;
            }
            self.in_flight.insert(key.clone());
            return Some((key, image));
        }
        None
    }

    /// The fetch of `key` ended, with an image or not.
    pub fn fetched(&mut self, key: &Key, found: bool, now: Instant) {
        self.in_flight.remove(key);
        if !found {
            self.failed.insert(key.clone(), now);
        }
    }
}

/// The format of an image, from its first bytes: the server says it in a header the port drops.
pub fn format(bytes: &[u8]) -> Option<ImageFormat> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some(ImageFormat::Png),
        [0xff, 0xd8, 0xff, ..] => Some(ImageFormat::Jpeg),
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'E',
            b'B',
            b'P',
            ..,
        ] => Some(ImageFormat::Webp),
        [b'G', b'I', b'F', b'8', ..] => Some(ImageFormat::Gif),
        [b'B', b'M', ..] => Some(ImageFormat::Bmp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_image_cache_lets_the_least_recently_drawn_go_beyond_its_budget() {
        let mut lru = Lru::new(10);
        assert!(lru.insert("a", 'a', 4).is_empty());
        assert!(lru.insert("b", 'b', 4).is_empty());
        // "a" drawn again: "b" is now the oldest.
        assert_eq!(lru.get(&"a"), Some(&'a'));
        assert_eq!(lru.insert("c", 'c', 4), ['b']);
        assert_eq!(lru.get(&"b"), None);
        assert_eq!(lru.get(&"a"), Some(&'a'));
        assert_eq!(lru.get(&"c"), Some(&'c'));
    }

    fn image(name: &str) -> (Key, ImageRef) {
        let image = ImageRef {
            item: bobine::provider::ItemId(name.into()),
            kind: ImageKind::Primary,
            tag: "t".into(),
        };
        (Key::new(&image, POSTER), image)
    }

    /// Draws `names`, then starts every fetch that may.
    fn draw(images: &mut Images, names: &[&str], now: Instant) -> Vec<String> {
        images.new_frame();
        for name in names {
            let (key, image) = image(name);
            images.want(&key, &image, now);
        }
        started(images)
    }

    fn started(images: &mut Images) -> Vec<String> {
        std::iter::from_fn(|| images.next_fetch())
            .map(|(_, image)| image.item.0)
            .collect()
    }

    #[test]
    fn a_new_login_gives_back_every_image_to_drop() {
        let mut lru = Lru::new(10);
        lru.insert("a", 'a', 4);
        lru.insert("b", 'b', 4);
        let mut gone = lru.drain();
        gone.sort();
        assert_eq!(gone, ['a', 'b']);
        assert_eq!(lru.get(&"a"), None);
        // The budget is whole again.
        assert!(lru.insert("c", 'c', 10).is_empty());
    }

    #[test]
    fn no_more_than_six_images_are_fetched_at_once() {
        let mut images = Images::default();
        let names: Vec<String> = (0..20).map(|n| format!("film-{n}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let now = Instant::now();
        assert_eq!(draw(&mut images, &names, now).len(), 6);
        assert_eq!(started(&mut images), Vec::<String>::new());
        images.fetched(&image("film-0").0, true, now);
        assert_eq!(started(&mut images), ["film-6"]);
    }

    #[test]
    fn images_scrolled_past_are_skipped_when_their_turn_comes() {
        let mut images = Images::default();
        let now = Instant::now();
        assert_eq!(
            draw(&mut images, &["a", "b", "c", "d", "e", "f", "g", "h"], now).len(),
            6
        );
        // The grid scrolled: only "h" and a new "i" are in view; "g" waited for nothing.
        assert_eq!(draw(&mut images, &["h", "i"], now), Vec::<String>::new());
        images.fetched(&image("a").0, true, now);
        assert_eq!(started(&mut images), ["h"]);
        images.fetched(&image("b").0, true, now);
        assert_eq!(started(&mut images), ["i"]);
    }

    #[test]
    fn an_image_drawn_twice_is_fetched_once() {
        let mut images = Images::default();
        let now = Instant::now();
        assert_eq!(draw(&mut images, &["a", "a"], now), ["a"]);
        // Still in flight at the next draw.
        assert_eq!(draw(&mut images, &["a"], now), Vec::<String>::new());
    }

    #[test]
    fn an_image_that_failed_is_asked_again_only_after_thirty_seconds() {
        let mut images = Images::default();
        let now = Instant::now();
        assert_eq!(draw(&mut images, &["a"], now), ["a"]);
        images.fetched(&image("a").0, false, now);
        // The renders that follow, for half a minute.
        for second in 1..30 {
            let later = now + Duration::from_secs(second);
            assert_eq!(draw(&mut images, &["a"], later), Vec::<String>::new());
        }
        assert_eq!(draw(&mut images, &["a"], now + RETRY_AFTER), ["a"]);
    }

    #[test]
    fn the_format_is_read_from_the_first_bytes() {
        assert_eq!(format(b"\x89PNG\r\n"), Some(ImageFormat::Png));
        assert_eq!(format(&[0xff, 0xd8, 0xff, 0xe0]), Some(ImageFormat::Jpeg));
        assert_eq!(format(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(
            format(&crate::fake::gradient("x", 2, 2)),
            Some(ImageFormat::Bmp)
        );
        assert_eq!(format(b"<html>"), None);
    }
}
