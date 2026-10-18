//! Films and episodes kept on disk, to watch away from the server (design #131, section 2).
//!
//! One account's downloads live under `<downloads>/<server id>/<user id>/`, one folder per item:
//!
//! ```text
//! <item id>/
//!   item.json                       the manifest
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
use std::ffi::CString;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use bobine::provider::{
    DownloadKind, DownloadPlan, Episode, Fetched, FilmDetail, ImageKind, ImageRef, ItemId,
    MediaServer, MediaSourceId, ProviderError, Segment, SeriesDetail, Sink, Trailers, Trickplay,
};
use serde::{Deserialize, Serialize};

use crate::images;

/// What must stay free on the disk beyond a download's size.
pub const MARGIN: u64 = 2 << 30;

/// The manifest's format: a change that an older Bobine cannot read raises it.
pub const FORMAT: u32 = 1;

const MANIFEST: &str = "item.json";
const PART: &str = "media.part";

/// The free space of a disk.
pub trait Disk: Send + Sync {
    /// The bytes free to a user on the disk that holds `path`.
    fn free(&self, path: &Path) -> io::Result<u64>;
}

/// The real disk, asked through `statvfs`.
pub struct Statvfs;

impl Disk for Statvfs {
    fn free(&self, path: &Path) -> io::Result<u64> {
        // The folder may not exist yet: its nearest parent that does is on the same disk.
        let existing = path
            .ancestors()
            .find(|path| path.exists())
            .unwrap_or(Path::new("/"));
        let path = CString::new(existing.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: `path` is a C string and `stat` room for the answer, which is read only when
        // the call succeeded.
        let stat = unsafe {
            if libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) != 0 {
                return Err(io::Error::last_os_error());
            }
            stat.assume_init()
        };
        #[allow(clippy::unnecessary_cast)] // Both are not u64 on every target.
        Ok((stat.f_bavail as u64).saturating_mul(stat.f_frsize as u64))
    }
}

/// What a download keeps of its title: what the pages show of it offline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Snapshot {
    Film(Box<FilmDetail>),
    /// An episode, and its series as its page shows it.
    Episode(Box<Episode>, Box<SeriesDetail>),
}

impl Snapshot {
    pub fn item(&self) -> &ItemId {
        match self {
            Snapshot::Film(film) => &film.film.id,
            Snapshot::Episode(episode, _) => &episode.item.id,
        }
    }

    /// The images kept with the title, at the width each page draws them.
    fn images(&self) -> Vec<(ImageRef, u32)> {
        let wanted = match self {
            Snapshot::Film(film) => vec![
                (&film.film.poster, images::POSTER),
                (&film.film.backdrop, images::BACKDROP),
                (&film.logo, images::LOGO),
            ],
            Snapshot::Episode(episode, series) => vec![
                (&episode.item.poster, images::CARD),
                (&series.series.poster, images::POSTER),
                (&series.series.backdrop, images::BACKDROP),
                (&series.logo, images::LOGO),
            ],
        };
        wanted
            .into_iter()
            .filter_map(|(image, width)| Some((image.clone()?, width)))
            .collect()
    }
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

impl Manifest {
    pub fn item(&self) -> &ItemId {
        self.title.item()
    }

    /// The complete file's name in the item's folder.
    pub fn media_name(&self) -> String {
        format!("media.{}", safe(self.container.as_deref().unwrap_or("bin")))
    }
}

/// A subtitle's file in the item's folder.
pub fn subtitle_name(subtitle: &SubtitleFile) -> PathBuf {
    Path::new("subtitles").join(format!("{}.{}", subtitle.index, safe(&subtitle.format)))
}

/// A trickplay tile's file in the item's folder.
pub fn tile_name(trickplay: &Trickplay, tile: u32) -> PathBuf {
    Path::new("trickplay")
        .join(trickplay.width.to_string())
        .join(format!("{tile}.jpg"))
}

/// An image's file in the item's folder.
pub fn image_name(image: &ImageRef, width: u32) -> PathBuf {
    let kind = match image.kind {
        ImageKind::Primary => "primary",
        ImageKind::Backdrop => "backdrop",
        ImageKind::Logo => "logo",
    };
    Path::new("images").join(format!("{}-{kind}-{width}.jpg", safe(&image.item.0)))
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let gb = |bytes: u64| format!("{:.1} GB", bytes as f64 / 1e9);
        match self {
            Self::Server(error) => error.fmt(f),
            Self::NoRoom { needed, free } => write!(
                f,
                "Not enough space: {} needed, {} free.",
                gb(*needed),
                gb(*free)
            ),
            Self::Disk(kind) => write!(f, "The download cannot be written to disk: {kind}"),
        }
    }
}

