//! The desktop's media controls (MPRIS): what plays, shown to the bar widget, and the media keys
//! and the widget's buttons heard back. The D-Bus side is an adapter behind the `MediaControls`
//! port (`dbus`); the tests use a fake.

pub mod dbus;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bobine::provider::{Episode, Film, ImageRef};

/// A jump of the position beyond what playing explains, in seconds: a seek.
const SEEK_TOLERANCE: f64 = 1.;

/// How long mpv has to answer a seek the desktop asked for.
const SEEK_ANSWER: Duration = Duration::from_secs(2);

/// What the clock and mpv's own rounding explain, in seconds: below it a position is where
/// playing puts it.
const DRIFT: f64 = 0.01;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Playing,
    Paused,
    Stopped,
}

/// What the widget shows of the title that plays. Built from the server's names and a local
/// file only: never from an address, which may carry a token.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The server's id of the item, which the track id is built from.
    pub item: String,
    pub title: String,
    /// The series of an episode: its album and its artist.
    pub series: Option<String>,
    /// The poster, written to the cache; `None` until it is.
    pub art: Option<PathBuf>,
    /// Seconds; 0 while unknown.
    pub length: f64,
}

/// What the desktop is shown.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub status: Status,
    pub track: Option<Track>,
    /// An episode follows the one playing: Next plays it.
    pub can_go_next: bool,
    pub rate: f64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            status: Status::Stopped,
            track: None,
            can_go_next: false,
            rate: 1.,
        }
    }
}

/// What the media keys and the widget's buttons ask.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    PlayPause,
    Play,
    Pause,
    Stop,
    Next,
    /// Seconds from where the film is, forward or back.
    SeekBy(f64),
    /// Seconds from the start, of the track shown (the adapter drops a stale one).
    SetPosition(f64),
}

/// The port: whatever shows the playback to the desktop.
pub trait MediaControls {
    /// What plays now: the adapter tells the desktop what changed since the last call.
    fn show(&mut self, state: &State);
    /// Seconds from the start: read when the desktop asks, never announced.
    fn position(&mut self, seconds: f64);
    /// The film jumped to `seconds`.
    fn seeked(&mut self, seconds: f64);
    /// What was asked since the last call, oldest first. Never waits.
    fn commands(&mut self) -> Vec<Command>;
    /// The window closes: the desktop forgets the player.
    fn close(&mut self);
}

/// Follows the playback for the port: what is shown changes only when it does, and a jump of
/// the position is told as a seek.
pub struct Publisher {
    controls: Box<dyn MediaControls>,
    shown: State,
    /// The position at the last update, when, and whether it was moving then.
    last: Option<(f64, Instant)>,
    /// When the desktop last asked for a seek that no position has answered yet.
    asked: Option<Instant>,
}

impl Publisher {
    pub fn new(controls: Box<dyn MediaControls>) -> Self {
        Self {
            controls,
            shown: State::default(),
            last: None,
            asked: None,
        }
    }

    /// The desktop asked for a seek at `now`: whatever its size, the position that answers it is
    /// told as a seek.
    pub fn sought(&mut self, now: Instant) {
        self.asked = Some(now);
    }

    pub fn follow(&mut self, state: State, position: f64, now: Instant) {
        let same_track =
            state.track.as_ref().map(|t| &t.item) == self.shown.track.as_ref().map(|t| &t.item);
        if let Some((before, at)) = self.last.filter(|_| same_track && state.track.is_some()) {
            let moving = self.shown.status == Status::Playing;
            let expected = if moving {
                before + now.saturating_duration_since(at).as_secs_f64() * self.shown.rate
            } else {
                before
            };
            self.asked = self
                .asked
                .filter(|at| now.saturating_duration_since(*at) < SEEK_ANSWER);
            // The position mpv has not updated yet, still `before`, answers nothing.
            let answered =
                self.asked.is_some() && position != before && (position - expected).abs() > DRIFT;
            if answered || (position - expected).abs() > SEEK_TOLERANCE {
                self.controls.seeked(position);
                self.asked = None;
            }
        }
        if !same_track {
            self.asked = None;
        }
        if state != self.shown {
            self.controls.show(&state);
        }
        self.controls.position(position);
        self.last = state.track.is_some().then_some((position, now));
        self.shown = state;
    }

    /// Nothing plays any more.
    pub fn stop(&mut self) {
        self.follow(State::default(), 0., Instant::now());
    }

    pub fn commands(&mut self) -> Vec<Command> {
        self.controls.commands()
    }

    pub fn close(&mut self) {
        self.controls.close();
    }
}

