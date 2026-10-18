//! The player, after Infuse: nothing over the picture until the mouse moves, then the title top
//! left, the seek bar and its buttons at the bottom; the audio and subtitles panel floats over
//! the video at the right. GPUI paints nothing else: the video shows through, from under the
//! window.

use bobine::provider::ItemId;
use gpui_kit::prelude::*;
use std::sync::Arc;

use gpui_kit::{
    App, ClickEvent, Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ObjectFit, RenderImage, Rgba, ScrollHandle, Stateful, Window,
    div, img, linear_color_stop, linear_gradient, px, rgb, rgba,
};

use super::empty_screen;
use crate::icons::{IconName, icon};
use crate::motion;
use crate::player::Aspect;
use crate::player::controls::{BOTTOM_CONTROLS_HEIGHT, SKIP_SECONDS, TOP_CONTROLS_HEIGHT};
use crate::player::segments::Button;
use crate::player::session::{Choice, Session};
use crate::player::settings::{self, Page, Quality, SPEEDS};
use crate::player::thumbnails::Crop;
use crate::player::time;
use crate::shell::Shell;
use crate::theme;

/// How wide each side of the buttons row is, so that the middle buttons sit in the middle.
const SIDE_WIDTH: f32 = 320.;
/// The margin of the controls from the edges of the window, after the design.
pub const EDGE: f32 = 40.;
/// How wide the box of a time alone is, so that the time sits centred under the pointer.
pub const TIME_BOX_WIDTH: f32 = 76.;
/// The preview floats this far above the controls.
const PREVIEW_GAP: f32 = 14.;
/// The "Black out other screens" switch: its track, and the gap around its knob.
const SWITCH_WIDTH: f32 = 34.;
const SWITCH_HEIGHT: f32 = 20.;
const SWITCH_GAP: f32 = 2.;
/// The settings menu: its width, one of its lines, and the room between it and the seek bar.
const MENU_WIDTH: f32 = 300.;
const MENU_LINE: f32 = 34.;
const MENU_GAP: f32 = 8.;
/// The room the track panel leaves free under it.
const PANEL_TOP: f32 = 96.;

/// The player screen with no film to play.
pub fn empty(_cx: &mut Context<Shell>) -> impl IntoElement {
    empty_screen("Player")
}

