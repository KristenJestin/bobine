//! The root view: the screen the navigator points at, framed by the pill bar or the back link.

mod home;
mod lists;
mod pages;
mod palette;
mod preview;
mod series;
mod skip;

pub use home::{HomePage, ListPage};
pub use lists::{CollectionPage, CollectionsPage, MyListPage};
pub use pages::{Chip, DetailPage, GONE, LibraryPage, Loadable, LoginForm, OpenConnector, Tool};
pub use palette::{PersonPage, SearchState};
pub use series::SeriesPage;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, Context, Entity, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    Pixels, Render, SharedString, Size, Subscription, Task, Window, div, px,
};

use bobine::provider::{
    Connector, Episode, Film as FilmItem, MediaServer, Playback as Opened, ProviderError, Series,
    TranscodeRequest,
};

use crate::chrome::{back_link, pill_bar};
use crate::fallback::Fallback;
use crate::images::Images;
use crate::motion::{self, Fade, Presence};
use crate::navigation::{Navigator, Screen};
use crate::player::libmpv::Libmpv;
use crate::player::mpv::MpvPlayer;
use crate::player::segments::{Button, Label};
use crate::player::session::{Session, WindowRequest};
use crate::player::{Media, Player};
use crate::reports::{self, Reporting};
use crate::screens;
use crate::search;
use crate::series_page::{self, UpNext};
use crate::theme;
use crate::video::Rect;
use crate::video::subsurface::{VideoSurface, Wayland};
use preview::SeekPreview;

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
    /// The kept session is being looked for: nothing to show yet.
    pub starting: bool,
    connector: Option<Arc<dyn Connector>>,
    server: Option<Arc<dyn MediaServer>>,
    pub login: Option<LoginForm>,
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
    /// The "Épisode suivant" card over the series page, after an episode ended.
    pub up_next: UpNext,
    up_next_ticker: Option<Task<()>>,
    /// The numbers of the series pages' refreshes, counted over every page: an answer on its way
    /// for a page that closed never matches the page that opens in its place.
    refreshes_asked: u64,
    /// The film and series pages opened so far: each takes the next number, so that an answer on
    /// its way for a page that closed never reaches the page that opens in its place.
    pages_opened: u64,
    /// The card fades in and out; the episode it showed stays drawn while it fades out.
    pub up_next_fade: Presence<Episode>,
    /// The episode ended in fullscreen: the window stays so while the card counts down, and
    /// leaves it if the next episode does not play.
    fullscreen_kept: bool,
    /// The person pages open, the last one on top: one per `Screen::Person` of the navigator.
    pub people: Vec<PersonPage>,
    pub collections: CollectionsPage,
    /// The collection pages open, the last one on top: one per `Screen::CollectionDetail`.
    pub collection_pages: Vec<CollectionPage>,
    pub my_list: MyListPage,
    /// The search palette, while it shows (it stays fading out after it closed).
    pub search: Option<SearchState>,
    pub images: Images,
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
    /// Given to every mpv: the headless harness silences the sound.
    pub mpv_options: Vec<(&'static str, &'static str)>,
}

/// The film being played and what keeps it in step with the window.
pub struct Playback {
    pub session: Session,
    /// The controls and the panel fade in and out.
    controls_fade: Fade,
    panel_fade: Fade,
    controls_shown: bool,
    had_error: bool,
    /// What the server is told; `None` for a local file.
    reporting: Option<Reporting>,
    /// The episode after the one playing: the card offers it at the end.
    next: Option<Episode>,
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
    _ticker: Task<()>,
    _bounds: Subscription,
}

/// What the player shows of the film above the seek bar.
pub struct Film {
    pub title: String,
    pub subtitle: Option<String>,
}

impl Shell {
    pub fn new(font: Option<SharedString>, open: OpenConnector, cx: &mut Context<Self>) -> Self {
        let this = cx.weak_entity();
        let hero_view = cx.new(|_| screens::HeroView::new(this));
        let mut shell = Self {
            navigator: Navigator::new(),
            focus: cx.focus_handle(),
            font,
            playback: None,
            wayland: None,
            starting: true,
            connector: None,
            server: None,
            login: None,
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
            search: None,
            images: Images::default(),
            image_wake: None,
            page: (Screen::Login, 0),
            page_turns: 0,
            turned_forward: true,
            viewport: Size::default(),
            #[cfg(feature = "harness")]
            frames: Default::default(),
            mpv_options: Vec::new(),
        };
        shell.begin(open, cx);
        shell
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
        let wayland = match &self.wayland {
            Some(wayland) => wayland.clone(),
            None => self
                .wayland
                .insert(Rc::new(RefCell::new(Wayland::new(window)?)))
                .clone(),
        };
        let display = VideoSurface::new(wayland, mpv.clone())?;
        let mut player = MpvPlayer::new(mpv);
        player.load(media).map_err(|e| e.to_string())?;
        let session = Session::new(
            Box::new(player),
            Box::new(display),
            film.title,
            film.subtitle,
        );
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
        self.playback = Some(Playback {
            session,
            controls_fade: Fade::new(motion::quick()),
            panel_fade: Fade::new(motion::quick()),
            controls_shown: false,
            had_error: false,
            reporting,
            next: None,
            generation: 0,
            skip_fade: Presence::new(motion::quick()),
            skip_label: None,
            seek: SeekPreview::new(),
            fallback: None,
            notice_fade: Fade::new(motion::quick()),
            notice_shown: false,
            _ticker: ticker,
            _bounds: bounds,
        });
        self.lay_out_video(window);
        self.navigator.open(Screen::Player);
        cx.notify();
        Ok(())
    }