/// An id as a file name: letters, digits, `-` and `_` kept, every other byte as `%XX`, so that
/// no id names a folder above it. An empty id is `%`, which no other id gives, so that it never
/// names the folder it sits in.
fn safe(id: &str) -> String {
    if id.is_empty() {
        return "%".to_string();
    }
    id.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' => (byte as char).to_string(),
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/// The folder's name is one `safe` can give: nothing else in the account folder is ours to remove.
fn looks_like_an_item(folder: &Path) -> bool {
    let is_link = fs::symlink_metadata(folder).is_ok_and(|meta| meta.file_type().is_symlink());
    !is_link
        && folder.file_name().is_some_and(|name| {
            let name = name.as_bytes();
            !name.is_empty()
                && name
                    .iter()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'%'))
        })
}

/// Writes the manifest through a temporary file, so a crash leaves the old one.
fn save(folder: &Path, manifest: &Manifest) -> io::Result<()> {
    fs::create_dir_all(folder)?;
    let text = serde_json::to_vec_pretty(manifest).map_err(io::Error::other)?;
    let tmp = folder.join("item.json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, folder.join(MANIFEST))
}

fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}

/// What a write that failed leaves the download in.
fn written(error: io::Error) -> State {
    match error.kind() {
        io::ErrorKind::StorageFull => State::Paused(Halt::DiskFull),
        kind => State::Failed(ProviderError::Write(kind).to_string()),
    }
}

/// The page shows a title offline without what needs the server: not even a trailer's address.
fn offline(mut title: Snapshot) -> Snapshot {
    match &mut title {
        Snapshot::Film(film) => film.trailers = Trailers::default(),
        Snapshot::Episode(_, series) => series.trailers = Trailers::default(),
    }
    title
}

fn subtitles_of(plan: &DownloadPlan) -> Vec<SubtitleFile> {
    plan.files
        .iter()
        .filter_map(|file| match &file.kind {
            DownloadKind::Subtitle { index, format } => Some(SubtitleFile {
                index: *index,
                format: format.clone(),
            }),
            DownloadKind::Media => None,
        })
        .collect()
}

/// The `.part` file as a fetch's sink: it stops the transfer once the download is stopped, and
/// keeps the validator in the manifest before the first byte.
struct Part<'a> {
    file: File,
    stop: &'a AtomicBool,
    started: &'a dyn Fn(Option<String>) -> io::Result<()>,
}

impl Write for Part<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(io::Error::other("stopped"));
        }
        self.file.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Sink for Part<'_> {
    fn start(&mut self, fetched: &Fetched) -> io::Result<()> {
        if !fetched.resumed {
            self.file.set_len(0)?;
        }
        (self.started)(fetched.validator.clone())
    }
}

/// A small file fetched whole into memory.
struct Memory(Vec<u8>);

