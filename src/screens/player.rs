//! The player, after Infuse: nothing over the picture until the mouse moves, then the title top
//! left, the seek bar and its buttons at the bottom; the audio and subtitles panel floats over
//! the video at the right. GPUI paints nothing else: the video shows through, from under the
//! window.

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Stateful,
    Window, div, linear_color_stop, linear_gradient, px, rgb, rgba,
};

use super::empty_screen;
use crate::player::controls::SKIP_SECONDS;
use crate::player::session::{Choice, Session};
use crate::player::time;
use crate::shell::Shell;
use crate::theme;

/// The margin of the controls from the edges of the window, after the design.
const EDGE: f32 = 40.;

/// The player screen with no film to play.
pub fn empty(_cx: &mut Context<Shell>) -> impl IntoElement {
    empty_screen("Lecteur")
}

pub fn render(session: &Session, window: &Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let now = Instant::now();
    let shown = session.controls_visible(now);
    div()
        .id("player")
        .size_full()
        .relative()
        .on_mouse_move(cx.listener(|shell, _: &MouseMoveEvent, _, cx| {
            shell.player_mouse_moved(cx);
        }))
        .when(shown, |root| {
            root.child(shade(true))
                .child(shade(false))
                .child(title_bar(session, cx))
                .child(bottom_bar(session, window, cx))
        })
        .when(session.panel_open(), |root| root.child(panel(session, cx)))
        .when_some(session.error(), |root, error| {
            root.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .px(px(20.))
                            .py(px(14.))
                            .rounded(px(theme::RADIUS_LARGE))
                            .bg(theme::panel())
                            .child(format!("Lecture impossible : {error}")),
                    ),
            )
        })
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

fn title_bar(session: &Session, cx: &mut Context<Shell>) -> impl IntoElement {
    div()
        .absolute()
        .top(px(30.))
        .left(px(EDGE))
        .right(px(EDGE))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(16.))
                .child(
                    round_button("player-back", "←", 36.)
                        .text_size(px(20.))
                        .on_click(cx.listener(|shell, _, window, cx| {
                            shell.player_back(window, cx);
                        })),
                )
                .child(
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
            text_button("player-tracks", "Audio et sous-titres")
                .when(session.panel_open(), |button| button.bg(theme::line()))
                .on_click(cx.listener(|shell, _, _, cx| {
                    if let Some(playback) = &mut shell.playback {
                        playback.session.toggle_panel();
                    }
                    cx.notify();
                })),
        )
}

fn bottom_bar(session: &Session, window: &Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let played = if session.duration > 0. {
        (session.position / session.duration).clamp(0., 1.) as f32
    } else {
        0.
    };
    let left = (session.duration - session.position).max(0.);
    let bar_width = f32::from(window.viewport_size().width) - 2. * EDGE;

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
                    cx.listener(move |shell, event: &MouseDownEvent, _, cx| {
                        if let Some(playback) = &mut shell.playback {
                            let x = f32::from(event.position.x) - EDGE;
                            playback.session.seek_to_fraction(x / bar_width.max(1.));
                        }
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(5.))
                        .rounded(px(3.))
                        .bg(rgba(0xffffff33))
                        .child(
                            div()
                                .h_full()
                                .w(gpui_kit::relative(played))
                                .rounded(px(3.))
                                .bg(rgb(0xffffff)),
                        )
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
                .child(div().w(px(220.)).child(time::clock(session.position)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(28.))
                        .child(text_button("player-back-10", "−10 s").on_click(cx.listener(
                            |shell, _, _, cx| {
                                if let Some(playback) = &mut shell.playback {
                                    playback.session.skip(-SKIP_SECONDS);
                                }
                                cx.notify();
                            },
                        )))
                        .child(play_pause(session.paused).on_click(cx.listener(
                            |shell, _, _, cx| {
                                if let Some(playback) = &mut shell.playback {
                                    playback.session.toggle_pause();
                                }
                                cx.notify();
                            },
                        )))
                        .child(
                            text_button("player-forward-10", "+10 s").on_click(cx.listener(
                                |shell, _, _, cx| {
                                    if let Some(playback) = &mut shell.playback {
                                        playback.session.skip(SKIP_SECONDS);
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .child(
                    div()
                        .w(px(220.))
                        .flex()
                        .justify_end()
                        .text_color(theme::muted())
                        .child(format!(
                            "{} · {}",
                            time::remaining(left),
                            time::end_clock(time::local_now(), left)
                        )),
                ),
        )
}

/// White, round: the pause bars while the film plays, the play triangle when it is paused.
fn play_pause(paused: bool) -> Stateful<Div> {
    let button = round_button("player-play-pause", "", 52.)
        .bg(rgb(0xffffff))
        .text_color(rgb(0x000000))
        .hover(|style| style.bg(rgb(0xe4e4e7)));
    if paused {
        button.text_size(px(18.)).child("▶")
    } else {
        let bar = || div().w(px(5.)).h(px(17.)).rounded(px(1.)).bg(rgb(0x000000));
        button.gap(px(5.)).child(bar()).child(bar())
    }
}

fn round_button(id: &'static str, label: &'static str, side: f32) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(side))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff1f)))
        .child(label)
}

fn text_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(8.))
        .rounded(px(theme::RADIUS))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.bg(rgba(0xffffff1f)))
        .child(label)
}

/// Over the video at the right: the picture keeps its whole size under it.
fn panel(session: &Session, cx: &mut Context<Shell>) -> impl IntoElement {
    div()
        .id("player-panel")
        .absolute()
        .top(px(96.))
        .right(px(EDGE))
        .w(px(340.))
        .pt(px(22.))
        .px(px(20.))
        .pb(px(14.))
        .rounded(px(theme::RADIUS_LARGE))
        .border_1()
        .border_color(theme::line())
        // No blur behind it in GPUI: opaque enough to read over any picture.
        .bg(rgba(0x0e0e10e0))
        // A click in the panel never reaches the video's area.
        .occlude()
        .child(section_title("AUDIO"))
        .child(
            choices("audio", session.audio_choices(), cx, |shell, id| {
                if let (Some(playback), Some(id)) = (&mut shell.playback, id) {
                    playback.session.select_audio(id);
                }
            })
            .mb(px(26.)),
        )
        .child(section_title("SOUS-TITRES"))
        .child(choices(
            "subtitle",
            session.subtitle_choices(),
            cx,
            |shell, id| {
                if let Some(playback) = &mut shell.playback {
                    playback.session.select_subtitle(id);
                }
            },
        ))
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
    cx: &mut Context<Shell>,
    select: fn(&mut Shell, Option<i64>),
) -> Div {
    let lines = choices.into_iter().enumerate().map(|(index, choice)| {
        let id = choice.id;
        div()
            .id((group, index))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(choice.current, |line| line.bg(theme::line()))
            .hover(|style| style.bg(theme::line()))
            .on_click(cx.listener(move |shell, _, _, cx| {
                select(shell, id);
                cx.notify();
            }))
            .child(if choice.current {
                format!("✓ {}", choice.label)
            } else {
                choice.label
            })
    });
    div()
        .mt(px(12.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(14.))
        .children(lines)
}
