//! The look of Bobine, in one place: screens ask for names, never for colours. The values come
//! from the design the maintainer validated (after nabby.pro).

use gpui_kit::prelude::*;
use gpui_kit::{App, Div, ElementId, FontWeight, Rgba, SharedString, Stateful, div, px, rgb, rgba};

pub fn background() -> Rgba {
    rgb(0x09090a)
}

pub fn panel() -> Rgba {
    rgb(0x141416)
}

pub fn line() -> Rgba {
    rgb(0x232327)
}

pub fn text() -> Rgba {
    rgb(0xf4f4f5)
}

pub fn muted() -> Rgba {
    rgb(0xa1a1aa)
}

pub fn dim() -> Rgba {
    rgb(0x6b6b73)
}

pub fn accent() -> Rgba {
    rgb(0x4ade80)
}

/// A button, an entry of the pill bar, a search field.
pub const RADIUS: f32 = 9.;
/// A floating bar, a card.
pub const RADIUS_LARGE: f32 = 14.;

/// Inter when it is installed, else `None`: the system font stays in charge.
pub fn font_family(cx: &App) -> Option<SharedString> {
    cx.text_system()
        .all_font_names()
        .iter()
        .any(|name| name == "Inter")
        .then(|| "Inter".into())
}

/// White, black text: the one thing to do on a screen.
pub fn primary_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(20.))
        .py(px(12.))
        .rounded(px(RADIUS))
        .bg(rgb(0xffffff))
        .text_color(rgb(0x000000))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0xe4e4e7)))
        .child(label)
}

/// Outlined, for what comes second.
pub fn ghost_button(id: impl Into<ElementId>, label: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(20.))
        .py(px(12.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(rgb(0x3a3a40))
        .bg(rgba(0x14141680))
        .text_color(text())
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|style| style.bg(line()))
        .child(label)
}

/// A small pill: a filter, a version, a track.
pub fn chip(label: &'static str) -> Div {
    div()
        .px(px(10.))
        .py(px(5.))
        .rounded_full()
        .border_1()
        .border_color(line())
        .bg(rgba(0x14141699))
        .text_color(muted())
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}
