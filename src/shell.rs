//! The root view: the screen the navigator points at, framed by the pill bar or the back link.

mod desktop;
mod discover;
mod home;
mod keys;
mod lists;
mod pages;
mod palette;
mod playlists;
mod posters;
mod preview;
mod request;
mod series;
mod settings;
mod skip;
mod stats;
mod trailer;

pub use discover::{OpenSeerr, Seerr};
pub use home::{HomePage, ListPage};
pub use lists::{CollectionPage, CollectionsPage, ListCard, ListsPage, MyListPage, PlayerList};
pub use pages::{Chip, DetailPage, GONE, LibraryPage, Loadable, LoginForm, OpenConnector, Tool};
pub use palette::{PersonPage, SearchState};
pub use playlists::{PlaylistMenu, PlaylistPage};
pub use request::RequestPage;
pub use series::SeriesPage;
pub use settings::{QualityMenu, SettingsPage};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, App, Context, DispatchPhase, Entity, FocusHandle, Focusable,
    IntoElement, MouseButton, MouseDownEvent, Pixels, Render, SharedString, Size, Subscription,
    Task, Window, canvas, div, px,
};

use bobine::provider::{
    Connector, Film as FilmItem, MediaServer, Playback as Opened, ProviderError, Series, TokenStore,
};

use crate::chrome::{Account, AccountView, ButtonBounds, back_link, bar_fade, pill_bar};
use crate::config::Settings;
use crate::fallback::{Fallback, Reopen};
use crate::images::Images;
use crate::menu::Menus;
use crate::motion::{self, Fade, Presence};
use crate::navigation::{NavEntry, Navigator, Screen};
use crate::player::controls::KeyAction;
use crate::player::libmpv::Libmpv;
use crate::player::mpv::MpvPlayer;
use crate::player::segments::{Button, Label, offered};
use crate::player::session::{Session, WindowRequest};
use crate::player::{Media, Player};
use crate::poster_actions::Marks;
use crate::reports::{self, Reporting};
use crate::ring::Ring;
use crate::screens;
use crate::series_page::{self, Next, UpNext};
use crate::theme;
use crate::video::Rect;
use crate::video::blackout::{self, layer::Screens};
use crate::video::subsurface::{VideoSurface, Wayland};
use preview::SeekPreview;
use uuid::Uuid;

/// How often the player is asked what changed, and the controls checked for hiding.
const PLAYER_TICK: Duration = Duration::from_millis(100);

/// How long closing the window waits for "stopped" to reach the server.
const CLOSE_WAITS_FOR_REPORTS: Duration = Duration::from_secs(2);

/// Where the keyboard focus goes at a draw.
#[derive(Debug, PartialEq, Eq)]
enum FocusTo {
    /// It stays where it is.
    Keep,
    /// The window's root, so that Escape and the player's keys are heard.
    Root,
    /// The login's address field.
    Address,
}

/// What to do with the focus at a draw. A text field keeps it while it has it; the login's
/// address field takes it when nothing has, and also when the login has just been shown: the
/// window starts on the login before the form exists, so the root took the focus meanwhile.
fn focus_to(login_shown: bool, address_due: &mut bool, nothing_focused: bool) -> FocusTo {
    if login_shown && *address_due {
        *address_due = false;
        return FocusTo::Address;
    }
    if nothing_focused {
        return if login_shown {
            FocusTo::Address
        } else {
            FocusTo::Root
        };
    }
    FocusTo::Keep
}

/// What becomes of a fullscreen kept for the next episode, at a draw.
#[derive(Debug, PartialEq, Eq)]
enum KeptFullscreen {
    /// Still kept, or nothing was.
    Keep,
    /// The next episode plays: the fullscreen is the player's again.
    Release,
    /// The card was cancelled, or the next episode did not play: the window leaves it.
    GiveBack,
}

/// `kept`: the fullscreen is kept for the next episode; `on_player`, it plays; `card`, the card
/// counts down; `waiting`, the series page opens the next episode.
fn kept_fullscreen(kept: bool, on_player: bool, card: bool, waiting: bool) -> KeptFullscreen {
    match (kept, on_player) {
        (false, _) => KeptFullscreen::Keep,
        (true, true) => KeptFullscreen::Release,
        (true, false) if card || waiting => KeptFullscreen::Keep,
        (true, false) => KeptFullscreen::GiveBack,
    }
}

/// Leaving the player in fullscreen keeps it only for the next episode the card offers.
fn keeps_fullscreen(offered: bool, fullscreen: bool) -> bool {
    offered && fullscreen
}

