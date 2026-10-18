//! Films and episodes kept on disk, to watch away from the server (design #131, section 2).
//!
//! One account's downloads live under `<downloads>/<server id>/<user id>/`, one folder per item:
//!
//! ```text
//! <item id>/
//!   item.json                     the manifest
//!   media.part | media.<container>  the file, while it comes | once complete
//!   subtitles/<index>.<format>
//!   trickplay/<width>/<tile>.jpg
//!   images/<image item id>-<kind>-<width>.jpg
//! ```
//!
//! Every name comes from an id the server gave, never from a title or a URL. The queue is the
//! manifests themselves: one download at a time, in the order queued. `run` does the queued
//! downloads and blocks until none is left; the window calls it on its background executor after
//! each `queue_*` or `resume`, and a call while another runs returns at once.

// Driven by tests until the Downloads page (#134) and the player (#135) use it.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bobine::provider::{
    Episode, FilmDetail, ItemId, MediaServer, MediaSourceId, ProviderError, Segment, SeriesDetail,
    Trickplay,
};
use serde::{Deserialize, Serialize};

/// What must stay free on the disk beyond a download's size.
pub const MARGIN: u64 = 2 << 30;

/// The manifest's format: a change that an older Bobine cannot read raises it.
pub const FORMAT: u32 = 1;

/// The free space of a disk.
pub trait Disk: Send + Sync {
    /// The bytes free to a user on the disk that holds `path`.
    fn free(&self, path: &Path) -> io::Result<u64>;
}

/// The real disk, asked through `statvfs`.
pub struct Statvfs;

impl Disk for Statvfs {
    fn free(&self, _path: &Path) -> io::Result<u64> {
        todo!("#133")
    }
}

/// What a download keeps of its title: what the pages show of it offline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Snapshot {
    Film(Box<FilmDetail>),
    /// An episode, and its series as its page shows it.
    Episode(Box<Episode>, Box<SeriesDetail>),
}

/// Where a download is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Queued,
    Downloading,
    Paused(Halt),
    Done,
    /// What went wrong, in words, without an address.
    Failed(String),
}

/// Why a download is paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Halt {
    /// The viewer paused it.
    Asked,
    /// A write found the disk full: the part is kept.
    DiskFull,
    /// It no longer had room to start: its size and the margin are not free.
    NoRoom,
}

/// An external subtitle of a download, at `subtitles/<index>.<format>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleFile {
    pub index: i32,
    pub format: String,
}

/// `item.json`: what a download is and where it is. Never a URL, a token or a path on the
/// server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    /// Its place in the queue: the lowest goes first.
    pub order: u64,
    pub title: Snapshot,
    /// The version downloaded.
    pub source: MediaSourceId,
    pub size: Option<u64>,
    /// The server's marker of the file: it changes when the file does.
    pub version: String,
    pub container: Option<String>,
    /// What a resume continues the part with: the server's own marker of the bytes.
    pub validator: Option<String>,
    pub subtitles: Vec<SubtitleFile>,
    pub segments: Vec<Segment>,
    /// The layout of the thumbnails kept, when the server made some.
    pub trickplay: Option<Trickplay>,
    pub state: State,
    /// No longer on the server (set by the sync, #136).
    pub gone: bool,
    /// The server holds a newer file (set by the sync, #136).
    pub update: bool,
}

/// A download as the Downloads page lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Download {
    pub item: ItemId,
    pub manifest: Manifest,
    /// How many bytes of the file are on disk.
    pub received: u64,
}

/// Why a download was not queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    Server(ProviderError),
    /// The disk lacks room: `needed` is the size and the margin.
    NoRoom {
        needed: u64,
        free: u64,
    },
    /// The download's folder could not be written.
    Disk(io::ErrorKind),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        todo!("#133")
    }
}

/// One account's downloads, and the queue that brings them.
pub struct Downloads {
    root: PathBuf,
    server: Arc<dyn MediaServer>,
    disk: Arc<dyn Disk>,
    items: BTreeMap<String, Manifest>,
}

impl Downloads {
    /// The downloads of the session's account under `downloads` (`config::downloads_path`): a
    /// download cut by a crash is queued again, a part without a manifest removed.
    pub fn open(
        _downloads: &Path,
        _server: Arc<dyn MediaServer>,
        _disk: Arc<dyn Disk>,
    ) -> io::Result<Downloads> {
        todo!("#133")
    }

    /// The folder of an item's download.
    pub fn folder(&self, _item: &ItemId) -> PathBuf {
        todo!("#133")
    }

    /// Queues one version of a film, when the disk has room for it. Already queued: nothing.
    pub fn queue_film(
        &self,
        _film: &FilmDetail,
        _source: &MediaSourceId,
    ) -> Result<(), DownloadError> {
        todo!("#133")
    }