/// How much of the controls and of the panel shows, from 0 to 1: they fade in and out.
pub struct Shown {
    pub controls: f32,
    pub panel: f32,
    /// The skip button and how much of it shows.
    pub skip: Option<(Button, f32)>,
    /// "Conversion par le serveur".
    pub notice: f32,
    /// "This quality could not be started."
    pub refusal: f32,
    /// Where the "Black out other screens" switch is, from off (0) to on (1); `None` when the
    /// compositor cannot cover a monitor, and the switch is not offered.
    pub blackout: Option<f32>,
    /// The settings menu.
    pub settings: f32,
    /// The "My list" button: whether the title is in the list, and why the last press failed;
    /// `None` for a file of no server.
    pub list: Option<(bool, Option<&'static str>)>,
    /// What "Add to playlist" puts in a playlist: the film or the episode playing; `None` for a
    /// file of no server.
    pub add: Option<ItemId>,
    /// The tracks of the panel scroll on this.
    pub tracks_scroll: ScrollHandle,
    /// What floats over the seek bar where the pointer is.
    pub preview: Option<Preview>,
}

/// The time under the pointer on the seek bar, and the thumbnail of that moment if the server
/// has them.
pub struct Preview {
    /// How much of it shows, from 0 to 1.
    pub opacity: f32,
    /// Where the box starts, from the left of the window.
    pub left: f32,
    pub time: String,
    /// The size of the thumbnail's frame, when the film has thumbnails at all.
    pub frame: Option<(f32, f32)>,
    /// The thumbnail in the frame and how much of it shows; none while its tile is not here.
    pub thumb: Option<(Thumb, f32)>,
}

/// A tile's picture and where in it the thumbnail is.
#[derive(Clone)]
pub struct Thumb {
    pub image: Arc<RenderImage>,
    pub crop: Crop,
    /// The size the whole tile picture is drawn at.
    pub picture: (f32, f32),
}

pub fn render(
    session: &Session,
    shown: Shown,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let controls = (shown.controls > 0.).then(|| {
        div()
            .absolute()
            .inset_0()
            .opacity(shown.controls)
            .child(shade(true))
            .child(shade(false))
            .child(title_bar(session, shown.list, shown.add, window, cx))
            .child(bottom_bar(session, window, cx))
            .children(shown.preview.map(preview))
    });
    // Only an open panel or menu closes on a click outside; one fading out is just drawn.
    let outside = (session.panel_open() || session.settings_open()).then(|| outside_panel(cx));
    let panel = (shown.panel > 0.).then(|| {
        panel(
            session,
            shown.panel,
            shown.blackout,
            &shown.tracks_scroll,
            window,
            cx,
        )
    });
    let menu = (shown.settings > 0.).then(|| settings_menu(session, shown.settings, window, cx));
    let conversion = (shown.notice > 0.).then(|| notice("Converted by the server", shown.notice));
    let refusal =
        (shown.refusal > 0.).then(|| notice("This quality could not be started.", shown.refusal));
    div()
        .id("player")
        .size_full()
        .relative()
        .on_mouse_move(cx.listener(|shell, event: &MouseMoveEvent, window, cx| {
            shell.player_mouse_moved(cx);
            shell.seek_drag(
                f32::from(event.position.x),
                event.pressed_button == Some(MouseButton::Left),
                window,
                cx,
            );
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|shell, _: &MouseUpEvent, _, cx| shell.seek_release(cx)),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|shell, event: &MouseDownEvent, window, cx| {
                // A button or a box over the picture keeps its clicks.
                if event.click_count == 2 && !window.default_prevented() {
                    shell.player_double_click(f32::from(event.position.y), window, cx);
                }
            }),
        )
        .children(controls)
        .children(conversion)
        .children(refusal)
        .children(
            shown.skip.map(|(button, level)| {
                super::skip::button(button, level, shown.controls, window, cx)
            }),
        )
        .children(outside)
        .children(panel)
        .children(menu)
        .when_some(session.error(), |root, error| {
            root.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(keeps_clicks(
                        div()
                            .id("player-error")
                            .px(px(20.))
                            .py(px(14.))
                            .rounded(px(theme::RADIUS_LARGE))
                            .bg(theme::panel())
                            .child(format!("Cannot play: {error}")),
                    )),
            )
        })
}

/// A double click on `element` is not one on the picture: fullscreen stays as it is.
pub fn keeps_clicks<E: InteractiveElement>(element: E) -> E {
    element.on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
}

/// One line at the top, over the picture whether the controls show or not: the server
/// converts the film, which the viewer may notice in its picture or its start, or would not
/// give the quality chosen.
fn notice(text: &'static str, shown: f32) -> Div {
    div()
        .absolute()
        .top(px(30.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .opacity(shown)
        .child(
            div()
                .px(px(16.))
                .py(px(8.))
                .rounded(px(theme::RADIUS_LARGE))
                .bg(theme::panel())
                .text_size(px(13.))
                .child(text),
        )
}

/// The darkening behind the controls, top or bottom, so they read over a bright picture.
fn shade(top: bool) -> Div {
    let (dark, clear) = (rgba(0x000000a6), rgba(0x00000000));
    let shade = div().absolute().left_0().right_0();
    if top {
        shade.top_0().h(px(180.)).bg(linear_gradient(
            180.,
            linear_color_stop(dark, 0.),
            linear_color_stop(clear, 1.),
        ))
    } else {
        shade.bottom_0().h(px(240.)).bg(linear_gradient(
            0.,
            linear_color_stop(rgba(0x000000cc), 0.),
            linear_color_stop(clear, 1.),
        ))
    }
}

fn title_bar(
    session: &Session,
    list: Option<(bool, Option<&'static str>)>,
    add: Option<ItemId>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let back = round_button("player-back", 36., window, cx)
        .child(icon(IconName::ArrowLeft, 20.))
        .on_click(cx.listener(|shell, _, window, cx| {
            shell.player_back(window, cx);
        }));
    let tracks = text_button("player-tracks", "Audio and subtitles", window, cx)
        .when(session.panel_open(), |button| button.bg(theme::line()))
        .on_click(cx.listener(|shell, _, _, cx| {
            if let Some(playback) = &mut shell.playback {
                playback.session.toggle_panel();
            }
            cx.notify();
        }));
    let list = list.map(|(on, error)| {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .children(error.map(|error| {
                div()
                    .text_size(px(13.))
                    .text_color(theme::muted())
                    .child(error)
            }))
            .child(super::film_detail::list_button(
                "player-list",
                on,
                window,
                cx,
            ))
    });
    div()
        .absolute()
        .top(px(30.))
        .left(px(EDGE))
        .right(px(EDGE))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div().flex().items_center().gap(px(16.)).child(back).child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(17.))
                            .font_weight(FontWeight::BOLD)
                            .child(session.title.clone()),
                    )
                    .when_some(session.subtitle.clone(), |column, subtitle| {
                        column.child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(subtitle),
                        )
                    }),
            ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .children(list)
                .children(add.map(|item| {
                    super::playlist::playlist_button("player-playlist", item, false, window, cx)
                }))
                .child(tracks),
        )
}