pub struct Shell {
    pub navigator: Navigator,
    focus: FocusHandle,
    font: Option<SharedString>,
    pub playback: Option<Playback>,
    /// What the video binds of the compositor, once for this window: films reuse it.
    wayland: Option<Rc<RefCell<Wayland>>>,
    /// What the viewer chose, and where it is kept (`None`: kept nowhere).
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    /// Where the secrets the settings hold are kept: the system keyring.
    secrets: Arc<dyn TokenStore>,
    pub settings_page: SettingsPage,
    /// The request server, when one is configured (#63), and how its client is made.
    pub seerr: Seerr,
    pub open_seerr: OpenSeerr,
    /// What turns the other monitors black, from the first film on; `None` after that when the
    /// compositor cannot (no layer shell): the switch is not offered.
    screens: Option<Screens>,
    screens_tried: bool,
    window_seen: blackout::Steady<(bool, Option<Uuid>)>,
    /// The "Black out other screens" switch slides from one side to the other.
    pub blackout_switch: Fade,
    /// The kept session is being looked for: nothing to show yet.
    pub starting: bool,
    connector: Option<Arc<dyn Connector>>,
    server: Option<Arc<dyn MediaServer>>,
    pub login: Option<LoginForm>,
    /// The menu of the account button in the pill bar, which fades in and out; where the button
    /// was drawn.
    account_menu: Menus<Account>,
    account_fade: Presence<Account>,
    account_button: ButtonBounds,
    /// The login was just shown: its address field takes the focus at the next draw.
    login_focus_due: bool,
    pub home: HomePage,
    /// The full list of a row of the Home, while one is open.
    pub list: Option<ListPage>,
    /// Makes the header change title by itself, while the Home shows.
    hero_ticker: Option<Task<()>>,
    /// The Home's header, drawn apart: its ticks redraw it alone, not the whole page.
    pub hero_view: Entity<screens::HeroView>,
    pub library: pages::LibraryPage<FilmItem>,
    pub series: pages::LibraryPage<Series>,
    /// The film pages open, the last one on top: one per `Screen::FilmDetail` of the navigator.
    pub details: Vec<DetailPage>,
    /// The series pages open, the last one on top: one per `Screen::SeriesDetail`.
    pub series_pages: Vec<SeriesPage>,
    /// The "Next episode" card over the series page, after an episode ended, or over a
    /// playlist's page, after one of its titles ended.
    pub up_next: UpNext<Next>,
    up_next_ticker: Option<Task<()>>,
    /// The numbers of the series pages' refreshes, counted over every page: an answer on its way
    /// for a page that closed never matches the page that opens in its place.
    refreshes_asked: u64,
    /// The film and series pages opened so far: each takes the next number, so that an answer on
    /// its way for a page that closed never reaches the page that opens in its place.
    pages_opened: u64,
    /// The card fades in and out; the episode it showed stays drawn while it fades out.
    pub up_next_fade: Presence<Next>,
    /// The episode ended in fullscreen: the window stays so while the card counts down, and
    /// leaves it if the next episode does not play.
    fullscreen_kept: bool,
    /// The person pages open, the last one on top: one per `Screen::Person` of the navigator.
    pub people: Vec<PersonPage>,
    pub collections: CollectionsPage,
    /// The collection pages open, the last one on top: one per `Screen::CollectionDetail`.
    pub collection_pages: Vec<CollectionPage>,
    pub my_list: MyListPage,
    pub lists: ListsPage,
    /// What the viewer watched, once read: None while there is nothing to show, and no page.
    pub stats: Option<Rc<crate::stats::Stats>>,
    /// The local clock the stats are counted on: a fixed one in the tests and the harness.
    pub clock: fn() -> bobine::provider::LocalTime,
    /// The Stats page scrolls down, and its rows of movies and of series sideways.
    pub stats_scrolls: [gpui_kit::ScrollHandle; 3],
    /// The watch history is being read: it is not asked again meanwhile.
    stats_reading: bool,
    /// Each read of the watch history is numbered: only the newest one's answer is shown.
    stats_asked: u64,
    /// The playlists' pages open, the last one on top: one per `Screen::Playlist`.
    pub playlist_pages: Vec<PlaylistPage>,
    /// The "Add to playlist" menu, while it shows.
    pub playlist_menu: Option<PlaylistMenu>,
    /// How many times the menu was opened: each open's reads carry its number.
    playlist_menus: u64,
    /// The search palette, while it shows (it stays fading out after it closed).
    pub search: Option<SearchState>,
    /// The pages gone back from, the last one on top: the mouse's forward button opens them again.
    forward: Vec<pages::Reopen>,
    /// Each film and start position a playback was asked with, even when the server refused it:
    /// no player starts in a test, so this is what the player would have been given.
    #[cfg(test)]
    planned: Vec<(bobine::provider::ItemId, Duration)>,
    /// The player the next playback starts on, in a test: no mpv, no window.
    #[cfg(test)]
    fake_player: Option<crate::player::fake::FakePlayer>,
    /// The highlight of each entry of the pill bar eases from one section to the next.
    nav_pills: [Fade; NavEntry::ALL.len()],
    pub images: Images,
    /// The cards drawn that answer the pointer, each a view of its own.
    pub cards: screens::Cards,
    /// The keyboard's focus ring over the cards.
    pub ring: Ring,
    /// The rows and the pages drawn that scroll: the ring scrolls them to the card it is on.
    pub scrolls: Vec<gpui_kit::ScrollHandle>,
    /// The help over the screen, `?`.
    help_open: bool,
    help_fade: Fade,
    /// The volume and the mute the last film was left at, carried to the next one.
    volume: (f64, bool),
    /// What the buttons of the posters changed that their lists do not say yet.
    pub marks: Marks,
    /// What the window says over the page for a moment (a poster's button the server refused),
    /// fading in and out; the wait before it goes.
    notice: Option<&'static str>,
    notice_fade: Presence<&'static str>,
    notice_timer: Option<Task<()>>,
    /// `yt-dlp` is installed, read once at start: a trailer hosted elsewhere plays through it.
    pub ytdlp: bool,
    /// Each trailer the player was given, with the options of its mpv: no player starts in a
    /// test.
    #[cfg(test)]
    trailers_opened: Vec<(String, Vec<(&'static str, &'static str)>)>,
    /// The wake-up planned for an image to ask again, and when.
    image_wake: Option<(Instant, Task<()>)>,
    /// The page shown at the last draw, and how many times it changed: each change slides the
    /// new page in, forwards (deeper) or backwards.
    page: (Screen, usize),
    page_turns: u64,
    turned_forward: bool,
    /// The window's size at the last draw: a chip menu does not follow its chip when it changes.
    viewport: Size<Pixels>,
    /// When each draw started: the harness reads the frame times of a move from it.
    #[cfg(feature = "harness")]
    pub frames: std::cell::RefCell<Vec<Instant>>,
    /// When the shell and its header were drawn: the tests and the harness tell from them whether
    /// the whole window is built again while only the header moves.
    #[cfg(any(test, feature = "harness"))]
    pub draws: std::cell::RefCell<Draws>,
    /// Given to every mpv: the headless harness silences the sound.
    pub mpv_options: Vec<(&'static str, &'static str)>,
    /// What the desktop's media controls are shown, and where the posters they show are
    /// written (`None`: nowhere, and none is shown).
    pub mpris: crate::mpris::Publisher,
    art_dir: Option<PathBuf>,
}