impl Write for Memory {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Sink for Memory {
    fn start(&mut self, _fetched: &Fetched) -> io::Result<()> {
        self.0.clear();
        Ok(())
    }
}

#[derive(Default)]
struct Table {
    /// By item id.
    items: BTreeMap<String, Manifest>,
    /// The item downloading now.
    current: Option<String>,
}

/// One account's downloads, and the queue that brings them.
pub struct Downloads {
    root: PathBuf,
    server: Arc<dyn MediaServer>,
    disk: Arc<dyn Disk>,
    table: Mutex<Table>,
    /// Held while downloads run: one at a time.
    running: Mutex<()>,
    /// The running download is to stop: paused or removed.
    stop: AtomicBool,
}

impl Downloads {
    /// The downloads of the session's account under `downloads` (`config::downloads_path`): a
    /// download cut by a crash is queued again, a part without a manifest removed.
    pub fn open(
        downloads: &Path,
        server: Arc<dyn MediaServer>,
        disk: Arc<dyn Disk>,
    ) -> io::Result<Downloads> {
        let root = downloads
            .join(safe(&server.server().id))
            .join(safe(server.user_id()));
        let mut items = BTreeMap::new();
        for entry in fs::read_dir(&root).into_iter().flatten() {
            let folder = entry?.path();
            if !folder.is_dir() {
                continue;
            }
            let Ok(text) = fs::read(folder.join(MANIFEST)) else {
                // `add` writes the manifest first: a folder without one is what a crash or a
                // removal left, never a download to resume. Only a folder named as ours are.
                if looks_like_an_item(&folder) {
                    let _ = fs::remove_dir_all(&folder);
                }
                continue;
            };
            // One this Bobine cannot read is left as it is.
            let Ok(mut manifest) = serde_json::from_slice::<Manifest>(&text) else {
                continue;
            };
            if manifest.state == State::Downloading {
                manifest.state = State::Queued;
                save(&folder, &manifest)?;
            }
            items.insert(manifest.item().0.clone(), manifest);
        }
        Ok(Downloads {
            root,
            server,
            disk,
            table: Mutex::new(Table {
                items,
                current: None,
            }),
            running: Mutex::new(()),
            stop: AtomicBool::new(false),
        })
    }

    /// The folder of an item's download.
    pub fn folder(&self, item: &ItemId) -> PathBuf {
        self.root.join(safe(&item.0))
    }

    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn has(&self, item: &ItemId) -> bool {
        self.table().items.contains_key(&item.0)
    }

    /// Queues one version of a film, when the disk has room for it. Already queued: nothing.
    pub fn queue_film(
        &self,
        film: &FilmDetail,
        source: &MediaSourceId,
    ) -> Result<(), DownloadError> {
        if self.has(&film.film.id) {
            return Ok(());
        }
        let plan = self
            .server
            .download_plan(&film.film.id, source)
            .map_err(DownloadError::Server)?;
        self.room(plan.size.unwrap_or(0))?;
        self.add(Snapshot::Film(Box::new(film.clone())), source, &plan)
    }

    /// Queues one version of an episode, like a film.
    pub fn queue_episode(
        &self,
        episode: &Episode,
        source: &MediaSourceId,
    ) -> Result<(), DownloadError> {
        if self.has(&episode.item.id) {
            return Ok(());
        }
        let plan = self
            .server
            .download_plan(&episode.item.id, source)
            .map_err(DownloadError::Server)?;
        self.room(plan.size.unwrap_or(0))?;
        let series = self
            .server
            .series_detail(&episode.series)
            .map_err(DownloadError::Server)?;
        let title = Snapshot::Episode(Box::new(episode.clone()), Box::new(series));
        self.add(title, source, &plan)
    }

    /// Queues each episode of a season the server lets download, in order, its first version,
    /// when the disk has room for them all: how many it queued.
    pub fn queue_season(&self, series: &ItemId, season: &ItemId) -> Result<usize, DownloadError> {
        let wanted: Vec<(Episode, MediaSourceId)> = self
            .server
            .episodes(series, season)
            .map_err(DownloadError::Server)?
            .into_iter()
            .filter(|episode| episode.downloadable && !self.has(&episode.item.id))
            .filter_map(|episode| {
                let source = episode.versions.first()?.id.clone();
                Some((episode, source))
            })
            .collect();
        if wanted.is_empty() {
            return Ok(0);
        }
        let plans = wanted
            .iter()
            .map(|(episode, source)| self.server.download_plan(&episode.item.id, source))
            .collect::<Result<Vec<_>, _>>()
            .map_err(DownloadError::Server)?;
        self.room(plans.iter().map(|plan| plan.size.unwrap_or(0)).sum())?;
        let detail = self
            .server
            .series_detail(series)
            .map_err(DownloadError::Server)?;
        let queued = wanted.len();
        for ((episode, source), plan) in wanted.into_iter().zip(plans) {
            let title = Snapshot::Episode(Box::new(episode), Box::new(detail.clone()));
            self.add(title, &source, &plan)?;
        }
        Ok(queued)
    }

