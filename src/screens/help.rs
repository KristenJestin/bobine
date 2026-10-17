//! The help, `?`: the keys of the screen in two columns, on a panel over a dim backdrop. It is
//! read from the keymap itself (`crate::keys`), so it says what the keys do and nothing else.
//! It fades and grows in.

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, Context, Div, FontWeight, MouseButton, Window, div, px, rgba};

use crate::keys::{self, Key};
use crate::navigation::NavEntry;
use crate::shell::Shell;
use crate::theme;

const WIDTH: f32 = 760.;
/// The size it grows from, against its own.
const FROM_SCALE: f32 = 0.94;

/// The help over the screen, the player's when `player`, while it shows or fades out.
pub fn overlay(
    shell: &mut Shell,
    player: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let now = Instant::now();
    let (open, fade) = shell.help();
    if !fade.visible(now) {
        return None;
    }
    if fade.moving(now) {
        window.request_animation_frame();
    }
    let level = fade.level(now);
    // GPUI scales no box: the panel is laid out at its size of the moment instead.
    let scale = FROM_SCALE + (1. - FROM_SCALE) * level;
    let at = |size: f32| px(size * scale);

    // The Discover pill, and its key, only with a request server.
    let discover = shell.seerr_client().is_some();
    let lines: Vec<&Key> = keys::listed(player)
        .filter(|key| discover || key.what != NavEntry::Discover.label())
        .collect();
    let half = lines.len().div_ceil(2);
    let column = |lines: &[&Key]| {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(at(10.))
            .children(lines.iter().map(|key| line(key, scale)))
    };
    let panel = div()
        .id("help")
        .w(at(WIDTH))
        .flex()
        .flex_col()
        .gap(at(20.))
        .p(at(28.))
        .rounded(at(16.))
        .border_1()
        .border_color(rgba(0x2a2a30ff))
        .bg(theme::panel())
        .shadow_lg()
        .occlude()
        .child(
            div()
                .text_size(at(18.))
                .font_weight(FontWeight::BOLD)
                .child("Keyboard shortcuts"),
        )
        .child(
            div()
                .flex()
                .gap(at(40.))
                .child(column(&lines[..half]))
                .child(column(&lines[half..])),
        );
    Some(
        div()
            .absolute()
            .size_full()
            .top_0()
            .left_0()
            .child(
                // A click outside the panel closes it; nothing under it is reached meanwhile.
                div()
                    .id("help-backdrop")
                    .absolute()
                    .size_full()
                    .occlude()
                    .bg(rgba((0x8c as f32 * level) as u32))
                    .when(open, |backdrop| {
                        backdrop.on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|shell, _, _, cx| shell.close_help(cx)),
                        )
                    }),
            )
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .opacity(level)
                    .child(panel),
            )
            .into_any_element(),
    )
}

/// What a key does, then its keys, the alternatives apart.
fn line(key: &Key, scale: f32) -> Div {
    let at = |size: f32| px(size * scale);
    let mut keys = div().flex_none().flex().items_center().gap(at(4.));
    for (index, keystroke) in key.keys.iter().enumerate() {
        if index > 0 {
            keys = keys.child(div().px(at(2.)).text_color(theme::dim()).child("/"));
        }
        for cap in keys::caps(keystroke) {
            keys = keys.child(
                div()
                    .min_w(at(24.))
                    .h(at(24.))
                    .px(at(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(at(6.))
                    .border_1()
                    .border_color(rgba(0x3a3a42ff))
                    .bg(rgba(0x1c1c20ff))
                    .text_size(at(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(cap),
            );
        }
    }
    div()
        .debug_selector(|| format!("help-line {}", key.what))
        .flex()
        .items_center()
        .justify_between()
        .gap(at(16.))
        .child(
            div()
                .text_size(at(13.))
                .text_color(theme::muted())
                .child(key.what),
        )
        .child(keys)
}