/// When the shell and its header were drawn, and, for the harness, when the shell was notified.
#[cfg(any(test, feature = "harness"))]
#[derive(Default)]
pub struct Draws {
    pub shell: Vec<Instant>,
    pub hero: Vec<Instant>,
    /// The cards that answer the pointer, by card.
    pub cards: Vec<(gpui_kit::ElementId, Instant)>,
    #[cfg(feature = "harness")]
    pub notified: Vec<Instant>,
}

/// The film being played and what keeps it in step with the window.
pub struct Playback {
    pub session: Session,
    /// The controls and the panel fade in and out.
    controls_fade: Fade,
    panel_fade: Fade,
    settings_fade: Fade,
    /// The tracks of the panel scroll on this, so that a long list opens on the one shown.
    pub tracks_scroll: gpui_kit::ScrollHandle,
    controls_shown: bool,
    had_error: bool,
    /// What the server is told; `None` for a local file.
    reporting: Option<Reporting>,
    /// The attempt that failed is being told stopped: the conversion's reports wait for it.
    stopping: Option<std::thread::JoinHandle<()>>,
    /// The episode after the one playing, or the playlist's next title: the card offers it at
    /// the end.
    next: Option<Next>,
    /// Told apart from every other playback: an answer on its way for one already left is
    /// dropped.
    generation: u64,
    /// The skip button fades in and out; what it said stays drawn while it fades out.
    skip_fade: Presence<Button>,
    skip_label: Option<Label>,
    /// The pointer over the seek bar: the time under it and the thumbnail of that moment.
    seek: SeekPreview,
    /// What the server converts when the file does not play; `None` for a local file.
    fallback: Option<Fallback>,
    notice_fade: Fade,
    notice_shown: bool,
    /// "This quality could not be started."
    refusal_fade: Fade,
    refusal_shown: bool,
    /// The "My list" button of the player.
    pub list: Option<PlayerList>,
    /// What the desktop's media controls show of it; `None` for a trailer, which shows nothing.
    track: Option<crate::mpris::Track>,
    /// What plays, as the server knows it: what "Add to playlist" puts in a playlist.
    pub playing: Option<bobine::provider::ItemId>,
    _ticker: Task<()>,
    _bounds: Subscription,
}

/// What the player shows of the film above the seek bar.
pub struct Film {
    pub title: String,
    pub subtitle: Option<String>,
    /// What the player's "My list" button puts in the list: the film, or the series of the
    /// episode; `None` for a file of no server.
    pub list: Option<bobine::provider::ItemId>,
    /// What the desktop's media controls show; `None` for a trailer.
    pub desktop: Option<crate::mpris::Title>,
}

impl Shell {
    /// The font the window is drawn with, when it is not the system's.
    pub fn font(&self) -> Option<SharedString> {
        self.font.clone()
    }