    /// The disk holds `size` and the margin.
    fn room(&self, size: u64) -> Result<(), DownloadError> {
        let free = self
            .disk
            .free(&self.root)
            .map_err(|e| DownloadError::Disk(e.kind()))?;
        let needed = size.saturating_add(MARGIN);
        if free < needed {
            return Err(DownloadError::NoRoom { needed, free });
        }
        Ok(())
    }

    /// Writes the manifest of a new download, last in the queue.
    fn add(
        &self,
        title: Snapshot,
        source: &MediaSourceId,
        plan: &DownloadPlan,
    ) -> Result<(), DownloadError> {
        let mut table = self.table();
        let id = title.item().0.clone();
        if table.items.contains_key(&id) {
            return Ok(());
        }
        let order = table.items.values().map(|m| m.order + 1).max().unwrap_or(0);
        let manifest = Manifest {
            format: FORMAT,
            order,
            title: offline(title),
            source: source.clone(),
            size: plan.size,
            version: plan.version.clone(),
            container: plan.container.clone(),
            validator: None,
            subtitles: subtitles_of(plan),
            segments: Vec::new(),
            trickplay: None,
            state: State::Queued,
            gone: false,
            update: false,
        };
        save(&self.folder(manifest.item()), &manifest)
            .map_err(|e| DownloadError::Disk(e.kind()))?;
        table.items.insert(id, manifest);
        Ok(())
    }

    /// Changes a download's manifest and writes it; nothing when it was removed.
    fn update(&self, id: &str, change: impl FnOnce(&mut Manifest)) -> io::Result<()> {
        let mut table = self.table();
        let Some(manifest) = table.items.get_mut(id) else {
            return Ok(());
        };
        change(manifest);
        save(&self.folder(manifest.item()), manifest)
    }

    /// Does the queued downloads, one at a time, in order, until none is left.
    pub fn run(&self) {
        loop {
            let running = match self.running.try_lock() {
                Ok(running) => running,
                Err(TryLockError::Poisoned(running)) => running.into_inner(),
                Err(TryLockError::WouldBlock) => return,
            };
            while let Some(id) = self.next() {
                self.download(&id);
            }
            drop(running);
            // One queued as this run ended found it still running.
            if self.next().is_none() {
                return;
            }
        }
    }

    fn next(&self) -> Option<String> {
        let table = self.table();
        table
            .items
            .iter()
            .filter(|(_, manifest)| manifest.state == State::Queued)
            .min_by_key(|(_, manifest)| manifest.order)
            .map(|(id, _)| id.clone())
    }

    fn download(&self, id: &str) {
        let outcome = self.bring(id);
        let mut table = self.table();
        table.current = None;
        let Some(manifest) = table.items.get_mut(id) else {
            // Removed while it ran: what it wrote since goes too.
            let _ = fs::remove_dir_all(self.root.join(safe(id)));
            return;
        };
        // Paused or queued again meanwhile, it stays so.
        if manifest.state == State::Downloading {
            manifest.state = outcome.err().unwrap_or(State::Done);
        }
        let _ = save(&self.folder(manifest.item()), manifest);
    }