/// What a playback shows the desktop, and the poster to write for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Title {
    pub track: Track,
    pub poster: Option<ImageRef>,
}

impl Title {
    pub fn film(film: &Film) -> Self {
        Self {
            track: Track {
                item: film.id.0.clone(),
                title: film.title.clone(),
                series: None,
                art: None,
                length: film.runtime.map_or(0., |runtime| runtime.as_secs_f64()),
            },
            poster: film.poster.clone(),
        }
    }

    /// The episode's own title; the series as album and artist, which is what a bar widget
    /// shows beside the title ("artist - title").
    pub fn episode(episode: &Episode) -> Self {
        let mut title = Self::film(&episode.item);
        title.track.series = Some(episode.series_title.clone());
        title
    }
}

/// The MPRIS track id of `item`: an object path, outside `/org/mpris`, whose last element keeps
/// the id's letters and digits and writes every other byte as `_` and two hex digits.
pub fn track_id(item: &str) -> String {
    let mut path = String::from("/org/bobine/track/");
    if item.is_empty() {
        path.push('_');
    }
    for byte in item.bytes() {
        if byte.is_ascii_alphanumeric() {
            path.push(byte as char);
        } else {
            path.push_str(&format!("_{byte:02x}"));
        }
    }
    path
}

/// `$XDG_CACHE_HOME/bobine/mpris`, else `~/.cache/bobine/mpris`. A relative `XDG_CACHE_HOME`
/// is ignored, as the XDG specification says.
pub fn art_dir(xdg_cache_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let cache = xdg_cache_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache.join("bobine").join("mpris"))
}

/// What is safe in a file name of `text`; the rest is written `_`.
fn safe(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The extension of an image by its first bytes, which is what the server sent whatever it was
/// asked for.
fn image_extension(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => "jpg",
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [b'B', b'M', ..] => "bmp",
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
        ] => "webp",
        _ => "img",
    }
}

/// The poster of `item` at image tag `tag`, if it is already written: the tag is in the name, so
/// a new poster of the item is not mistaken for it.
pub fn cached_art(dir: &Path, item: &str, tag: &str) -> Option<PathBuf> {
    let stem = format!("{}.{}.", safe(item), safe(tag));
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(&stem))
                .is_some_and(|extension| !extension.contains('.'))
        })
}

/// Writes the poster's bytes where the desktop reads them, named by item, tag and real format,
/// and drops the item's older posters.
pub fn store_art(dir: &Path, item: &str, tag: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let item = safe(item);
    let path = dir.join(format!("{item}.{}.{}", safe(tag), image_extension(bytes)));
    // Whole or not at all: a half-written file is never taken for the poster.
    let partial = path.with_extension("part");
    std::fs::write(&partial, bytes)?;
    std::fs::rename(&partial, &path)?;
    let own = format!("{item}.");
    for entry in std::fs::read_dir(dir)?.flatten() {
        let older = entry.path();
        let name = entry.file_name();
        if older != path && name.to_str().is_some_and(|name| name.starts_with(&own)) {
            let _ = std::fs::remove_file(older);
        }
    }
    Ok(path)
}

/// `file://` and the path, every byte that is not plain written as `%` and two hex digits.
pub fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

#[cfg(test)]
pub mod fake {
    //! Controls that show nothing: they record what they are given and hand out the commands a
    //! test queued.

    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;

    #[derive(Default, Clone)]
    pub struct FakeControls {
        pub shown: Rc<RefCell<Vec<State>>>,
        pub positions: Rc<RefCell<Vec<f64>>>,
        pub seeked: Rc<RefCell<Vec<f64>>>,
        pub queued: Rc<RefCell<Vec<Command>>>,
        pub closed: Rc<RefCell<bool>>,
    }

    impl FakeControls {
        /// The last state shown.
        pub fn last(&self) -> State {
            self.shown.borrow().last().cloned().unwrap_or_default()
        }
    }