    /// Closes the player: the video surface and mpv go before anything else does. The server is
    /// told the playback stopped; the handle ends once it has been.
    pub fn stop_playback(&mut self) -> Option<std::thread::JoinHandle<()>> {
        let Playback {
            session, reporting, ..
        } = self.playback.take()?;
        let reports = reporting.map(|reporting| reporting.finish(&session));
        drop(session);
        reports
    }

    /// The window is closing: the film goes, the server is told where it stopped (a moment
    /// is waited for it, no more), then what the video bound of the compositor goes, while
    /// GPUI's display is still connected.
    pub fn close_video(&mut self) {
        if let Some(reports) = self.stop_playback() {
            reports::wait(reports, CLOSE_WAITS_FOR_REPORTS);
        }
        self.wayland = None;
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
        self.fullscreen_kept = keeps_fullscreen(next.is_some(), window.is_fullscreen());
        if window.is_fullscreen() && !self.fullscreen_kept {
            window.toggle_fullscreen();
        }
        self.release_tiles(window, cx);
        let reports = self.stop_playback();
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
            self.convert(request, cx);
        }
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
        let skip_label = playback.session.skip_button().map(|button| button.label);
        // Hidden controls have nothing to redraw: the video is not GPUI's.
        if shown
            || shown != playback.controls_shown
            || has_error != playback.had_error
            || notice != playback.notice_shown
            || skip_label != playback.skip_label
        {
            playback.controls_shown = shown;
            playback.had_error = has_error;
            playback.notice_shown = notice;
            playback.skip_label = skip_label;
            cx.notify();
        }
    }

    /// The film of the server does not play as it is: the server is asked to convert it. The
    /// playback that did not play is told stopped first.
    fn convert(&mut self, request: TranscodeRequest, cx: &mut Context<Self>) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let Some(fallback) = &playback.fallback else {
            return;
        };
        if let Some(reporting) = playback.reporting.take() {
            drop(reporting.finish(&playback.session));
        }
        let (server, id) = (fallback.server.clone(), fallback.id());
        let task = cx
            .background_executor()
            .spawn(async move { server.transcode(&request) });
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
        if let Some(report) = fallback.answered(id, &mut playback.session, opened) {
            playback.reporting = Some(Reporting::start(fallback.server.clone(), report));
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

    fn player_key(
        &mut self,
        key: &str,
        with_modifier: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(playback) = &mut self.playback else {
            return;
        };
        let request = playback
            .session
            .key(key, with_modifier, window.is_fullscreen());
        self.player_request(request, window, cx);
        cx.notify();
    }

    /// What the session asks of the window after a key or a click.
    fn player_request(
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
        }
        #[cfg(feature = "harness")]
        self.note_frame();
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
        let playing = screen == Screen::Player && self.playback.is_some();
        // The next episode playing keeps the fullscreen; anything else gives it back.
        match kept_fullscreen(
            self.fullscreen_kept,
            screen == Screen::Player,
            self.up_next.episode().is_some(),
            self.series_pages.last().is_some_and(SeriesPage::waiting),
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
                + self.collection_pages.len(),
        );
        if page != self.page {
            self.turned_forward = page.1 > self.page.1;
            self.page = page;
            self.page_turns += 1;
        }
        let (turns, shift) = (
            self.page_turns,
            if self.turned_forward { 32. } else { -32. },
        );

        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                let held = &event.keystroke.modifiers;
                let others = held.alt || held.shift || held.platform || held.function;
                if search::is_shortcut(key, held.control, others) && app.open_search(window, cx) {
                    return;
                }
                if app.search_is_open() {
                    app.search_key(key, window, cx);
                    return;
                }
                if app.navigator.current() == Screen::Player && app.playback.is_some() {
                    let held = &event.keystroke.modifiers;
                    let with_modifier = held.control || held.alt || held.platform || held.function;
                    app.player_key(key, with_modifier, window, cx);
                } else if key == "escape" {
                    app.escape(cx);
                }
            }))
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
                    let skip = playback.skip_fade.follow(session.skip_button(), now);
                    let preview = playback.seek.draw(session.duration, now);
                    let notice = playback
                        .fallback
                        .as_ref()
                        .is_some_and(|f| f.notice_shown(session, now));
                    playback.notice_fade.set(notice, now);
                    if playback.controls_fade.moving(now)
                        || playback.panel_fade.moving(now)
                        || playback.notice_fade.moving(now)
                        || playback.skip_fade.moving(now)
                        || playback.seek.moving(now)
                    {
                        window.request_animation_frame();
                    }
                    let shown = screens::player::Shown {
                        controls: playback.controls_fade.level(now),
                        panel: playback.panel_fade.level(now),
                        preview,
                        notice: playback.notice_fade.level(now),
                        skip,
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
            .when(screen == Screen::SeriesDetail, |root| {
                root.children(screens::up_next_card(self, window, cx))
            })
            .when(screen.shows_pill_bar(), |root| {
                root.child(pill_bar(screen, window, cx))
            })
            .when_some(
                back_label.filter(|_| screen.shows_back_link()),
                |root, label| root.child(back_link(label, window, cx)),
            )
            .children(screens::search::palette(self, window, cx))
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