    /// Queues one version of an episode, like a film.
    pub fn queue_episode(
        &self,
        _episode: &Episode,
        _source: &MediaSourceId,
    ) -> Result<(), DownloadError> {
        todo!("#133")
    }

    /// Queues each episode of a season the server lets download, in order, its first version,
    /// when the disk has room for them all: how many it queued.
    pub fn queue_season(&self, _series: &ItemId, _season: &ItemId) -> Result<usize, DownloadError> {
        todo!("#133")
    }

    /// Does the queued downloads, one at a time, in order, until none is left.
    pub fn run(&self) {
        todo!("#133")
    }

    /// Pauses a download, the one running included: its part and its place are kept.
    pub fn pause(&self, _item: &ItemId) -> io::Result<()> {
        todo!("#133")
    }

    /// Queues a paused or failed download again, at its place.
    pub fn resume(&self, _item: &ItemId) -> io::Result<()> {
        todo!("#133")
    }

    /// Stops a download if it runs and deletes its folder.
    pub fn remove(&self, _item: &ItemId) -> io::Result<()> {
        todo!("#133")
    }

    /// Every download, in the order of the queue.
    pub fn list(&self) -> Vec<Download> {
        todo!("#133")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FakeServer, SERIES, download_bytes};
    use bobine::provider::{SegmentKind, Trickplay};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    /// A disk with as many bytes free as a test says.
    struct FakeDisk(AtomicU64);

    impl Disk for FakeDisk {
        fn free(&self, _path: &Path) -> io::Result<u64> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    fn roomy() -> Arc<FakeDisk> {
        Arc::new(FakeDisk(AtomicU64::new(1 << 40)))
    }

    fn fake() -> FakeServer {
        FakeServer::new(10, true)
    }

    fn open(dir: &Path, server: &FakeServer, disk: Arc<FakeDisk>) -> Downloads {
        Downloads::open(dir, Arc::new(server.clone()), disk).unwrap()
    }

    fn film(server: &FakeServer, id: &str) -> FilmDetail {
        server.film(&ItemId(id.into())).unwrap()
    }

    fn episode(server: &FakeServer, id: &str) -> Episode {
        match server.title(&ItemId(id.into())).unwrap() {
            bobine::provider::Title::Episode(episode) => episode,
            _ => panic!("an episode"),
        }
    }

    fn source(id: &str) -> MediaSourceId {
        MediaSourceId(id.into())
    }

    fn item(id: &str) -> ItemId {
        ItemId(id.into())
    }

    /// The account folder of the fake server's user.
    fn account(dir: &Path) -> PathBuf {
        dir.join("server-1").join("user-1")
    }

    fn manifest(dir: &Path, id: &str) -> Manifest {
        let text = std::fs::read_to_string(account(dir).join(id).join("item.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.extend(files_under(&path));
            } else {
                found.push(path);
            }
        }
        found
    }

    #[test]
    fn downloads_go_under_the_data_dir_by_server_and_user() {
        let data = tempfile::tempdir().unwrap();
        let root =
            crate::config::downloads_path(Some(data.path().into()), Some("/home/ada".into()))
                .unwrap();
        assert_eq!(root, data.path().join("bobine").join("downloads"));
        let server = fake();
        let downloads = open(&root, &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        assert!(
            root.join("server-1/user-1")
                .join(FILM)
                .join("item.json")
                .is_file()
        );
        assert_eq!(downloads.folder(&item(FILM)), account(&root).join(FILM));
        // Another server's account sees none of them.
        let other = open(&root, &server.clone().on_server("server-2"), roomy());
        assert!(other.list().is_empty());
    }

    #[test]
    fn a_download_needs_room_for_its_size_and_a_margin() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let size = 64;
        let disk = Arc::new(FakeDisk(AtomicU64::new(size + MARGIN - 1)));
        let downloads = open(dir.path(), &server, disk.clone());
        let refused = downloads.queue_film(&film(&server, FILM), &source("source-4k"));
        assert_eq!(
            refused,
            Err(DownloadError::NoRoom {
                needed: size + MARGIN,
                free: size + MARGIN - 1,
            })
        );
        assert!(downloads.list().is_empty());
        assert!(!account(dir.path()).join(FILM).exists());

        disk.0.store(size + MARGIN, Ordering::SeqCst);
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        assert_eq!(downloads.list().len(), 1);

        let said = DownloadError::NoRoom {
            needed: 18_400_000_000,
            free: 9_100_000_000,
        };
        assert_eq!(
            said.to_string(),
            "Not enough space: 18.4 GB needed, 9.1 GB free."
        );
    }