fn bottom_bar(session: &Session, window: &mut Window, cx: &mut Context<Shell>) -> Div {
    let played = if session.duration > 0. {
        (session.position / session.duration).clamp(0., 1.) as f32
    } else {
        0.
    };
    let left = (session.duration - session.position).max(0.);
    let bar_width = f32::from(window.viewport_size().width) - 2. * EDGE;
    let skip = |seconds: f64| {
        move |shell: &mut Shell, _: &ClickEvent, _: &mut Window, cx: &mut Context<Shell>| {
            if let Some(playback) = &mut shell.playback {
                playback.session.skip(seconds);
            }
            cx.notify();
        }
    };
    let back_10 = text_button("player-back-10", "−10s", window, cx)
        .on_click(cx.listener(skip(-SKIP_SECONDS)));
    let play = play_pause(session.paused, window, cx).on_click(cx.listener(|shell, _, _, cx| {
        if let Some(playback) = &mut shell.playback {
            playback.session.toggle_pause();
        }
        cx.notify();
    }));
    let forward_10 = text_button("player-forward-10", "+10s", window, cx)
        .on_click(cx.listener(skip(SKIP_SECONDS)));
    let gear = round_button("player-settings", 36., window, cx)
        .when(session.settings_open(), |button| button.bg(theme::line()))
        .child(icon(IconName::Gear, 20.))
        .on_click(cx.listener(|shell, _, _, cx| {
            if let Some(playback) = &mut shell.playback {
                playback.session.toggle_settings();
            }
            cx.notify();
        }));
    let fullscreen = round_button("player-fullscreen", 36., window, cx)
        .child(icon(
            if window.is_fullscreen() {
                IconName::CornersIn
            } else {
                IconName::CornersOut
            },
            20.,
        ))
        .on_click(cx.listener(|shell, _, window, cx| {
            let request = shell
                .playback
                .as_ref()
                .and_then(|playback| playback.session.press_fullscreen());
            shell.player_request(request, window, cx);
        }));
    // The segments the server marked: lighter stretches of the bar, cut from the rest by a
    // notch at each end that shows over the played part too.
    let marks = session.segment_marks();
    let notches: Vec<f32> = marks
        .iter()
        .flat_map(|(start, width)| [*start, start + width])
        .filter(|at| *at > 0. && *at < 1.)
        .collect();

    div()
        .absolute()
        .left(px(EDGE))
        .right(px(EDGE))
        .bottom(px(EDGE))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            // A taller strip than the bar, so it is easy to hit.
            div()
                .id("player-seek")
                .h(px(15.))
                .flex()
                .items_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|shell, event: &MouseDownEvent, window, cx| {
                        shell.seek_press(f32::from(event.position.x), window, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|shell, event: &MouseMoveEvent, window, cx| {
                    shell.seek_hover(f32::from(event.position.x), window, cx);
                }))
                .on_hover(cx.listener(|shell, hovered: &bool, _, cx| {
                    if !*hovered {
                        shell.seek_left(cx);
                    }
                }))
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(5.))
                        .rounded(px(3.))
                        .bg(rgba(0xffffff33))
                        .children(marks.iter().map(|(start, width)| {
                            div()
                                .absolute()
                                .top_0()
                                .h_full()
                                .left(gpui_kit::relative(*start))
                                .w(gpui_kit::relative(*width))
                                .bg(rgba(0xffffff66))
                        }))
                        .child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative(played))
                                .rounded(px(3.))
                                .bg(rgb(0xffffff)),
                        )
                        .children(notches.into_iter().map(|at| {
                            div()
                                .absolute()
                                .top_0()
                                .h_full()
                                .left(px(at * bar_width - 1.))
                                .w(px(2.))
                                .bg(rgba(0x000000b3))
                        }))
                        .child(
                            div()
                                .absolute()
                                .top(px(-5.))
                                .left(px(played * bar_width - 7.5))
                                .size(px(15.))
                                .rounded_full()
                                .bg(rgb(0xffffff)),
                        ),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .text_size(px(13.))
                .child(div().w(px(SIDE_WIDTH)).child(time::clock(session.position)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(28.))
                        .child(back_10)
                        .child(play)
                        .child(forward_10),
                )
                .child(
                    div()
                        .w(px(SIDE_WIDTH))
                        .flex()
                        .items_center()
                        .justify_end()
                        .gap(px(8.))
                        .child(div().mr(px(8.)).text_color(theme::muted()).child(format!(
                            "{} · {}",
                            time::remaining(left),
                            time::end_clock(time::local_now(), left)
                        )))
                        .child(gear)
                        .child(fullscreen),
                ),
        )
}

