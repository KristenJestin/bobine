//! The root view: the screen the navigator points at, framed by the pill bar or the back link.

mod pages;

pub use pages::{Chip, DetailPage, Loadable, LoginForm, OpenConnector};

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    SharedString, Subscription, Task, Window, div, px,
};

use bobine::provider::{Connector, MediaServer};

use crate::chrome::{back_link, pill_bar};
use crate::images::Images;
use crate::motion::{self, Fade};
use crate::navigation::{Navigator, Screen};
use crate::player::libmpv::Libmpv;
use crate::player::mpv::MpvPlayer;
use crate::player::session::{Session, WindowRequest};
use crate::player::{Media, Player};
use crate::reports::{self, Reporting};
use crate::screens;
use crate::theme;
use crate::video::Rect;
use crate::video::subsurface::{VideoSurface, Wayland};

/// How often the player is asked what changed, and the controls checked for hiding.
const PLAYER_TICK: Duration = Duration::from_millis(100);

/// How long closing the window waits for "stopped" to reach the server.
const CLOSE_WAITS_FOR_REPORTS: Duration = Duration::from_secs(2);

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
    pub home: pages::HomePage,
    pub library: pages::LibraryPage,
    /// The film pages open, the last one on top: one per `Screen::FilmDetail` of the navigator.
    pub details: Vec<DetailPage>,
    pub images: Images,
    /// The wake-up planned for an image to ask again, and when.
    image_wake: Option<(Instant, Task<()>)>,
    /// The page shown at the last draw, and how many times it changed: each change slides the
    /// new page in, forwards (deeper) or backwards.
    page: (Screen, usize),
    page_turns: u64,
    turned_forward: bool,
    /// When each draw started: the harness reads the frame times of a move from it.
    #[cfg(feature = "harness")]
    pub frames: Vec<Instant>,
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
            home: Default::default(),
            library: Default::default(),
            details: Vec::new(),
            images: Images::default(),
            image_wake: None,
            page: (Screen::Login, 0),
            page_turns: 0,
            turned_forward: true,
            #[cfg(feature = "harness")]
            frames: Vec::new(),
            mpv_options: Vec::new(),
        };
        shell.begin(open, cx);
        shell
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

    fn leave_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        let reports = self.stop_playback();
        self.navigator.back();
        if let Some(reports) = reports {
            self.after_playback(reports, cx);
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
            self.leave_player(window, cx);
            return;
        }
        let shown = playback.session.controls_visible(now);
        let has_error = playback.session.error().is_some();
        // Hidden controls have nothing to redraw: the video is not GPUI's.
        if shown || shown != playback.controls_shown || has_error != playback.had_error {
            playback.controls_shown = shown;
            playback.had_error = has_error;
            cx.notify();
        }
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
        match playback
            .session
            .key(key, with_modifier, window.is_fullscreen())
        {
            Some(WindowRequest::ToggleFullscreen | WindowRequest::ExitFullscreen) => {
                window.toggle_fullscreen();
            }
            Some(WindowRequest::Leave) => self.leave_player(window, cx),
            None => {}
        }
        cx.notify();
    }

    pub fn player_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_player(window, cx);
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.images.new_frame();
        #[cfg(feature = "harness")]
        self.frames.push(Instant::now());
        // Escape needs the focus; a click on a button must not lose it for good. A text field
        // keeps it while it has it; the login's first field takes it when nothing has.
        if window.focused(cx).is_none() {
            match &self.login {
                Some(form) if self.navigator.current() == Screen::Login => {
                    window.focus(&form.address.focus_handle(cx), cx)
                }
                _ => window.focus(&self.focus, cx),
            }
        }
        let screen = self.navigator.current();
        let back_label = self.navigator.back_label();
        let playing = screen == Screen::Player && self.playback.is_some();
        let page = (screen, self.details.len());
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
                    if playback.controls_fade.moving(now) || playback.panel_fade.moving(now) {
                        window.request_animation_frame();
                    }
                    let shown = screens::player::Shown {
                        controls: playback.controls_fade.level(now),
                        panel: playback.panel_fade.level(now),
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
            .when(screen.shows_pill_bar(), |root| {
                root.child(pill_bar(screen, cx))
            })
            .when_some(
                back_label.filter(|_| screen.shows_back_link()),
                |root, label| root.child(back_link(label, window, cx)),
            )
    }
}