    pub fn new(
        font: Option<SharedString>,
        open: OpenConnector,
        settings_path: Option<PathBuf>,
        secrets: Arc<dyn TokenStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        let settings = settings_path
            .as_deref()
            .map(Settings::read)
            .unwrap_or_default();
        let blackout_switch = Fade::starting_at(motion::quick(), settings.black_out_other_monitors);
        let this = cx.weak_entity();
        let hero_view = cx.new(|_| screens::HeroView::new(this));
        let mut shell = Self {
            navigator: Navigator::new(),
            focus: cx.focus_handle(),
            font,
            playback: None,
            wayland: None,
            settings,
            settings_path,
            secrets,
            settings_page: SettingsPage::default(),
            seerr: Seerr::default(),
            open_seerr: discover::real_seerr(),
            screens: None,
            screens_tried: false,
            window_seen: blackout::Steady::default(),
            blackout_switch,
            starting: true,
            connector: None,
            server: None,
            login: None,
            account_menu: Menus::default(),
            account_fade: Presence::new(motion::quick()),
            account_button: Default::default(),
            login_focus_due: false,
            home: Default::default(),
            list: None,
            hero_ticker: None,
            hero_view,
            library: Default::default(),
            series: Default::default(),
            details: Vec::new(),
            series_pages: Vec::new(),
            up_next: UpNext::default(),
            up_next_ticker: None,
            refreshes_asked: 0,
            pages_opened: 0,
            up_next_fade: Presence::new(motion::quick()),
            fullscreen_kept: false,
            people: Vec::new(),
            collections: Default::default(),
            collection_pages: Vec::new(),
            my_list: Default::default(),
            lists: Default::default(),
            stats: None,
            #[cfg(not(test))]
            clock: bobine::provider::LocalTime::now,
            #[cfg(test)]
            clock: || bobine::provider::LocalTime::from_unix(crate::fake::NOW),
            stats_scrolls: Default::default(),
            stats_reading: false,
            stats_asked: 0,
            playlist_pages: Vec::new(),
            playlist_menu: None,
            playlist_menus: 0,
            search: None,
            forward: Vec::new(),
            #[cfg(test)]
            planned: Vec::new(),
            #[cfg(test)]
            fake_player: None,
            nav_pills: [Fade::new(motion::quick()); NavEntry::ALL.len()],
            images: Images::default(),
            cards: Default::default(),
            ring: Ring::default(),
            scrolls: Vec::new(),
            help_open: false,
            help_fade: Fade::new(motion::quick()),
            volume: (100., false),
            marks: Marks::default(),
            notice: None,
            notice_fade: Presence::new(motion::quick()),
            notice_timer: None,
            // A test never depends on what this machine has installed: it says so itself.
            #[cfg(not(test))]
            ytdlp: crate::trailer::ytdlp_installed(),
            #[cfg(test)]
            ytdlp: false,
            #[cfg(test)]
            trailers_opened: Vec::new(),
            image_wake: None,
            page: (Screen::Login, 0),
            page_turns: 0,
            turned_forward: true,
            viewport: Size::default(),
            #[cfg(feature = "harness")]
            frames: Default::default(),
            #[cfg(any(test, feature = "harness"))]
            draws: Default::default(),
            mpv_options: Vec::new(),
            // A test never reaches a bus nor writes a poster: it gives its own.
            #[cfg(not(test))]
            mpris: crate::mpris::Publisher::new(Box::new(crate::mpris::dbus::Dbus::session())),
            #[cfg(test)]
            mpris: crate::mpris::Publisher::new(Box::new(
                crate::mpris::fake::FakeControls::default(),
            )),
            #[cfg(not(test))]
            art_dir: crate::mpris::art_dir(
                std::env::var_os("XDG_CACHE_HOME"),
                std::env::var_os("HOME"),
            ),
            #[cfg(test)]
            art_dir: None,
        };
        shell.begin(open, cx);
        shell
    }

    /// The desktop's media controls off: no bus taken, no poster written. The harness on its fake
    /// server must not show invented films on the user's real bus nor fill their real cache.
    #[cfg(any(test, feature = "harness"))]
    pub fn without_desktop(&mut self) {
        self.mpris = crate::mpris::Publisher::new(Box::new(crate::mpris::dbus::Dbus::at(None)));
        self.art_dir = None;
    }

    /// A frame was drawn: the page and its header are two views of one frame, which count once.
    #[cfg(feature = "harness")]
    pub fn note_frame(&self) {
        let now = Instant::now();
        let mut frames = self.frames.borrow_mut();
        if frames
            .last()
            .is_none_or(|last| now.duration_since(*last) > Duration::from_millis(3))
        {
            frames.push(now);
        }
    }

    /// The number of the film or series page about to open.
    fn next_generation(&mut self) -> u64 {
        self.pages_opened += 1;
        self.pages_opened
    }

    /// Opens the player on `media`. Its addresses go straight to mpv: nothing here keeps or
    /// prints them.
    pub fn play(
        &mut self,
        mpv: Arc<Libmpv>,
        media: Media<'_>,
        film: Film,
        reporting: Option<Reporting>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let session = self.open_session(mpv, media, film, window)?;
        self.begin_playback(session, reporting, window, cx);
        Ok(())
    }

    /// The video surface under the window and mpv loading `media`, not yet on the window.
    fn open_session(
        &mut self,
        mpv: Arc<Libmpv>,
        media: Media<'_>,
        film: Film,
        window: &mut Window,
    ) -> Result<Session, String> {
        let wayland = match &self.wayland {
            Some(wayland) => wayland.clone(),
            None => self
                .wayland
                .insert(Rc::new(RefCell::new(Wayland::new(window)?)))
                .clone(),
        };
        if !self.screens_tried {
            self.screens_tried = true;
            self.screens = Screens::connect().ok();
        }
        let display = VideoSurface::new(wayland, mpv.clone())?;
        let mut player = MpvPlayer::new(mpv);
        player.load(media).map_err(|e| e.to_string())?;
        Ok(Session::new(
            Box::new(player),
            Box::new(display),
            film.title,
            film.subtitle,
        ))
    }