/// Over the seek bar, where the pointer is: the thumbnail of that moment when there is one, with
/// the time under it. It catches no mouse event.
fn preview(preview: Preview) -> Div {
    let Preview {
        opacity,
        left,
        time,
        frame,
        thumb,
    } = preview;
    let width = frame.map_or(TIME_BOX_WIDTH, |(width, _)| width);
    let framed = frame.is_some();
    let frame = frame.map(|(width, height)| {
        div()
            .relative()
            .w(px(width))
            .h(px(height))
            .overflow_hidden()
            .bg(rgba(0x0e0e10e0))
            .border_1()
            .border_color(theme::line())
            .children(thumb.map(|(thumb, shown)| {
                div().absolute().inset_0().opacity(shown).child(
                    img(thumb.image)
                        .absolute()
                        .left(px(-thumb.crop.left))
                        .top(px(-thumb.crop.top))
                        .w(px(thumb.picture.0))
                        .h(px(thumb.picture.1))
                        .object_fit(ObjectFit::Fill),
                )
            }))
    });
    div()
        .absolute()
        .left(px(left))
        .bottom(px(BOTTOM_CONTROLS_HEIGHT + PREVIEW_GAP))
        .w(px(width))
        .opacity(opacity)
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .children(frame)
        .child(
            div()
                .px(px(10.))
                .py(px(4.))
                // Under the square frame the label is square too: GPUI clips to a rectangle,
                // and the picture is a part of a bigger one, so the frame cannot be rounded.
                .map(|label| if framed { label } else { label.rounded_full() })
                .bg(rgba(0x0e0e10e0))
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(time),
        )
}

/// White, round: the pause icon while the film plays, the play icon when it is paused.
fn play_pause(paused: bool, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    let shades = theme::Shades {
        rest: rgb(0xffffff),
        hover: rgb(0xe4e4e7),
        press: rgb(0xd4d4d8),
    };
    round(theme::eased("player-play-pause", shades, window, cx), 52.)
        .text_color(rgb(0x000000))
        .child(icon(
            if paused {
                IconName::Play
            } else {
                IconName::Pause
            },
            22.,
        ))
}

fn round_button(id: &'static str, side: f32, window: &mut Window, cx: &mut App) -> Stateful<Div> {
    round(theme::eased(id, theme::Shades::veil(), window, cx), side)
}

fn round(button: Stateful<Div>, side: f32) -> Stateful<Div> {
    keeps_clicks(button)
        .size(px(side))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
}

