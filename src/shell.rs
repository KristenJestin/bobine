//! The root view: the screen the navigator points at, framed by the pill bar or the back link.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::*;
use gpui_kit::{
    Context, FocusHandle, IntoElement, KeyDownEvent, Render, SharedString, Subscription, Task,
    Window, div, px,
};

use crate::chrome::{back_link, pill_bar};
use crate::navigation::{Navigator, Screen};
use crate::player::libmpv::Libmpv;
use crate::player::mpv::MpvPlayer;
use crate::player::session::{Session, WindowRequest};
use crate::player::{Media, Player};
use crate::screens;
use crate::theme;
use crate::video::Rect;
use crate::video::subsurface::{VideoSurface, Wayland};

/// How often the player is asked what changed, and the controls checked for hiding.
const PLAYER_TICK: Duration = Duration::from_millis(100);

pub struct Shell {
    pub navigator: Navigator,
    focus: FocusHandle,
    font: Option<SharedString>,
    pub playback: Option<Playback>,
    /// What the video binds of the compositor, once for this window: films reuse it.
    wayland: Option<Rc<RefCell<Wayland>>>,
}

/// The film being played and what keeps it in step with the window.
pub struct Playback {
    pub session: Session,
    controls_shown: bool,
    had_error: bool,
    _ticker: Task<()>,
    _bounds: Subscription,
}

/// What the player shows of the film above the seek bar.
pub struct Film {
    pub title: String,
    pub subtitle: Option<String>,
}

impl Shell {
    pub fn new(font: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            navigator: Navigator::new(),
            focus: cx.focus_handle(),
            font,
            playback: None,
            wayland: None,
        }
    }

    /// Opens the player on `media`. Its addresses go straight to mpv: nothing here keeps or
    /// prints them.
    #[allow(
        dead_code,
        reason = "the film page calls it once the server's streams reach it; the headless \
                  harness calls it today"
    )]
    pub fn play(
        &mut self,
        mpv: Arc<Libmpv>,
        media: Media<'_>,
        film: Film,
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
            controls_shown: false,
            had_error: false,
            _ticker: ticker,
            _bounds: bounds,
        });
        self.lay_out_video(window);
        self.navigator.open(Screen::Player);
        cx.notify();
        Ok(())
    }

    /// Closes the player: the video surface and mpv go before anything else does.
    pub fn stop_playback(&mut self) {
        self.playback = None;
    }

    /// The window is closing: the film goes, then what the video bound of the compositor,
    /// while GPUI's display is still connected.
    pub fn close_video(&mut self) {
        self.stop_playback();
        self.wayland = None;
    }

    fn leave_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.is_fullscreen() {
            window.toggle_fullscreen();
        }
        self.stop_playback();
        self.navigator.back();
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
        if playback.session.update(now) {
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
        // Escape needs the focus; a click on a button must not lose it for good.
        if !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let screen = self.navigator.current();
        let back_label = self.navigator.back_label();
        let playing = screen == Screen::Player && self.playback.is_some();

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
                } else if key == "escape" && app.navigator.back() {
                    cx.notify();
                }
            }))
            // The player leaves the root transparent: the video is under the window.
            .when(!playing, |root| root.bg(theme::background()))
            .text_color(theme::text())
            .text_size(px(14.))
            .when_some(self.font.clone(), |root, font| root.font_family(font))
            .child(match &self.playback {
                Some(playback) if playing => {
                    screens::player::render(&playback.session, window, cx).into_any_element()
                }
                _ => screens::render(screen, cx),
            })
            .when(screen.shows_pill_bar(), |root| {
                root.child(pill_bar(screen, cx))
            })
            .when_some(
                back_label.filter(|_| screen.shows_back_link()),
                |root, label| root.child(back_link(label, cx)),
            )
    }
}