    /// Puts `session` on the window: the player screen opens on it.
    fn begin_playback(
        &mut self,
        session: Session,
        reporting: Option<Reporting>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ticker = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(PLAYER_TICK).await;
                if this
                    .update_in(cx, |shell, window, cx| shell.tick(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let bounds = cx.observe_window_bounds(window, |shell, window, cx| {
            shell.lay_out_video(window);
            cx.notify();
        });
        let mut session = session;
        session.carry_volume(self.volume.0, self.volume.1);
        self.playback = Some(Playback {
            session,
            controls_fade: Fade::new(motion::quick()),
            panel_fade: Fade::new(motion::quick()),
            settings_fade: Fade::new(motion::quick()),
            tracks_scroll: gpui_kit::ScrollHandle::new(),
            controls_shown: false,
            had_error: false,
            reporting,
            stopping: None,
            next: None,
            generation: 0,
            skip_fade: Presence::new(motion::quick()),
            skip_label: None,
            seek: SeekPreview::new(),
            fallback: None,
            notice_fade: Fade::new(motion::quick()),
            notice_shown: false,
            refusal_fade: Fade::new(motion::quick()),
            refusal_shown: false,
            list: None,
            track: None,
            playing: None,
            _ticker: ticker,
            _bounds: bounds,
        });
        // What the desktop asked while nothing played is not for this film.
        self.mpris.commands();
        self.lay_out_video(window);
        self.forward.clear();
        self.navigator.open(Screen::Player);
        cx.notify();
    }

    /// Closes the player: the video surface and mpv go before anything else does. The server is
    /// told the playback stopped; the handle ends once it has been.
    pub fn stop_playback(&mut self) -> Option<std::thread::JoinHandle<()>> {
        // The covers first: mpv shutting down can take a moment.
        let reports = blackout::stop_with(self.screens.as_mut(), || {
            let Playback {
                session,
                reporting,
                stopping,
                ..
            } = self.playback.take()?;
            self.volume = session.volume();
            let reports = reports::last_reports(reporting, stopping, &session);
            drop(session);
            reports
        });
        self.mpris.stop();
        reports
    }

    /// The window is closing: the film goes, the server is told where it stopped (a moment
    /// is waited for it, no more), then what the video bound of the compositor goes, while
    /// GPUI's display is still connected.
    pub fn close_video(&mut self) {
        if let Some(reports) = self.stop_playback() {
            reports::wait(reports, CLOSE_WAITS_FOR_REPORTS);
        }
        self.screens = None;
        self.wayland = None;
        self.mpris.close();
    }

    /// Where the account button was last drawn.
    #[cfg(feature = "harness")]
    pub fn account_button_bounds(&self) -> Option<gpui_kit::Bounds<Pixels>> {
        self.account_button.get()
    }

    /// The "Black out other screens" switch is offered: the compositor can cover a monitor.
    #[cfg(feature = "harness")]
    pub fn blackout_offered(&self) -> bool {
        self.screens.is_some()
    }

    /// The switch was pressed: the option changes, is kept, and applies at once.
    pub fn toggle_blackout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.black_out_other_monitors = !self.settings.black_out_other_monitors;
        self.keep_settings();
        self.follow_blackout(window, cx);
        cx.notify();
    }

    /// The other monitors are covered while a film plays fullscreen with the option on, and
    /// uncovered as soon as one of those stops, whoever ended the fullscreen.
    fn follow_blackout(&mut self, window: &Window, cx: &App) {
        let Some(screens) = &mut self.screens else {
            return;
        };
        let fullscreen = window.is_fullscreen();
        let window_on = window.display(cx).and_then(|display| display.uuid().ok());
        // Not on the tick the fullscreen starts: GPUI may still name the old monitor then.
        let steady = self.window_seen.settles((fullscreen, window_on));
        let wanted = steady
            && blackout::wanted(
                self.settings.black_out_other_monitors,
                self.playback.is_some(),
                fullscreen,
            );
        screens.want(wanted, window_on);
        screens.beat();
    }

    /// Back to the page the film was started from. `ended`: the file ended by itself, and the
    /// card may offer the next episode.
    fn leave_player(&mut self, ended: bool, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.playback.as_mut().and_then(|playback| {
            let session = &playback.session;
            series_page::offered_on_leaving(
                ended,
                playback.next.take(),
                session.position,
                session.duration,
            )
        });
        self.leave_to(next, window, cx);
    }

    /// Back to the page the film was started from, the card offering `next` there.
    fn leave_to(&mut self, next: Option<Next>, window: &mut Window, cx: &mut Context<Self>) {
        self.fullscreen_kept = keeps_fullscreen(next.is_some(), window.is_fullscreen());
        if window.is_fullscreen() && !self.fullscreen_kept {
            window.toggle_fullscreen();
        }
        self.release_tiles(window, cx);
        let reports = self.stop_playback();
        self.follow_blackout(window, cx);
        self.navigator.back();
        if let Some(reports) = reports {
            self.after_playback(reports, cx);
        }
        if let Some(next) = next {
            self.start_up_next(next, window, cx);
        }
        cx.notify();
    }

    /// The video fills the window: the controls and the panel float over it.
    fn lay_out_video(&mut self, window: &Window) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let size = window.viewport_size();
        let (width, height) = (size.width.into(), size.height.into());
        let whole = Rect {
            x: 0.,
            y: 0.,
            width,
            height,
        };
        playback
            .session
            .layout((width, height), whole, window.scale_factor());
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.follow_blackout(window, cx);
        if self.playback.is_none() {
            return;
        }
        self.follow_desktop(window, cx);
        let Some(playback) = &mut self.playback else {
            return;
        };
        let now = Instant::now();
        let ended = playback.session.update(now);
        if let Some(reporting) = &mut playback.reporting {
            reporting.tick(now, &playback.session, ended);
        }
        if ended {
            // The end of the film: back to where it was started from.
            self.leave_player(true, window, cx);
            return;
        }
        let ask = playback
            .fallback
            .as_mut()
            .and_then(|f| f.tick(&mut playback.session, now));
        if let Some(request) = ask {
            self.reopen(Reopen::Convert(request), true, cx);
        }
        self.show_to_desktop();
        let Some(playback) = &mut self.playback else {
            return;
        };
        let shown = playback.session.controls_visible(now);
        let has_error = playback.session.error().is_some();
        if preview::bar_is_left(playback.controls_shown, shown) {
            playback.seek.scrub.left_bar();
        }
        let notice = playback
            .fallback
            .as_ref()
            .is_some_and(|f| f.notice_shown(&playback.session, now));
        let refusal = playback
            .fallback
            .as_ref()
            .is_some_and(|f| f.refusal_shown(now));
        let skip_label = playback.session.skip_button().map(|button| button.label);
        // Hidden controls have nothing to redraw: the video is not GPUI's.
        if shown
            || shown != playback.controls_shown
            || has_error != playback.had_error
            || notice != playback.notice_shown
            || refusal != playback.refusal_shown
            || skip_label != playback.skip_label
        {
            playback.controls_shown = shown;
            playback.had_error = has_error;
            playback.notice_shown = notice;
            playback.refusal_shown = refusal;
            playback.skip_label = skip_label;
            cx.notify();
        }
    }