fn text_button(
    id: &'static str,
    label: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    keeps_clicks(theme::eased(id, theme::Shades::veil(), window, cx))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(theme::RADIUS))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(label)
}

/// Under the open panel, over everything else: a click there closes the panel and does nothing
/// more, even on the button that opened it.
fn outside_panel(cx: &mut Context<Shell>) -> Stateful<Div> {
    div()
        .id("player-outside-panel")
        .absolute()
        .inset_0()
        .on_any_mouse_down(cx.listener(|shell, _: &MouseDownEvent, _, cx| {
            if let Some(playback) = &mut shell.playback {
                playback.session.clicked_outside_panel();
            }
            cx.stop_propagation();
            cx.notify();
        }))
}

/// Over the video at the right: the picture keeps its whole size under it. It slides in a little
/// from the right as it fades in, `shown` of the way. A list taller than the window scrolls.
fn panel(
    session: &Session,
    shown: f32,
    blackout: Option<f32>,
    scroll: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    // Fading out, the panel is only drawn: it catches no click.
    let open = session.panel_open();
    let audio = choices(
        "audio",
        session.audio_choices(),
        open,
        window,
        cx,
        |shell, id| {
            if let (Some(playback), Some(id)) = (&mut shell.playback, id) {
                playback.session.select_audio(id);
            }
        },
    );
    let subtitles = choices(
        "subtitle",
        session.subtitle_choices(),
        open,
        window,
        cx,
        |shell, id| {
            if let Some(playback) = &mut shell.playback {
                playback.session.select_subtitle(id);
            }
        },
    );
    let height = f32::from(window.viewport_size().height);
    // One line after another, the titles included: `Session::chosen_line` counts them so.
    let tracks = div()
        .id("player-tracks-list")
        .track_scroll(scroll)
        .overflow_y_scroll()
        // Shrinks, as flex items do, down to what the panel leaves it.
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(14.))
        .child(section_title("AUDIO").mb(px(8.)))
        .children(audio)
        .child(section_title("SUBTITLES").mt(px(22.)).mb(px(8.)))
        .children(subtitles);
    div()
        .id("player-panel")
        .absolute()
        .top(px(PANEL_TOP))
        .right(px(EDGE - 16. * (1. - shown)))
        .opacity(shown)
        .w(px(340.))
        .max_h(px(panel_height(height)))
        .flex()
        .flex_col()
        .pt(px(22.))
        .px(px(20.))
        .pb(px(14.))
        .rounded(px(theme::RADIUS_LARGE))
        .border_1()
        .border_color(theme::line())
        // No blur behind it in GPUI: opaque enough to read over any picture.
        .bg(rgba(0x0e0e10e0))
        // A click in the panel never reaches the video's area.
        .when(open, |panel| panel.occlude())
        .child(tracks)
        .when_some(blackout, |panel, on| {
            panel.child(blackout_switch(on, open, window, cx))
        })
}

/// The most the panel may take of a window `window_height` high: down to the bottom margin.
fn panel_height(window_height: f32) -> f32 {
    (window_height - PANEL_TOP - EDGE).max(0.)
}

/// What a line of the menu does when clicked.
type Act = fn(&mut Shell, &mut Window, &mut Context<Shell>);