    impl MediaControls for FakeControls {
        fn show(&mut self, state: &State) {
            self.shown.borrow_mut().push(state.clone());
        }
        fn position(&mut self, seconds: f64) {
            self.positions.borrow_mut().push(seconds);
        }
        fn seeked(&mut self, seconds: f64) {
            self.seeked.borrow_mut().push(seconds);
        }
        fn commands(&mut self) -> Vec<Command> {
            std::mem::take(&mut *self.queued.borrow_mut())
        }
        fn close(&mut self) {
            *self.closed.borrow_mut() = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::fake::FakeControls;
    use super::*;

    fn track(item: &str) -> Track {
        Track {
            item: item.into(),
            title: "Le Dernier Hiver".into(),
            series: None,
            art: None,
            length: 7860.,
        }
    }

    fn playing(item: &str) -> State {
        State {
            status: Status::Playing,
            track: Some(track(item)),
            can_go_next: false,
            rate: 1.,
        }
    }

    fn publisher() -> (Publisher, FakeControls) {
        let fake = FakeControls::default();
        (Publisher::new(Box::new(fake.clone())), fake)
    }

    #[test]
    fn the_same_state_is_shown_once() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.follow(playing("film-0"), 10.1, now + Duration::from_millis(100));
        assert_eq!(*fake.shown.borrow(), [playing("film-0")]);
        assert_eq!(*fake.positions.borrow(), [10., 10.1]);
    }

    #[test]
    fn a_status_change_is_shown() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        let paused = State {
            status: Status::Paused,
            ..playing("film-0")
        };
        publisher.follow(paused.clone(), 10., now + Duration::from_millis(100));
        assert_eq!(*fake.shown.borrow(), [playing("film-0"), paused]);
    }