    /// What the account button of the bar shows now: who is signed in, and how much of its menu.
    fn account_view(&mut self, window: &mut Window) -> Option<AccountView> {
        let server = self.server.clone()?;
        let now = Instant::now();
        let open = self.account_menu.open();
        let drawn = self.account_fade.follow(open, now);
        if self.account_fade.moving(now) {
            window.request_animation_frame();
        }
        Some(AccountView {
            user: server.user_name().to_string(),
            server: server.server().name.clone(),
            open: open.is_some(),
            stats: self.stats.is_some(),
            shown: drawn.map(|(_, shown)| shown),
            bounds: self.account_button.clone(),
        })
    }

    /// The server is asked for a new playback of the film: its conversion when the file does
    /// not play as it is, which is told stopped first, or the quality the viewer chose, while
    /// the film plays on until it comes.
    fn reopen(&mut self, request: Reopen, stop_first: bool, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let Some(fallback) = &playback.fallback else {
            return;
        };
        if stop_first && let Some(reporting) = playback.reporting.take() {
            playback.stopping = Some(reporting.finish(&playback.session));
        }
        let (server, id) = (fallback.server.clone(), fallback.id());
        let task = cx
            .background_executor()
            .spawn(async move { request.open(&*server) });
        cx.spawn(async move |this, cx| {
            let opened = task.await;
            this.update(cx, |shell, cx| shell.converted(id, opened, cx))
                .ok();
        })
        .detach();
        cx.notify();
    }