/// The settings menu, over the right of the seek bar, under its gear: the list of settings, or
/// the one opened. It fades in rising a little toward its place, `shown` of the way, like the
/// chip menus; only an open menu answers the mouse.
fn settings_menu(
    session: &Session,
    shown: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let open = session.settings_open();
    let lines: Vec<Stateful<Div>> = match session.settings_page() {
        Page::Main => main_page(session, open, window, cx),
        Page::Speed => {
            let mut lines = vec![back_line("Playback speed", open, window, cx)];
            lines.extend(SPEEDS.iter().enumerate().map(|(index, speed)| {
                let speed = *speed;
                choice_line(
                    ("speed", index),
                    settings::speed_label(speed),
                    session.speed() == speed,
                    open,
                    window,
                    cx,
                    move |shell, _, cx| {
                        if let Some(playback) = &mut shell.playback {
                            playback.session.set_speed(speed);
                        }
                        cx.notify();
                    },
                )
            }));
            lines
        }
        Page::Aspect => {
            let mut lines = vec![back_line("Aspect ratio", open, window, cx)];
            lines.extend(Aspect::ALL.iter().enumerate().map(|(index, aspect)| {
                let aspect = *aspect;
                choice_line(
                    ("aspect", index),
                    aspect.label().to_string(),
                    session.aspect() == aspect,
                    open,
                    window,
                    cx,
                    move |shell, _, cx| {
                        if let Some(playback) = &mut shell.playback {
                            playback.session.set_aspect(aspect);
                        }
                        cx.notify();
                    },
                )
            }));
            lines
        }
        Page::Quality => {
            let mut lines = vec![back_line("Quality", open, window, cx)];
            lines.extend(Quality::ALL.iter().enumerate().map(|(index, quality)| {
                let quality = *quality;
                choice_line(
                    ("quality", index),
                    session.quality_label(quality),
                    session.quality() == quality,
                    open,
                    window,
                    cx,
                    move |shell, window, cx| {
                        let request = shell.playback.as_mut().and_then(|playback| {
                            playback.session.clicked_outside_panel();
                            playback.session.choose_quality(quality)
                        });
                        shell.player_request(request, window, cx);
                        cx.notify();
                    },
                )
            }));
            lines
        }
        Page::Info => {
            let mut lines = vec![back_line("Playback info", open, window, cx)];
            lines.extend(session.playback_info().into_iter().enumerate().map(
                |(index, (label, value))| {
                    div()
                        .id(("info", index))
                        .h(px(MENU_LINE - 6.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(16.))
                        .child(div().text_color(theme::muted()).child(label))
                        .child(value)
                },
            ));
            lines
        }
    };
    let height = f32::from(window.viewport_size().height);
    let bottom = BOTTOM_CONTROLS_HEIGHT + MENU_GAP;
    div()
        .id("player-settings-menu")
        .absolute()
        .right(px(EDGE))
        .bottom(px(bottom - 8. * (1. - shown)))
        .opacity(shown)
        .w(px(MENU_WIDTH))
        .max_h(px((height - bottom - TOP_CONTROLS_HEIGHT).max(0.)))
        .overflow_y_scroll()
        .p(px(6.))
        .flex()
        .flex_col()
        .rounded(px(theme::RADIUS_LARGE))
        .border_1()
        .border_color(theme::line())
        .bg(theme::panel())
        .text_size(px(13.))
        .when(open, |menu| menu.occlude())
        .children(lines)
}

/// The list of settings, each with what it is set to.
fn main_page(
    session: &Session,
    open: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Vec<Stateful<Div>> {
    let page = |page: Page| -> Act {
        match page {
            Page::Speed => |shell, _, cx| show_page(shell, Page::Speed, cx),
            Page::Aspect => |shell, _, cx| show_page(shell, Page::Aspect, cx),
            Page::Quality => |shell, _, cx| show_page(shell, Page::Quality, cx),
            Page::Info => |shell, _, cx| show_page(shell, Page::Info, cx),
            Page::Main => |shell, _, cx| show_page(shell, Page::Main, cx),
        }
    };
    let mut lines = vec![
        setting_line(
            "Playback speed",
            Some(settings::speed_label(session.speed())),
            open,
            page(Page::Speed),
            window,
            cx,
        ),
        offset_line(session.subtitle_delay(), open, window, cx),
        setting_line(
            "Aspect ratio",
            Some(session.aspect().label().to_string()),
            open,
            page(Page::Aspect),
            window,
            cx,
        ),
    ];
    if session.quality_offered() {
        lines.push(setting_line(
            "Quality",
            Some(session.quality().short_label()),
            open,
            page(Page::Quality),
            window,
            cx,
        ));
    }
    lines.push(setting_line(
        "Playback info",
        None,
        open,
        page(Page::Info),
        window,
        cx,
    ));
    lines
}

fn show_page(shell: &mut Shell, page: Page, cx: &mut Context<Shell>) {
    if let Some(playback) = &mut shell.playback {
        playback.session.show_settings_page(page);
    }
    cx.notify();
}

/// A line of the menu: eased under the pointer, clickable while the menu is open.
fn menu_line(
    id: impl Into<gpui_kit::ElementId>,
    open: bool,
    act: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    theme::eased(id, theme::Shades::line(), window, cx)
        .flex_none()
        .h(px(MENU_LINE))
        .px(px(12.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .gap(px(10.))
        .when(open, |line| {
            line.cursor_pointer()
                .on_click(cx.listener(move |shell, _, window, cx| act(shell, window, cx)))
        })
}

/// A setting of the list, what it is set to, and the chevron to its page.
fn setting_line(
    label: &'static str,
    value: Option<String>,
    open: bool,
    act: Act,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    menu_line(label, open, act, window, cx)
        .justify_between()
        .child(label)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_color(theme::muted())
                .children(value)
                .child(icon(IconName::ChevronRight, 14.)),
        )
}

/// The top of a setting's page: back to the list.
fn back_line(
    title: &'static str,
    open: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    menu_line(
        "settings-back",
        open,
        |shell, _, cx| show_page(shell, Page::Main, cx),
        window,
        cx,
    )
    .font_weight(FontWeight::SEMIBOLD)
    .child(icon(IconName::ChevronLeft, 14.))
    .child(title)
}

/// One choice of a setting's page, checked when it is the one set.
#[allow(
    clippy::too_many_arguments,
    reason = "one line's look and what it does"
)]
fn choice_line(
    id: impl Into<gpui_kit::ElementId>,
    label: String,
    current: bool,
    open: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
    act: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static,
) -> Stateful<Div> {
    menu_line(id, open, act, window, cx)
        .child(
            div()
                .size(px(14.))
                .when(current, |mark| mark.child(icon(IconName::Check, 14.))),
        )
        .child(label)
}

/// "Subtitle offset": earlier, the offset, later, and a reset once it is not zero.
fn offset_line(
    tenths: i32,
    open: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let step = |id: &'static str,
                label: &'static str,
                by: i32,
                window: &mut Window,
                cx: &mut Context<Shell>| {
        theme::eased(id, theme::Shades::line(), window, cx)
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .font_weight(FontWeight::BOLD)
            .child(label)
            .when(open, |button| {
                button
                    .cursor_pointer()
                    .on_click(cx.listener(move |shell, _, _, cx| {
                        if let Some(playback) = &mut shell.playback {
                            playback.session.shift_subtitles(by);
                        }
                        cx.notify();
                    }))
            })
    };
    let earlier = step("subtitles-earlier", "−", -1, window, cx);
    let later = step("subtitles-later", "+", 1, window, cx);
    let reset = (tenths != 0).then(|| {
        theme::eased("subtitles-reset", theme::Shades::line(), window, cx)
            .size(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .child(icon(IconName::RotateCcw, 14.))
            .when(open, |button| {
                button
                    .cursor_pointer()
                    .on_click(cx.listener(|shell, _, _, cx| {
                        if let Some(playback) = &mut shell.playback {
                            playback.session.reset_subtitle_delay();
                        }
                        cx.notify();
                    }))
            })
    });
    div()
        .id("subtitle-offset")
        .flex_none()
        .h(px(MENU_LINE))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_between()
        .child("Subtitle offset")
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .children(reset)
                .child(earlier)
                .child(
                    div()
                        .w(px(52.))
                        .flex()
                        .justify_center()
                        .text_color(theme::muted())
                        .child(settings::offset_label(tenths)),
                )
                .child(later),
        )
}

/// "Black out other screens", under the tracks: a line with a switch, `on` of the way to on.
fn blackout_switch(
    on: f32,
    open: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    theme::eased("player-blackout", theme::Shades::line(), window, cx)
        .mt(px(18.))
        .px(px(12.))
        .py(px(9.))
        .rounded(px(8.))
        .when(open, |line| {
            line.cursor_pointer()
                .on_click(cx.listener(|shell, _, window, cx| {
                    shell.toggle_blackout(window, cx);
                }))
        })
        .flex()
        .items_center()
        .justify_between()
        .text_size(px(14.))
        .child("Black out other screens")
        .child(switch(on))
}

/// A switch, `on` of the way from off to on: the knob slides and the track eases to the accent.
pub(super) fn switch(on: f32) -> Div {
    let track = switch_track(on, theme::accent());
    let knob = switch_knob(on, theme::accent());
    div()
        .relative()
        .w(px(SWITCH_WIDTH))
        .h(px(SWITCH_HEIGHT))
        .rounded_full()
        .bg(track)
        .child(
            div()
                .absolute()
                .top(px(SWITCH_GAP))
                .left(px(SWITCH_GAP + on * (SWITCH_WIDTH - SWITCH_HEIGHT)))
                .size(px(SWITCH_HEIGHT - 2. * SWITCH_GAP))
                .rounded_full()
                .bg(knob),
        )
}

/// The panel's colour behind the off track, which is translucent white.
const PANEL: u32 = 0x0e0e10;

/// The switch's track: faint when off, the accent when on.
fn switch_track(on: f32, accent: Rgba) -> Rgba {
    motion::mix(rgba(0xffffff33), accent, on)
}

/// The knob follows the track: light on the dark off track, and, once on, whichever of dark or
/// light reads on the accent, so the switch shows whatever the accent is.
fn switch_knob(on: f32, accent: Rgba) -> Rgba {
    let light = rgb(0xffffff);
    let dark = rgb(PANEL);
    // Past this luminance dark reads better than white.
    let on_accent = if luminance(accent) > 0.18 {
        dark
    } else {
        light
    };
    motion::mix(light, on_accent, on)
}

/// The WCAG relative luminance of an opaque colour.
fn luminance(color: Rgba) -> f32 {
    let channel = |c: f32| {
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

fn section_title(label: &'static str) -> Div {
    div()
        .text_size(px(13.))
        .font_weight(FontWeight::BOLD)
        .text_color(theme::muted())
        .child(label)
}

fn choices(
    group: &'static str,
    choices: Vec<Choice>,
    open: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
    select: fn(&mut Shell, Option<i64>),
) -> Vec<Stateful<Div>> {
    choices
        .into_iter()
        .enumerate()
        .map(|(index, choice)| {
            let id = choice.id;
            let mut shades = theme::Shades::line();
            if choice.current {
                shades.rest = theme::line();
            }
            theme::eased((group, index), shades, window, cx)
                .px(px(12.))
                .py(px(9.))
                .rounded(px(8.))
                .when(open, |line| {
                    line.cursor_pointer()
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            select(shell, id);
                            cx.notify();
                        }))
                })
                .flex()
                .items_center()
                .gap(px(8.))
                .child(div().size(px(14.)).when(choice.current, |mark| {
                    mark.child(icon(IconName::Check, 14.))
                }))
                .child(choice.label)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// A translucent colour over the panel.
    fn over_panel(color: Rgba) -> Rgba {
        motion::mix(rgb(PANEL), Rgba { a: 1., ..color }, color.a)
    }

    const ACCENTS: [u32; 3] = [0xf4f4f5, 0xffffff, 0x1d4ed8];

    #[test]
    fn the_knob_reads_on_the_track_whatever_the_accent_is() {
        for accent in ACCENTS {
            let accent = rgb(accent);
            for on in [0., 1.] {
                let track = over_panel(switch_track(on, accent));
                let knob = switch_knob(on, accent);
                assert!(
                    contrast(knob, track) >= 3.,
                    "accent {accent:?}, on {on}: contrast {}",
                    contrast(knob, track)
                );
            }
        }
    }

    #[test]
    fn a_long_track_list_scrolls_inside_the_window() {
        // The panel stops above the bottom margin: what does not fit scrolls.
        assert_eq!(panel_height(810.), 810. - 96. - 40.);
        assert_eq!(panel_height(100.), 0.);
    }

    #[test]
    fn the_knob_eases_with_the_track() {
        let accent = rgb(0xf4f4f5);
        let (off, half, on) = (
            switch_knob(0., accent),
            switch_knob(0.5, accent),
            switch_knob(1., accent),
        );
        assert!(luminance(off) > luminance(half) && luminance(half) > luminance(on));
    }
}