    #[test]
    fn a_full_disk_pauses_the_download_and_keeps_the_part() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        server.state.lock().unwrap().disk_full_after = Some(20);
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        downloads.run();
        assert_eq!(
            manifest(dir.path(), FILM).state,
            State::Paused(Halt::DiskFull)
        );
        let part = account(dir.path()).join(FILM).join("media.part");
        assert_eq!(std::fs::read(&part).unwrap().len(), 20);
        assert_eq!(downloads.list()[0].received, 20);

        // Room again: the download goes on from where it stopped.
        downloads.resume(&item(FILM)).unwrap();
        downloads.run();
        let fetches = server.state.lock().unwrap().record.fetches.clone();
        assert_eq!(fetches[1].1, 20);
        assert_eq!(manifest(dir.path(), FILM).state, State::Done);
        assert_eq!(
            std::fs::read(account(dir.path()).join(FILM).join("media.mkv")).unwrap(),
            download_bytes("film-0/source-4k", 64)
        );
    }

    #[test]
    fn a_part_file_resumes_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let crashed = tempfile::tempdir().unwrap();
        let server = fake();
        // Halfway, the disk is copied as a crash would leave it.
        let (from, to) = (dir.path().to_path_buf(), crashed.path().to_path_buf());
        server.state.lock().unwrap().mid_download =
            Some((24, Box::new(move || copy_dir(&from, &to))));
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        downloads.run();
        assert_eq!(manifest(crashed.path(), FILM).state, State::Downloading);