    /// Brings the file, its subtitles and its extras: what the download is left in otherwise.
    fn bring(&self, id: &str) -> Result<(), State> {
        let manifest = {
            let mut table = self.table();
            let manifest = table.items.get_mut(id).ok_or(State::Done)?;
            // Paused or removed since `next` picked it: not ours to start.
            if manifest.state != State::Queued {
                return Err(State::Done);
            }
            manifest.state = State::Downloading;
            let kept = manifest.clone();
            save(&self.folder(kept.item()), &kept).map_err(written)?;
            table.current = Some(id.to_string());
            self.stop.store(false, Ordering::SeqCst);
            kept
        };
        let item = manifest.item().clone();
        let folder = self.folder(&item);
        let failed = |error: ProviderError| match error {
            _ if self.stop.load(Ordering::SeqCst) => State::Paused(Halt::Asked),
            ProviderError::Write(io::ErrorKind::StorageFull) => State::Paused(Halt::DiskFull),
            error => State::Failed(error.to_string()),
        };
        let plan = self
            .server
            .download_plan(&item, &manifest.source)
            .map_err(failed)?;
        let media = folder.join(manifest.media_name());
        if !media.exists() {
            let part = folder.join(PART);
            let mut validator = manifest.validator.clone();
            if plan.version != manifest.version {
                // Another file than the part's: start again.
                let _ = fs::remove_file(&part);
                validator = None;
            }
            let have = fs::metadata(&part).map_or(0, |part| part.len());
            if self
                .room(plan.size.unwrap_or(0).saturating_sub(have))
                .is_err()
            {
                return Err(State::Paused(Halt::NoRoom));
            }
            self.update(id, |kept| {
                kept.size = plan.size;
                kept.version = plan.version.clone();
                kept.container = plan.container.clone();
                kept.subtitles = subtitles_of(&plan);
            })
            .map_err(written)?;
            let media = {
                let table = self.table();
                let kept = table.items.get(id).ok_or(State::Done)?;
                folder.join(kept.media_name())
            };
            let file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&part)
                .map_err(written)?;
            let started =
                |validator: Option<String>| self.update(id, |kept| kept.validator = validator);
            let mut sink = Part {
                file,
                stop: &self.stop,
                started: &started,
            };
            let file = plan
                .files
                .iter()
                .find(|file| file.kind == DownloadKind::Media)
                .ok_or(State::Failed(ProviderError::NotFound.to_string()))?;
            self.server
                .fetch(file, have, validator.as_deref(), &mut sink)
                .map_err(failed)?;
            fs::rename(&part, media).map_err(written)?;
        }
        for file in &plan.files {
            let DownloadKind::Subtitle { index, format } = &file.kind else {
                continue;
            };
            self.halted()?;
            let mut sink = Memory(Vec::new());
            self.server
                .fetch(file, 0, None, &mut sink)
                .map_err(failed)?;
            let name = subtitle_name(&SubtitleFile {
                index: *index,
                format: format.clone(),
            });
            write_file(&folder.join(name), &sink.0).map_err(written)?;
        }
        self.halted()?;
        let segments = self.server.media_segments(&item).map_err(failed)?;
        let trickplay = self
            .server
            .trickplay(&item, &manifest.source)
            .map_err(failed)?;
        for tile in trickplay.iter().flat_map(|trickplay| 0..trickplay.tiles()) {
            self.halted()?;
            let trickplay = trickplay.as_ref().expect("tiles come from a layout");
            let bytes = self
                .server
                .trickplay_tile(&item, &manifest.source, trickplay, tile)
                .map_err(failed)?;
            write_file(&folder.join(tile_name(trickplay, tile)), &bytes).map_err(written)?;
        }
        for (image, width) in manifest.title.images() {
            self.halted()?;
            let bytes = self.server.image(&image, width).map_err(failed)?;
            write_file(&folder.join(image_name(&image, width)), &bytes).map_err(written)?;
        }
        self.update(id, |kept| {
            kept.segments = segments;
            kept.trickplay = trickplay;
        })
        .map_err(written)
    }

    /// The download was stopped between two files.
    fn halted(&self) -> Result<(), State> {
        match self.stop.load(Ordering::SeqCst) {
            true => Err(State::Paused(Halt::Asked)),
            false => Ok(()),
        }
    }

    /// Pauses a download, the one running included: its part and its place are kept.
    pub fn pause(&self, item: &ItemId) -> io::Result<()> {
        let mut table = self.table();
        if table.current.as_deref() == Some(item.0.as_str()) {
            self.stop.store(true, Ordering::SeqCst);
        }
        let Some(manifest) = table.items.get_mut(&item.0) else {
            return Ok(());
        };
        if matches!(manifest.state, State::Queued | State::Downloading) {
            manifest.state = State::Paused(Halt::Asked);
            save(&self.folder(item), manifest)?;
        }
        Ok(())
    }

    /// Queues a paused or failed download again, at its place.
    pub fn resume(&self, item: &ItemId) -> io::Result<()> {
        let mut table = self.table();
        let Some(manifest) = table.items.get_mut(&item.0) else {
            return Ok(());
        };
        if matches!(manifest.state, State::Paused(_) | State::Failed(_)) {
            manifest.state = State::Queued;
            save(&self.folder(item), manifest)?;
        }
        Ok(())
    }