    /// The server's answer for the playback `id`.
    fn converted(
        &mut self,
        id: u64,
        opened: Result<Opened, ProviderError>,
        cx: &mut Context<Self>,
    ) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let Some(fallback) = &mut playback.fallback else {
            return;
        };
        if let Some(report) = fallback.answered(id, &mut playback.session, opened, Instant::now()) {
            // The playback the viewer left for another quality ends here.
            if let Some(reporting) = playback.reporting.take() {
                playback.stopping = Some(reporting.finish(&playback.session));
            }
            playback.reporting = Some(Reporting::start_after(
                fallback.server.clone(),
                report,
                playback.stopping.take(),
            ));
        }
        cx.notify();
    }

    /// The mouse moved over the player: the controls and the cursor come back at once.
    pub fn player_mouse_moved(&mut self, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let now = Instant::now();
        playback.session.mouse_moved(now);
        if !playback.controls_shown {
            playback.session.update(now);
            playback.controls_shown = true;
            cx.notify();
        }
    }

    fn player_key(&mut self, action: KeyAction, window: &mut Window, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let request = playback.session.key(action, window.is_fullscreen());
        self.player_request(request, window, cx);
        cx.notify();
    }

    /// A double click on the player: on the picture, fullscreen comes or goes.
    pub fn player_double_click(&mut self, y: f32, window: &mut Window, cx: &mut Context<Self>) {
        let request = self
            .playback
            .as_ref()
            .and_then(|playback| playback.session.double_clicked(y));
        self.player_request(request, window, cx);
    }

    /// What the session asks of the window after a key or a click.
    pub fn player_request(
        &mut self,
        request: Option<WindowRequest>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match request {
            Some(WindowRequest::ToggleFullscreen | WindowRequest::ExitFullscreen) => {
                window.toggle_fullscreen();
            }
            Some(WindowRequest::Leave) => self.leave_player(false, window, cx),
            // The same end as the file's own: the next tick reports it and leaves.
            Some(WindowRequest::EndPlayback) => self.tick(window, cx),
            Some(WindowRequest::NextEpisode) => {
                self.tick(window, cx);
                self.play_up_next_now(window, cx);
            }
            Some(WindowRequest::PlayNext) => {
                let next = self
                    .playback
                    .as_mut()
                    .and_then(|playback| playback.next.take());
                if let Some(next) = next {
                    self.leave_to(Some(next), window, cx);
                    self.play_up_next_now(window, cx);
                }
            }
            Some(WindowRequest::Quality(quality)) => {
                let Some(playback) = &mut self.playback else {
                    return;
                };
                let reopen = playback
                    .fallback
                    .as_mut()
                    .and_then(|fallback| fallback.choose(quality, &mut playback.session));
                if let Some(reopen) = reopen {
                    self.reopen(reopen, false, cx);
                }
            }
            None => {}
        }
    }

    pub fn player_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_player(false, window, cx);
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.images.new_frame();
        self.cards.new_frame();
        self.scrolls.clear();
        if window.viewport_size() != self.viewport {
            self.viewport = window.viewport_size();
            for page in &mut self.details {
                page.menus.close();
            }
            for page in &mut self.series_pages {
                page.menus.close();
            }
            self.library.menus.close();
            self.series.menus.close();
            self.account_menu.close();
            self.close_playlist_menu(cx);
            self.settings_page.menus.close();
        }
        self.follow_playlist_menu(window);
        #[cfg(feature = "harness")]
        self.note_frame();
        #[cfg(any(test, feature = "harness"))]
        self.draws.borrow_mut().shell.push(Instant::now());
        // Escape needs the focus; a click on a button must not lose it for good. A text field
        // keeps it while it has it; the login's first field takes it when nothing has.
        let login_shown = self.login.is_some() && self.navigator.current() == Screen::Login;
        match focus_to(
            login_shown,
            &mut self.login_focus_due,
            window.focused(cx).is_none(),
        ) {
            FocusTo::Keep => {}
            // The palette keeps the keys while it is open.
            FocusTo::Root => match &self.search {
                Some(search) if search.open => window.focus(&search.input.focus_handle(cx), cx),
                _ => window.focus(&self.focus, cx),
            },
            FocusTo::Address => {
                if let Some(form) = &self.login {
                    window.focus(&form.address.focus_handle(cx), cx)
                }
            }
        }
        let screen = self.navigator.current();
        let back_label = self.navigator.back_label();
        // The menu belongs to the pill bar: it does not wait for the bar to come back.
        if !screen.shows_pill_bar() {
            self.account_menu.close();
        }
        let account = if screen.shows_pill_bar() {
            self.account_view(window)
        } else {
            None
        };
        let section = self.navigator.section();
        let now = Instant::now();
        let highlight = std::array::from_fn(|at| {
            let pill = &mut self.nav_pills[at];
            pill.set(NavEntry::ALL[at].is_current(section), now);
            if pill.moving(now) {
                window.request_animation_frame();
            }
            pill.level(now)
        });
        let shell = cx.weak_entity();
        let playing = screen == Screen::Player && self.playback.is_some();
        // The next episode playing keeps the fullscreen; anything else gives it back.
        match kept_fullscreen(
            self.fullscreen_kept,
            screen == Screen::Player,
            self.up_next.episode().is_some(),
            self.series_pages.last().is_some_and(SeriesPage::waiting)
                || self.playlist_pages.last().is_some_and(|page| page.busy),
        ) {
            KeptFullscreen::Keep => {}
            KeptFullscreen::Release => self.fullscreen_kept = false,
            KeptFullscreen::GiveBack => {
                self.fullscreen_kept = false;
                if window.is_fullscreen() {
                    window.toggle_fullscreen();
                }
            }
        }
        let page = (
            screen,
            self.details.len()
                + self.series_pages.len()
                + self.people.len()
                + usize::from(self.list.is_some())
                + self.collection_pages.len()
                + self.playlist_pages.len()
                + usize::from(self.navigator.current() == Screen::Favorites)
                + usize::from(screen == Screen::Settings)
                + usize::from(screen == Screen::Stats),
        );
        if page != self.page {
            // The ring was on a card of the page before, the help said the keys of that screen.
            self.ring = Ring::default();
            if self.help_open {
                self.close_help(cx);
            }
            self.turned_forward = page.1 > self.page.1;
            self.page = page;
            self.page_turns += 1;
        }
        let (turns, shift) = (
            self.page_turns,
            if self.turned_forward { 32. } else { -32. },
        );

        let help = screens::help::overlay(self, playing, window, cx);
        let again = self.ring_again(cx);
        let root = div()
            .id("root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context(self.key_context(screen));
        Self::answer_keys(root, cx)
            // The player leaves the root transparent: the video is under the window.
            .when(!playing, |root| root.bg(theme::background()))
            .text_color(theme::text())
            .text_size(px(14.))
            .when_some(self.font.clone(), |root, font| root.font_family(font))
            .child(match &mut self.playback {
                Some(playback) if playing => {
                    let now = Instant::now();
                    let session = &playback.session;
                    playback
                        .controls_fade
                        .set(session.controls_visible(now), now);
                    playback.panel_fade.set(session.panel_open(), now);
                    // A long list opens where the subtitle shown is. Asked at each frame of the
                    // fade in: GPUI drops the ask of the first, where the list has no place yet.
                    if session.panel_open() && playback.panel_fade.moving(now) {
                        playback.tracks_scroll.scroll_to_item(session.chosen_line());
                    }
                    playback.settings_fade.set(session.settings_open(), now);
                    let preview = playback.seek.draw(session.duration, now);
                    let skip = playback
                        .skip_fade
                        .follow(offered(session.skip_button(), preview.is_some()), now);
                    let notice = playback
                        .fallback
                        .as_ref()
                        .is_some_and(|f| f.notice_shown(session, now));
                    playback.notice_fade.set(notice, now);
                    let refusal = playback
                        .fallback
                        .as_ref()
                        .is_some_and(|f| f.refusal_shown(now));
                    playback.refusal_fade.set(refusal, now);
                    if playback.controls_fade.moving(now)
                        || playback.panel_fade.moving(now)
                        || playback.settings_fade.moving(now)
                        || playback.notice_fade.moving(now)
                        || playback.refusal_fade.moving(now)
                        || playback.skip_fade.moving(now)
                        || playback.seek.moving(now)
                    {
                        window.request_animation_frame();
                    }
                    let switch = &mut self.blackout_switch;
                    switch.set(self.settings.black_out_other_monitors, now);
                    if switch.moving(now) {
                        window.request_animation_frame();
                    }
                    let shown = screens::player::Shown {
                        controls: playback.controls_fade.level(now),
                        panel: playback.panel_fade.level(now),
                        preview,
                        notice: playback.notice_fade.level(now),
                        refusal: playback.refusal_fade.level(now),
                        blackout: self.screens.is_some().then(|| switch.level(now)),
                        skip,
                        settings: playback.settings_fade.level(now),
                        list: playback
                            .list
                            .as_ref()
                            .map(|list| (list.toggle.on(), list.error)),
                        add: playback.playing.clone(),
                        tracks_scroll: playback.tracks_scroll.clone(),
                    };
                    screens::player::render(session, shown, window, cx).into_any_element()
                }
                _ if self.starting => div().into_any_element(),
                // A page slides in from the side it comes from, and fades in.
                _ => div()
                    .id("page")
                    .size_full()
                    .relative()
                    .child(screens::render(self, screen, window, cx))
                    .with_animation(
                        ("page-turn", turns as usize),
                        Animation::new(motion::page()).with_easing(motion::ease_out),
                        move |page, t| page.opacity(t).left(px(shift * (1. - t))),
                    )
                    .into_any_element(),
            })
            .when(
                matches!(screen, Screen::SeriesDetail | Screen::Playlist),
                |root| root.children(screens::up_next_card(self, window, cx)),
            )
            // The side buttons of the mouse are heard over the whole window, the bar included,
            // which lets no click through to what is under it.
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            let MouseButton::Navigate(direction) = event.button else {
                                return;
                            };
                            if phase == DispatchPhase::Bubble {
                                let go = |shell: &mut Shell, cx: &mut Context<Shell>| {
                                    shell.navigate(direction, window, cx)
                                };
                                shell.update(cx, go).ok();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            // A light shade over the top of a backdrop, under the bar. It comes with the page.
            .when(screen.has_backdrop(), |root| {
                root.child(bar_fade().with_animation(
                    ("shade-turn", turns as usize),
                    Animation::new(motion::page()).with_easing(motion::ease_out),
                    |shade, t| shade.opacity(t),
                ))
            })
            .when(screen.shows_pill_bar(), |root| {
                root.child(pill_bar(
                    highlight,
                    account,
                    self.seerr.client.is_some(),
                    window,
                    cx,
                ))
            })
            .when_some(
                back_label.filter(|_| screen.shows_back_link()),
                |root, label| root.child(back_link(label, window, cx)),
            )
            .children(screens::playlist::add_menu(self, window, cx))
            .children(self.notice_pill(screen, window, cx))
            .children(screens::search::palette(self, window, cx))
            .children(help)
            .children(again)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_is_kept_through_auto_next_and_given_back_on_cancel() {
        use KeptFullscreen::*;
        // The episode ends in fullscreen, a card is offered: the window stays so.
        let kept = keeps_fullscreen(true, true);
        assert!(kept);
        assert_eq!(kept_fullscreen(kept, false, true, false), Keep);
        // The countdown ends, the next episode opens, then plays.
        assert_eq!(kept_fullscreen(kept, false, false, true), Keep);
        assert_eq!(kept_fullscreen(kept, true, false, false), Release);
        // Cancelled instead: given back.
        assert_eq!(kept_fullscreen(kept, false, false, false), GiveBack);
        // Left before the end, or not in fullscreen: nothing kept.
        assert!(!keeps_fullscreen(false, true));
        assert!(!keeps_fullscreen(true, false));
        assert_eq!(kept_fullscreen(false, false, false, false), Keep);
    }

    #[test]
    fn the_login_shown_takes_the_focus_the_root_had_taken_meanwhile() {
        let mut due = true;
        assert_eq!(focus_to(true, &mut due, false), FocusTo::Address);
    }

    #[test]
    fn the_address_takes_the_focus_once_only() {
        let mut due = true;
        assert_eq!(focus_to(true, &mut due, false), FocusTo::Address);
        assert_eq!(focus_to(true, &mut due, false), FocusTo::Keep);
    }

    #[test]
    fn a_field_the_user_went_to_keeps_the_focus() {
        let mut due = false;
        assert_eq!(focus_to(true, &mut due, false), FocusTo::Keep);
    }

    #[test]
    fn nothing_focused_on_the_login_gives_the_address_the_focus() {
        let mut due = false;
        assert_eq!(focus_to(true, &mut due, true), FocusTo::Address);
    }

    #[test]
    fn nothing_focused_elsewhere_gives_the_root_the_focus() {
        let mut due = false;
        assert_eq!(focus_to(false, &mut due, true), FocusTo::Root);
    }

    #[test]
    fn the_login_not_yet_built_leaves_the_focus_due() {
        let mut due = true;
        assert_eq!(focus_to(false, &mut due, true), FocusTo::Root);
        assert!(due);
    }
}
