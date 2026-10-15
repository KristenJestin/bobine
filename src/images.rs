//! Posters, backdrops and photos: asked of the server only when a screen draws them, decoded off
//! the window's thread, kept in memory up to a budget, the least recently drawn going first.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

use bobine::provider::{ImageKind, ImageRef};
use gpui_kit::{ImageFormat, RenderImage};

/// Decoded images take four bytes a pixel: 256 MB is some 500 posters and a few backdrops.
const BUDGET: usize = 256 << 20;

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
}

pub struct Images {
    pub cache: Lru<Key, Arc<RenderImage>>,
    /// Asked and not back yet.
    pub pending: HashSet<Key>,
    /// Not asked again until the next login.
    pub failed: HashSet<Key>,
}

impl Default for Images {
    fn default() -> Self {
        Self {
            cache: Lru::new(BUDGET),
            pending: HashSet::new(),
            failed: HashSet::new(),
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