    #[test]
    fn a_jump_of_the_position_is_told_as_a_seek() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.follow(playing("film-0"), 10.1, now + Duration::from_millis(100));
        publisher.follow(playing("film-0"), 70., now + Duration::from_millis(200));
        assert_eq!(*fake.seeked.borrow(), [70.]);
    }

    #[test]
    fn a_small_seek_asked_by_the_desktop_is_told_whatever_its_size() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.sought(now + Duration::from_millis(100));
        // Half a second forward, beyond what playing explains by far less than a second.
        publisher.follow(playing("film-0"), 10.6, now + Duration::from_millis(200));
        assert_eq!(*fake.seeked.borrow(), [10.6]);
    }

    #[test]
    fn a_small_seek_while_paused_is_told() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        let paused = State {
            status: Status::Paused,
            ..playing("film-0")
        };
        publisher.follow(paused.clone(), 10., now);
        publisher.sought(now + Duration::from_millis(100));
        publisher.follow(paused, 9.5, now + Duration::from_millis(200));
        assert_eq!(*fake.seeked.borrow(), [9.5]);
    }

    #[test]
    fn the_position_mpv_has_not_updated_yet_is_no_answer_to_a_seek() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.sought(now + Duration::from_millis(100));
        publisher.follow(playing("film-0"), 10., now + Duration::from_millis(200));
        assert!(fake.seeked.borrow().is_empty());
        publisher.follow(playing("film-0"), 10.9, now + Duration::from_millis(300));
        assert_eq!(*fake.seeked.borrow(), [10.9]);
    }

    #[test]
    fn a_seek_is_told_once_and_drift_after_it_is_not() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.sought(now);
        publisher.follow(playing("film-0"), 10.6, now + Duration::from_millis(100));
        publisher.follow(playing("film-0"), 10.7, now + Duration::from_millis(200));
        publisher.follow(playing("film-0"), 11.2, now + Duration::from_millis(300));
        assert_eq!(*fake.seeked.borrow(), [10.6]);
    }

    #[test]
    fn natural_drift_without_a_request_is_no_seek() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.follow(playing("film-0"), 10.6, now + Duration::from_millis(100));
        assert!(fake.seeked.borrow().is_empty());
    }

    #[test]
    fn a_request_that_nothing_answers_does_not_turn_drift_into_a_seek_later() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("film-0"), 10., now);
        publisher.sought(now);
        // The film just plays on, as a seek to where it already is would leave it.
        publisher.follow(playing("film-0"), 15., now + Duration::from_secs(5));
        publisher.follow(playing("film-0"), 20.6, now + Duration::from_secs(10));
        assert!(fake.seeked.borrow().is_empty());
    }

    #[test]
    fn playing_on_at_twice_the_speed_is_no_seek() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        let fast = State {
            rate: 2.,
            ..playing("film-0")
        };
        publisher.follow(fast.clone(), 10., now);
        publisher.follow(fast, 14., now + Duration::from_secs(2));
        assert!(fake.seeked.borrow().is_empty());
    }

    #[test]
    fn a_jump_while_paused_is_a_seek_and_standing_still_is_not() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        let paused = State {
            status: Status::Paused,
            ..playing("film-0")
        };
        publisher.follow(paused.clone(), 10., now);
        publisher.follow(paused.clone(), 10., now + Duration::from_secs(5));
        assert!(fake.seeked.borrow().is_empty());
        publisher.follow(paused, 0., now + Duration::from_secs(6));
        assert_eq!(*fake.seeked.borrow(), [0.]);
    }

    #[test]
    fn another_track_starting_elsewhere_is_no_seek() {
        let (mut publisher, fake) = publisher();
        let now = Instant::now();
        publisher.follow(playing("episode-1"), 1300., now);
        publisher.follow(playing("episode-2"), 0., now + Duration::from_millis(100));
        assert!(fake.seeked.borrow().is_empty());
    }

    #[test]
    fn stopping_shows_stopped_with_no_track() {
        let (mut publisher, fake) = publisher();
        publisher.follow(playing("film-0"), 10., Instant::now());
        publisher.stop();
        assert_eq!(fake.last(), State::default());
        assert_eq!(fake.last().status, Status::Stopped);
    }

    #[test]
    fn a_track_id_is_an_object_path_whatever_the_id() {
        assert_eq!(track_id("4f2a9c"), "/org/bobine/track/4f2a9c");
        assert_eq!(track_id("film-0"), "/org/bobine/track/film_2d0");
        assert_eq!(track_id(""), "/org/bobine/track/_");
        let path = track_id("a/b c.é");
        assert!(
            zbus::zvariant::ObjectPath::try_from(path.as_str()).is_ok(),
            "{path}"
        );
    }

    #[test]
    fn the_art_goes_under_the_cache_home() {
        assert_eq!(
            art_dir(Some("/tmp/cache".into()), Some("/home/camille".into())),
            Some(PathBuf::from("/tmp/cache/bobine/mpris"))
        );
        assert_eq!(
            art_dir(Some("relative".into()), Some("/home/camille".into())),
            Some(PathBuf::from("/home/camille/.cache/bobine/mpris"))
        );
        assert_eq!(art_dir(None, None), None);
    }

    #[test]
    fn a_poster_is_named_by_item_tag_and_real_format() {
        let dir = tempfile::tempdir().unwrap();
        let png = [0x89, b'P', b'N', b'G', 1, 2];
        let path = store_art(dir.path(), "../film 0", "t:1", &png).unwrap();
        assert_eq!(path, dir.path().join("___film_0.t_1.png"));
        assert_eq!(std::fs::read(&path).unwrap(), png);
        assert_eq!(cached_art(dir.path(), "../film 0", "t:1"), Some(path));
        assert_eq!(cached_art(dir.path(), "../film 0", "t2"), None);
        let webp = *b"RIFF\0\0\0\0WEBPVP8 ";
        let jpg = [0xFF, 0xD8, 0xFF, 0xE0];
        let name = |bytes: &[u8]| {
            let path = store_art(dir.path(), "x", "t", bytes).unwrap();
            path.extension().unwrap().to_str().unwrap().to_string()
        };
        assert_eq!(name(&webp), "webp");
        assert_eq!(name(&jpg), "jpg");
        assert_eq!(name(b"GIF89a"), "gif");
        assert_eq!(name(b"???"), "img");
    }

    #[test]
    fn a_new_poster_replaces_the_item_older_ones_and_no_other_item() {
        let dir = tempfile::tempdir().unwrap();
        let old = store_art(dir.path(), "film-0", "t1", b"BM1").unwrap();
        let other = store_art(dir.path(), "film-0-1", "t1", b"BM2").unwrap();
        let new = store_art(dir.path(), "film-0", "t2", b"BM3").unwrap();
        assert!(!old.exists());
        assert!(new.exists());
        assert!(
            other.exists(),
            "an item whose id starts the same is not touched"
        );
    }

    #[test]
    fn a_file_url_escapes_what_is_not_plain() {
        assert_eq!(
            file_url(Path::new("/home/camille/my cache/a%b.jpg")),
            "file:///home/camille/my%20cache/a%25b.jpg"
        );
    }

    #[test]
    fn an_episode_shows_its_series_as_album_and_artist() {
        let episode = crate::fake::episodes().remove(0);
        let title = Title::episode(&episode);
        assert_eq!(title.track.title, episode.item.title);
        assert_eq!(title.track.series.as_deref(), Some(&*episode.series_title));
        assert_eq!(title.track.item, episode.item.id.0);
        assert_eq!(title.poster, episode.item.poster);
    }

    #[test]
    fn a_film_shows_its_title_and_its_runtime() {
        let film = crate::fake::films(1).remove(0);
        let title = Title::film(&film);
        assert_eq!(title.track.title, film.title);
        assert_eq!(title.track.series, None);
        assert_eq!(
            title.track.length,
            film.runtime.map_or(0., |r| r.as_secs_f64())
        );
    }
}