    /// Stops a download if it runs and deletes its folder.
    pub fn remove(&self, item: &ItemId) -> io::Result<()> {
        let mut table = self.table();
        table.items.remove(&item.0);
        let running = table.current.as_deref() == Some(item.0.as_str());
        if running {
            self.stop.store(true, Ordering::SeqCst);
        }
        match fs::remove_dir_all(self.folder(item)) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            // The run, still writing, removes what is left once it stops.
            Err(_) if running => Ok(()),
            removed => removed,
        }
    }

    /// Every download, in the order of the queue.
    pub fn list(&self) -> Vec<Download> {
        let table = self.table();
        let mut all: Vec<&Manifest> = table.items.values().collect();
        all.sort_by_key(|manifest| manifest.order);
        all.into_iter()
            .map(|manifest| {
                let folder = self.folder(manifest.item());
                let received = [folder.join(PART), folder.join(manifest.media_name())]
                    .iter()
                    .find_map(|file| fs::metadata(file).ok())
                    .map_or(0, |file| file.len());
                Download {
                    item: manifest.item().clone(),
                    manifest: manifest.clone(),
                    received,
                }
            })
            .collect()
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
    fn the_free_space_of_a_folder_not_made_yet_is_its_disk_s() {
        let dir = tempfile::tempdir().unwrap();
        let free = Statvfs.free(&dir.path().join("server-1/user-1")).unwrap();
        assert!(free > 0);
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
    fn a_folder_left_without_its_manifest_is_removed_at_start() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        // A crash after the file was complete, before anything else was kept.
        let left = account(dir.path()).join("film-3");
        std::fs::create_dir_all(left.join("subtitles")).unwrap();
        std::fs::write(left.join("media.mkv"), b"whole").unwrap();
        std::fs::write(left.join("subtitles/5.srt"), b"text").unwrap();
        // Not an item folder: left alone.
        let foreign = account(dir.path()).join("not an item");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("media.mkv"), b"mine").unwrap();

        let reopened = open(dir.path(), &server, roomy());
        assert!(!left.exists());
        assert!(foreign.join("media.mkv").exists());
        assert!(account(dir.path()).join(FILM).join("item.json").exists());
        assert_eq!(reopened.list().len(), 1);
    }

    #[test]
    fn an_empty_id_never_names_the_account_folder() {
        assert!(!safe("").is_empty());
        assert_ne!(safe(""), safe("a"));
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        assert_ne!(downloads.folder(&item("")), account(dir.path()));
        assert!(downloads.folder(&item("")).starts_with(account(dir.path())));
        downloads.remove(&item("")).unwrap();
        assert!(account(dir.path()).join(FILM).join("item.json").exists());
        assert_eq!(downloads.list().len(), 1);

        // An empty server id is not the downloads folder itself either.
        let nameless = open(dir.path(), &server.clone().on_server(""), roomy());
        assert_ne!(
            nameless.folder(&item(FILM)),
            dir.path().join("user-1").join(FILM)
        );
        assert!(
            nameless
                .folder(&item(FILM))
                .starts_with(dir.path().join("%"))
        );
    }

    #[test]
    fn a_pause_between_picking_and_starting_keeps_the_download_paused() {
        let dir = tempfile::tempdir().unwrap();
        let server = fake();
        let downloads = open(dir.path(), &server, roomy());
        downloads
            .queue_film(&film(&server, FILM), &source("source-4k"))
            .unwrap();
        // `run` picked it, then the viewer paused it before it started.
        let picked = downloads.next().unwrap();
        downloads.pause(&item(FILM)).unwrap();
        downloads.download(&picked);
        assert_eq!(manifest(dir.path(), FILM).state, State::Paused(Halt::Asked));
        assert_eq!(
            downloads.list()[0].manifest.state,
            State::Paused(Halt::Asked)
        );
        assert!(server.state.lock().unwrap().record.fetches.is_empty());
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
        // A trailer hosted elsewhere is an address too: the offline page shows none.
        server.state.lock().unwrap().trailers = bobine::provider::Trailers {
            local: true,
            remote: Some("https://video.example.org/watch?v=tide".into()),
        };
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