        let again = fake();
        let restarted = open(crashed.path(), &again, roomy());
        assert_eq!(restarted.list()[0].manifest.state, State::Queued);
        assert_eq!(restarted.list()[0].received, 24);
        restarted.run();
        let fetches = again.state.lock().unwrap().record.fetches.clone();
        assert_eq!(fetches[0].1, 24);
        assert_eq!(fetches[0].2.as_deref(), Some("\"v1\""));
        assert_eq!(restarted.list()[0].manifest.state, State::Done);
        assert_eq!(
            std::fs::read(account(crashed.path()).join(FILM).join("media.mkv")).unwrap(),
            download_bytes("film-0/source-4k", 64)
        );
        assert!(
            !account(crashed.path())
                .join(FILM)
                .join("media.part")
                .exists()
        );
    }

    #[test]
    fn an_orphan_part_file_is_removed_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        let kept = account(dir.path()).join(FILM).join("media.part");
        std::fs::write(&kept, b"half").unwrap();
        let orphan = account(dir.path()).join("film-3");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(orphan.join("media.part"), b"half").unwrap();

        let reopened = open(dir.path(), &server, roomy());
        assert!(!orphan.join("media.part").exists());
        assert!(kept.exists());
        assert_eq!(reopened.list().len(), 1);
    }

    #[test]
    fn downloads_run_one_at_a_time_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        server.state.lock().unwrap().fetch_delay = Duration::from_millis(2);
        let downloads = Arc::new(open(dir.path(), &server, roomy()));
        for id in ["film-2", "film-1", "film-3"] {
            downloads
                .queue_film(&film(&server, id), &source("source-1080"))
                .unwrap();
        }
        let runs: Vec<_> = (0..3)
            .map(|_| {
                let downloads = downloads.clone();
                std::thread::spawn(move || downloads.run())
            })
            .collect();
        for run in runs {
            run.join().unwrap();
        }
        let record = &server.state.lock().unwrap().record;
        let handles: Vec<&str> = record.fetches.iter().map(|f| f.0.handle.as_str()).collect();
        assert_eq!(
            handles,
            [
                "film-2/source-1080",
                "film-1/source-1080",
                "film-3/source-1080"
            ]
        );
        assert_eq!(record.most_fetches_at_once, 1);
        assert!(
            downloads
                .list()
                .iter()
                .all(|d| d.manifest.state == State::Done)
        );
    }

    #[test]
    fn a_finished_download_has_its_subtitles_segments_thumbnails_and_posters() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let trickplay = Trickplay {
            width: 320,
            height: 180,
            columns: 2,
            rows: 2,
            count: 6,
            interval: Duration::from_secs(10),
        };
        let segments = vec![Segment {
            kind: SegmentKind::Intro,
            start: Duration::from_secs(5),
            end: Duration::from_secs(65),
        }];
        {
            let mut state = server.state.lock().unwrap();
            state.trickplay = Some(trickplay);
            state.tiles = vec![b"tile-a".to_vec(), b"tile-b".to_vec()];
            state.segments = segments.clone();
        }
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        downloads.run();

        let folder = account(dir.path()).join(FILM);
        let kept = manifest(dir.path(), FILM);
        assert_eq!(kept.state, State::Done);
        assert!(folder.join("media.mkv").is_file());
        // The 4K version has one subtitle file beside it, its fifth track.
        assert_eq!(
            kept.subtitles,
            [SubtitleFile {
                index: 5,
                format: "srt".into()
            }]
        );
        assert!(
            std::fs::read_to_string(folder.join("subtitles/5.srt"))
                .unwrap()
                .contains("Bonjour")
        );
        assert_eq!(kept.segments, segments);
        assert_eq!(kept.trickplay, Some(trickplay));
        assert_eq!(
            std::fs::read(folder.join("trickplay/320/0.jpg")).unwrap(),
            b"tile-a"
        );
        assert_eq!(
            std::fs::read(folder.join("trickplay/320/1.jpg")).unwrap(),
            b"tile-b"
        );
        assert!(folder.join("images/film-0-primary-300.jpg").is_file());
        assert!(folder.join("images/film-0-backdrop-1920.jpg").is_file());
        assert!(folder.join("images/film-0-logo-600.jpg").is_file());
        let Snapshot::Film(detail) = kept.title else {
            panic!("a film")
        };
        assert_eq!(*detail, film(&server, FILM));
    }

    #[test]
    fn a_season_queues_each_of_its_episodes() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let season = item("series-0-s1");
        // Room for two episodes, not three: the season is refused whole.
        let disk = Arc::new(FakeDisk(AtomicU64::new(2 * 64 + MARGIN)));
        let downloads = open(dir.path(), &server, disk.clone());
        assert_eq!(
            downloads.queue_season(&item(SERIES), &season),
            Err(DownloadError::NoRoom {
                needed: 3 * 64 + MARGIN,
                free: 2 * 64 + MARGIN,
            })
        );
        assert!(downloads.list().is_empty());

        disk.0.store(3 * 64 + MARGIN, Ordering::SeqCst);
        assert_eq!(downloads.queue_season(&item(SERIES), &season), Ok(3));
        let queued: Vec<String> = downloads.list().into_iter().map(|d| d.item.0).collect();
        assert_eq!(
            queued,
            ["series-0-s1-e1", "series-0-s1-e2", "series-0-s1-e3"]
        );
        downloads.run();
        for id in &queued {
            let kept = manifest(dir.path(), id);
            assert_eq!(kept.state, State::Done);
            let Snapshot::Episode(episode, series) = kept.title else {
                panic!("an episode")
            };
            assert_eq!(&episode.item.id.0, id);
            assert_eq!(series.series.id, item(SERIES));
        }
        // Queued again, nothing is downloaded twice.
        assert_eq!(downloads.queue_season(&item(SERIES), &season), Ok(0));
    }

    #[test]
    fn the_manifest_holds_no_url_and_no_token() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        server.state.lock().unwrap().trickplay = Some(Trickplay {
            width: 320,
            height: 180,
            columns: 2,
            rows: 2,
            count: 4,
            interval: Duration::from_secs(10),
        });
        server.state.lock().unwrap().tiles = vec![b"tile".to_vec()];
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        downloads
            .queue_episode(&episode(&server, "series-0-s1-e1"), &source("source-1080"))
            .unwrap();
        downloads.run();
        let files = files_under(dir.path());
        assert!(files.iter().any(|f| f.ends_with("item.json")));
        for file in files {
            let bytes = std::fs::read(&file).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for secret in [
                "://",
                "example.org",
                "nowhere",
                "ApiKey",
                crate::fake::PASSWORD,
            ] {
                assert!(!text.contains(secret), "{} holds {secret}", file.display());
            }
        }
    }

    #[test]
    fn removing_a_download_deletes_its_folder() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        downloads
            .queue_film(&film(&server, "film-1"), &source("source-1080"))
            .unwrap();
        downloads.run();
        downloads.remove(&item(FILM)).unwrap();
        assert!(!account(dir.path()).join(FILM).exists());
        assert!(account(dir.path()).join("film-1").exists());
        let left: Vec<ItemId> = downloads.list().into_iter().map(|d| d.item).collect();
        assert_eq!(left, [item("film-1")]);
        assert_eq!(open(dir.path(), &server, roomy()).list().len(), 1);
    }

    #[test]
    fn removing_a_running_download_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        server.state.lock().unwrap().fetch_delay = Duration::from_millis(5);
        let downloads = Arc::new(open(dir.path(), &server, roomy()));
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        let running = {
            let downloads = downloads.clone();
            std::thread::spawn(move || downloads.run())
        };
        while downloads.list()[0].received == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        downloads.remove(&item(FILM)).unwrap();
        running.join().unwrap();
        assert!(!account(dir.path()).join(FILM).exists());
        assert!(downloads.list().is_empty());
    }
}
