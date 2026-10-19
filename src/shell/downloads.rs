//! The download buttons and the Downloads page: what the window shows of `crate::downloads`.
//! Queuing asks the server and a run blocks until the queue is empty, so both go on the
//! background executor. What the page shows is read from `Downloads::list`, again on a timer
//! only while something is queued or coming, and the views are notified only when it changed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bobine::provider::{Episode, ItemId, Title};
use gpui_kit::{Context, ScrollHandle};

use super::Shell;
use crate::downloads::{Disk, Download, DownloadError, Downloads, Snapshot, State};
use crate::motion::Glide;
use crate::navigation::NavEntry;

/// How often the downloads are read while one is queued or coming.
const POLL: Duration = Duration::from_millis(500);

/// What the window holds of the account's downloads.
pub struct DownloadsState {
    /// Where the downloads live (`None`: nowhere, nothing is offered), and what tells how much
    /// the disk holds.
    root: Option<PathBuf>,
    disk: Arc<dyn Disk>,
    /// The account's downloads, once signed in.
    pub store: Option<Arc<Downloads>>,
    /// The downloads as last read.
    pub listed: Rc<[Download]>,
    /// The bytes free on the disk, as last read.
    pub free: Option<u64>,
    /// Why the last download asked was not queued, and the film or series it was asked on.
    pub refused: Option<(ItemId, String)>,
    /// The downloaded titles of the Home's row: the films, and the series of the episodes.
    pub titles: Rc<[Title]>,
    /// Each coming download's bytes at the last read, when, and its speed in bytes a second.
    speeds: HashMap<ItemId, (u64, Instant, Option<f64>)>,
    /// A run is on the background executor.
    running: bool,
    /// The timer that reads the downloads again is set.
    polling: bool,
    pub scroll: ScrollHandle,
    /// The rows' bars ease to each new share.
    pub bars: HashMap<ItemId, Glide>,
    /// The rows removed, still fading and collapsing, and since when.
    pub leaving: Vec<(Download, Instant)>,
}

impl DownloadsState {
    pub fn new(root: Option<PathBuf>, disk: Arc<dyn Disk>) -> Self {
        Self {
            root,
            disk,
            store: None,
            listed: Rc::from([]),
            free: None,
            refused: None,
            titles: Rc::from([]),
            speeds: HashMap::new(),
            running: false,
            polling: false,
            scroll: ScrollHandle::new(),
            bars: HashMap::new(),
            leaving: Vec::new(),
        }
    }

    /// The download of `item`, if there is one.
    pub fn of(&self, item: &ItemId) -> Option<&Download> {
        self.listed.iter().find(|download| &download.item == item)
    }

    /// How fast `item` comes, in bytes a second, once two reads have measured it.
    pub fn speed(&self, item: &ItemId) -> Option<f64> {
        self.speeds.get(item).and_then(|(_, _, speed)| *speed)
    }

    /// Something is queued or coming: the list is read again until nothing is.
    fn active(&self) -> bool {
        self.listed
            .iter()
            .any(|download| matches!(download.manifest.state, State::Queued | State::Downloading))
    }
}

/// The Home's "Downloaded" row: each film downloaded, and the series of each episode
/// downloaded, once, in the order of the queue.
pub fn downloaded_titles(listed: &[Download]) -> Vec<Title> {
    let mut titles: Vec<Title> = Vec::new();
    for download in listed.iter().filter(|d| d.manifest.state == State::Done) {
        let title = match &download.manifest.title {
            Snapshot::Film(film) => Title::Film(film.film.clone()),
            Snapshot::Episode(_, series) => Title::Series(series.series.clone()),
        };
        if !titles.iter().any(|kept| kept.id() == title.id()) {
            titles.push(title);
        }
    }
    titles
}

impl Shell {
    /// The downloads go under `root`, on a disk that says it has what `disk` says.
    #[cfg(test)]
    pub fn downloads_at(&mut self, root: PathBuf, disk: Arc<dyn Disk>) {
        self.downloads = DownloadsState::new(Some(root), disk);
    }

    /// Some downloads of the fake server for the harness (`--screen downloads`): two films and
    /// a season downloaded, a film paused. Under the harness's own data folder.
    #[cfg(feature = "harness")]
    pub fn harness_downloads(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(server)) = (self.downloads.store.clone(), self.server.clone())
        else {
            return;
        };
        let film = |id: &str| server.film(&ItemId(id.into())).ok();
        for id in ["film-1", "film-2"] {
            if let Some(film) = film(id) {
                let _ = store.queue_film(&film, &film.versions[0].id);
            }
        }
        let series = ItemId(crate::fake::SERIES.into());
        let _ = store.queue_season(&series, &ItemId("series-0-s1".into()));
        store.run();
        if let Some(film) = film("film-3") {
            let _ = store.queue_film(&film, &film.versions[0].id);
            let _ = store.pause(&film.film.id);
        }
        self.refresh_downloads(cx);
    }

    /// The downloads of the account signed in: what a crash cut short comes again.
    pub(super) fn open_downloads(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.downloads;
        let fresh = DownloadsState::new(state.root.clone(), state.disk.clone());
        *state = fresh;
        let (Some(root), Some(server)) = (&state.root, self.server.clone()) else {
            return;
        };
        // Unreadable, nothing is offered: the rest of the window goes on without it.
        state.store = Downloads::open(root, server, state.disk.clone())
            .ok()
            .map(Arc::new);
        self.refresh_downloads(cx);
        self.run_downloads(cx);
    }

    /// The Downloads page, from the pill bar or Alt+7.
    pub fn open_downloads_page(&mut self, cx: &mut Context<Self>) {
        self.select(NavEntry::Downloads, cx);
    }

    /// The film page's Download: the version chosen on its chip. Once queued, the button opens
    /// the Downloads page.
    pub fn download_film(&mut self, cx: &mut Context<Self>) {
        let Some((detail, choices)) = self.details.last().and_then(|page| page.ready()) else {
            return;
        };
        if self.downloads.of(&detail.film.id).is_some() {
            self.open_downloads_page(cx);
            return;
        }
        let Some(source) = choices.source(&detail.versions).map(|s| s.id.clone()) else {
            return;
        };
        let (on, detail) = (detail.film.id.clone(), detail.clone());
        self.queue_download(on, move |store| store.queue_film(&detail, &source), cx);
    }

    /// An episode's Download: its first version, as "Download season" takes.
    pub fn download_episode(&mut self, episode: Episode, cx: &mut Context<Self>) {
        if self.downloads.of(&episode.item.id).is_some() {
            self.open_downloads_page(cx);
            return;
        }
        let Some(source) = episode.versions.first().map(|s| s.id.clone()) else {
            return;
        };
        let on = episode.series.clone();
        self.queue_download(on, move |store| store.queue_episode(&episode, &source), cx);
    }

    /// "Download season" on the series page: each of its episodes not downloaded yet.
    pub fn download_season(&mut self, cx: &mut Context<Self>) {
        let Some(page) = self.series_pages.last() else {
            return;
        };
        let Some(season) = page.seasons.get(page.season).map(|s| s.id.clone()) else {
            return;
        };
        let series = page.id.clone();
        let on = series.clone();
        self.queue_download(
            on,
            move |store| store.queue_season(&series, &season).map(|_| ()),
            cx,
        );
    }

    /// Queues off the window's thread (it asks the server), then starts the queue. A refusal
    /// is said on the page of `on`.
    fn queue_download(
        &mut self,
        on: ItemId,
        queue: impl FnOnce(&Downloads) -> Result<(), DownloadError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.downloads.store.clone() else {
            return;
        };
        self.downloads.refused = None;
        let task = cx.background_executor().spawn(async move { queue(&store) });
        cx.spawn(async move |this, cx| {
            let queued = task.await;
            this.update(cx, |shell, cx| {
                match queued {
                    Ok(()) => shell.run_downloads(cx),
                    Err(why) => shell.downloads.refused = Some((on, why.to_string())),
                }
                shell.refresh_downloads(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Runs the queue on the background executor, once at a time; a download queued as a run
    /// ends starts the next one.
    fn run_downloads(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.downloads.store.clone() else {
            return;
        };
        if self.downloads.running {
            return;
        }
        self.downloads.running = true;
        let ran = store.clone();
        let task = cx.background_executor().spawn(async move { ran.run() });
        cx.spawn(async move |this, cx| {
            task.await;
            this.update(cx, |shell, cx| {
                // Another account's run, ended after a sign out, is not this one's.
                if !shell
                    .downloads
                    .store
                    .as_ref()
                    .is_some_and(|s| Arc::ptr_eq(s, &store))
                {
                    return;
                }
                shell.downloads.running = false;
                shell.refresh_downloads(cx);
                let queued = store
                    .list()
                    .iter()
                    .any(|d| d.manifest.state == State::Queued);
                if queued {
                    shell.run_downloads(cx);
                }
            })
            .ok();
        })
        .detach();
        self.watch_downloads(cx);
    }

    pub fn pause_download(&mut self, item: ItemId, cx: &mut Context<Self>) {
        self.change_download(move |store| store.pause(&item), false, cx);
    }

    pub fn resume_download(&mut self, item: ItemId, cx: &mut Context<Self>) {
        self.change_download(move |store| store.resume(&item), true, cx);
    }

    /// Remove, and Cancel for a download not complete: its folder goes, its row fades and
    /// collapses.
    pub fn remove_download(&mut self, item: ItemId, cx: &mut Context<Self>) {
        if let Some(download) = self.downloads.of(&item) {
            let gone = (download.clone(), Instant::now());
            self.downloads.leaving.push(gone);
        }
        self.change_download(move |store| store.remove(&item), false, cx);
    }

    /// Changes a download on disk off the window's thread, then reads them again; `run`: the
    /// queue goes on after.
    fn change_download(
        &mut self,
        change: impl FnOnce(&Downloads) -> std::io::Result<()> + Send + 'static,
        run: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.downloads.store.clone() else {
            return;
        };
        let task = cx
            .background_executor()
            .spawn(async move { change(&store) });
        cx.spawn(async move |this, cx| {
            let changed = task.await;
            this.update(cx, |shell, cx| {
                if changed.is_err() {
                    shell.show_notice("The download could not be changed on disk.", cx);
                }
                if run {
                    shell.run_downloads(cx);
                }
                shell.refresh_downloads(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Reads the downloads and the free space again; the views are told only when something
    /// changed. The timer is set while something is queued or coming.
    pub fn refresh_downloads(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.downloads;
        let Some(store) = &state.store else {
            return;
        };
        let listed = store.list();
        let free = state
            .root
            .as_ref()
            .and_then(|root| state.disk.free(root).ok());
        let now = Instant::now();
        let mut speeds = HashMap::new();
        for download in listed
            .iter()
            .filter(|d| d.manifest.state == State::Downloading)
        {
            let speed = match state.speeds.get(&download.item) {
                Some((bytes, at, speed)) if *bytes == download.received => *speed,
                Some((bytes, at, _)) if now > *at => {
                    let seconds = now.duration_since(*at).as_secs_f64();
                    Some(download.received.saturating_sub(*bytes) as f64 / seconds)
                }
                _ => None,
            };
            speeds.insert(download.item.clone(), (download.received, now, speed));
        }
        state.speeds = speeds;
        if *state.listed != *listed || state.free != free {
            state.titles = downloaded_titles(&listed).into();
            state.listed = listed.into();
            state.free = free;
            cx.notify();
        }
        self.watch_downloads(cx);
    }

    /// Sets the timer that reads the downloads again, while something is queued or coming.
    fn watch_downloads(&mut self, cx: &mut Context<Self>) {
        if self.downloads.polling || !self.downloads.active() {
            return;
        }
        self.downloads.polling = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                let again = this.update(cx, |shell, cx| {
                    shell.refresh_downloads(cx);
                    let again = shell.downloads.active();
                    shell.downloads.polling = again;
                    again
                });
                if !matches!(again, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bobine::provider::{Connector, Episode, ItemId, MediaServer, Title};
    use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, WindowHandle};

    use super::*;
    use crate::downloads::{Halt, State};
    use crate::fake::{FILM, FakeDisk, FakeServer, MemoryKeyring, SERIES};
    use crate::navigation::{NavEntry, Screen};

    const ROOMY: u64 = 1 << 40;
    const OTHER_FILM: &str = "film-1";

    fn item(id: &str) -> ItemId {
        ItemId(id.into())
    }

    /// A shell signed in on `server`, its downloads in a temporary folder of a disk with `free`
    /// bytes free.
    fn start(
        server: &FakeServer,
        free: u64,
        cx: &mut TestAppContext,
    ) -> (WindowHandle<Shell>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let connector: Arc<dyn Connector> = Arc::new(server.clone());
        let window = cx.add_window(|_, cx| {
            crate::keys::bind(cx);
            let mut shell = Shell::new(
                None,
                Box::new(move || Ok(connector)),
                None,
                Arc::new(MemoryKeyring::default()),
                cx,
            );
            shell.downloads_at(root, FakeDisk::with(free));
            shell
        });
        cx.run_until_parked();
        (window, dir)
    }

    fn drawn(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> VisualTestContext {
        let visual = VisualTestContext::from_window((*window).into(), cx);
        visual.run_until_parked();
        visual
    }

    fn click(visual: &mut VisualTestContext, selector: &str) {
        let selector: &'static str = selector.to_string().leak();
        let at = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"))
            .center();
        visual.simulate_mouse_move(at, None, Modifiers::none());
        visual.simulate_click(at, Modifiers::none());
        visual.run_until_parked();
    }

    fn top(visual: &mut VisualTestContext, selector: &str) -> f32 {
        let selector: &'static str = selector.to_string().leak();
        let bounds = visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} was not drawn"));
        f32::from(bounds.top())
    }

    fn store(window: &WindowHandle<Shell>, cx: &mut TestAppContext) -> Arc<Downloads> {
        window
            .update(cx, |shell, _, _| shell.downloads.store.clone())
            .unwrap()
            .expect("the downloads open with the session")
    }

    fn refresh(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| shell.refresh_downloads(cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn open_film(window: &WindowHandle<Shell>, id: &str, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| shell.open_film(item(id), cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn open_series(window: &WindowHandle<Shell>, cx: &mut TestAppContext) {
        window
            .update(cx, |shell, _, cx| shell.open_series(item(SERIES), cx))
            .unwrap();
        cx.run_until_parked();
    }

    fn episode(server: &FakeServer, id: &str) -> Episode {
        match server.title(&item(id)).unwrap() {
            Title::Episode(episode) => episode,
            _ => panic!("an episode"),
        }
    }

    fn queue_film(store: &Downloads, server: &FakeServer, id: &str) {
        let film = server.film(&item(id)).unwrap();
        let source = film.versions[0].id.clone();
        store.queue_film(&film, &source).unwrap();
    }

    fn queue_episode(store: &Downloads, server: &FakeServer, id: &str) {
        let episode = episode(server, id);
        let source = episode.versions[0].id.clone();
        store.queue_episode(&episode, &source).unwrap();
    }

    fn state_of(store: &Downloads, id: &str) -> State {
        store
            .list()
            .into_iter()
            .find(|download| download.item.0 == id)
            .map(|download| download.manifest.state)
            .unwrap()
    }

    #[gpui_kit::test]
    fn the_download_button_is_hidden_when_the_title_cannot_be_downloaded(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        {
            let mut state = server.state.lock().unwrap();
            state.not_downloadable.push(item(FILM));
            // The first episode of each season of the series cannot be downloaded.
            for episode in state.episodes.iter_mut() {
                if episode.series.0 == SERIES && episode.number == Some(1) {
                    episode.downloadable = false;
                }
            }
        }
        let (window, _dir) = start(&server, ROOMY, cx);

        open_film(&window, FILM, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("film-play").is_some());
        assert!(visual.debug_bounds("film-download").is_none());

        open_film(&window, OTHER_FILM, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("film-download").is_some());

        open_series(&window, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("episode 0").is_some());
        assert!(visual.debug_bounds("episode-download 0").is_none());
        assert!(visual.debug_bounds("episode-download 1").is_some());
        assert!(
            visual.debug_bounds("season-download").is_some(),
            "the season has episodes that can be downloaded"
        );

        // A season none of whose episodes can be downloaded offers no "Download season".
        for episode in server.state.lock().unwrap().episodes.iter_mut() {
            if episode.series.0 == SERIES {
                episode.downloadable = false;
            }
        }
        window.update(cx, |shell, _, cx| shell.go_back(cx)).unwrap();
        open_series(&window, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("episode 0").is_some());
        assert!(visual.debug_bounds("episode-download 1").is_none());
        assert!(visual.debug_bounds("season-download").is_none());
    }

    #[gpui_kit::test]
    fn the_download_button_shows_the_progress_then_downloaded(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, _dir) = start(&server, ROOMY, cx);
        open_film(&window, OTHER_FILM, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("film-download Download").is_some());

        // Half of the file has come when the download stops.
        let downloads = store(&window, cx);
        let stopped = downloads.clone();
        server.state.lock().unwrap().mid_download = Some((
            32,
            Box::new(move || stopped.pause(&item(OTHER_FILM)).unwrap()),
        ));
        click(&mut visual, "film-download");
        let mut visual = drawn(&window, cx);
        assert!(
            visual.debug_bounds("film-download 50%").is_some(),
            "half of the 64 bytes"
        );

        window
            .update(cx, |shell, _, cx| {
                shell.resume_download(item(OTHER_FILM), cx)
            })
            .unwrap();
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert_eq!(state_of(&downloads, OTHER_FILM), State::Done);
        assert!(visual.debug_bounds("film-download Downloaded").is_some());
    }

    #[gpui_kit::test]
    fn the_downloads_page_lists_films_then_series_by_season(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, _dir) = start(&server, ROOMY, cx);
        let downloads = store(&window, cx);
        // Queued in no particular order.
        queue_episode(&downloads, &server, "series-0-s2-e1");
        queue_film(&downloads, &server, OTHER_FILM);
        queue_episode(&downloads, &server, "series-0-s1-e2");
        queue_episode(&downloads, &server, "series-0-s1-e1");
        refresh(&window, cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Downloads, cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        window
            .update(cx, |shell, _, _| {
                assert_eq!(shell.navigator.current(), Screen::Downloads)
            })
            .unwrap();
        let order = [
            "download-row film-1",
            "downloads-series series-0",
            "downloads-season series-0-s1",
            "download-row series-0-s1-e1",
            "download-row series-0-s1-e2",
            "downloads-season series-0-s2",
            "download-row series-0-s2-e1",
        ];
        for pair in order.windows(2) {
            assert!(
                top(&mut visual, pair[0]) < top(&mut visual, pair[1]),
                "{} above {}",
                pair[0],
                pair[1]
            );
        }
        assert!(visual.debug_bounds("downloads-disk").is_some(), "disk use");
    }

    #[gpui_kit::test]
    fn pausing_a_download_keeps_its_part_and_its_place(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, dir) = start(&server, ROOMY, cx);
        let downloads = store(&window, cx);
        queue_film(&downloads, &server, OTHER_FILM);
        queue_film(&downloads, &server, "film-2");
        refresh(&window, cx);
        window
            .update(cx, |shell, _, cx| shell.select(NavEntry::Downloads, cx))
            .unwrap();
        let mut visual = drawn(&window, cx);
        click(&mut visual, "download-pause film-1");
        assert_eq!(state_of(&downloads, OTHER_FILM), State::Paused(Halt::Asked));
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("download-resume film-1").is_some());

        // Paused again halfway, while it comes.
        let stopped = downloads.clone();
        server.state.lock().unwrap().mid_download = Some((
            32,
            Box::new(move || stopped.pause(&item(OTHER_FILM)).unwrap()),
        ));
        click(&mut visual, "download-resume film-1");
        assert_eq!(state_of(&downloads, OTHER_FILM), State::Paused(Halt::Asked));
        let part = dir
            .path()
            .join("server-1/user-1")
            .join(OTHER_FILM)
            .join("media.part");
        assert_eq!(std::fs::read(&part).unwrap().len(), 32, "its part is kept");
        let mut visual = drawn(&window, cx);
        assert!(
            top(&mut visual, "download-row film-1") < top(&mut visual, "download-row film-2"),
            "and its place"
        );

        click(&mut visual, "download-resume film-1");
        assert_eq!(state_of(&downloads, OTHER_FILM), State::Done);
        let fetches = server.state.lock().unwrap().record.fetches.clone();
        let resumed = fetches
            .iter()
            .rev()
            .find(|(file, _, _)| file.handle.starts_with(OTHER_FILM))
            .unwrap();
        assert_eq!(resumed.1, 32, "the download goes on from its part");
    }

    #[gpui_kit::test]
    fn a_refused_download_says_how_much_space_is_missing(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, _dir) = start(&server, 1_000_000_000, cx);
        open_film(&window, OTHER_FILM, cx);
        let mut visual = drawn(&window, cx);
        click(&mut visual, "film-download");
        let said = "Not enough space: 2.1 GB needed, 1.0 GB free.";
        window
            .update(cx, |shell, _, _| {
                let refused = shell.downloads.refused.as_ref();
                assert_eq!(refused, Some(&(item(OTHER_FILM), said.to_string())));
                assert!(shell.downloads.listed.is_empty());
            })
            .unwrap();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("download-refused").is_some());
    }

    #[gpui_kit::test]
    fn home_shows_a_downloaded_row_only_when_something_is_downloaded(cx: &mut TestAppContext) {
        let server = FakeServer::new(6, true);
        let (window, _dir) = start(&server, ROOMY, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("home-row Next up").is_some());
        assert!(visual.debug_bounds("home-row Downloaded").is_none());

        // Queued is not downloaded.
        let downloads = store(&window, cx);
        queue_film(&downloads, &server, OTHER_FILM);
        queue_episode(&downloads, &server, "series-0-s1-e1");
        refresh(&window, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("home-row Downloaded").is_none());

        downloads.run();
        refresh(&window, cx);
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("home-row Downloaded").is_some());
        assert!(
            top(&mut visual, "home-row Next up") < top(&mut visual, "home-row Downloaded"),
            "after Next up"
        );
        window
            .update(cx, |shell, _, _| {
                let ids: Vec<&str> = shell
                    .downloads
                    .titles
                    .iter()
                    .map(|t| t.id().0.as_str())
                    .collect();
                assert_eq!(
                    ids,
                    [OTHER_FILM, SERIES],
                    "the film, and the episode's series"
                );
            })
            .unwrap();

        for id in [OTHER_FILM, "series-0-s1-e1"] {
            window
                .update(cx, |shell, _, cx| shell.remove_download(item(id), cx))
                .unwrap();
        }
        cx.run_until_parked();
        let mut visual = drawn(&window, cx);
        assert!(visual.debug_bounds("home-row Downloaded").is_none());
    }
}
