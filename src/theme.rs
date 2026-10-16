//! The look of Bobine, in one place: screens ask for names, never for colours. The values come
//! from the design the maintainer validated (after nabby.pro).

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Div, ElementId, FontWeight, MouseButton, Rgba, SharedString, Stateful, Window,
    div, px, rgb, rgba,
};

use crate::icons::{IconName, icon};
use crate::motion::{self, Fade};

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

/// What went wrong.
pub fn danger() -> Rgba {
    rgb(0xf87171)
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

/// The size of an icon beside the text of a button.
pub const BUTTON_ICON: f32 = 16.;

/// The background of something that answers the pointer: at rest, under it, pressed.
#[derive(Clone, Copy)]
pub struct Shades {
    pub rest: Rgba,
    pub hover: Rgba,
    pub press: Rgba,
}

impl Shades {
    /// Clear at rest, a light veil under the pointer: over a picture.
    pub fn veil() -> Self {
        Self {
            rest: rgba(0xffffff00),
            hover: rgba(0xffffff1f),
            press: rgba(0xffffff33),
        }
    }

    /// Clear at rest, the line colour under the pointer: a line of a menu.
    pub fn line() -> Self {
        Self {
            rest: rgba(0x23232700),
            hover: line(),
            press: rgb(0x2e2e33),
        }
    }
}

/// How the pointer over one element stands, eased.
struct Pointer {
    hover: Fade,
    press: Fade,
}

/// An element whose background eases between its `shades` as the pointer comes, goes and
/// presses, instead of jumping. Asks for frames only while it moves.
pub fn eased(
    id: impl Into<ElementId>,
    shades: Shades,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let state = window.use_keyed_state(id.clone(), cx, |_, _| Pointer {
        hover: Fade::new(motion::quick()),
        press: Fade::new(motion::quick()),
    });
    let now = Instant::now();
    let pointer = state.read(cx);
    let bg = motion::mix(
        motion::mix(shades.rest, shades.hover, pointer.hover.level(now)),
        shades.press,
        pointer.press.level(now),
    );
    if pointer.hover.moving(now) || pointer.press.moving(now) {
        window.request_animation_frame();
    }
    let (hovered, pressed, released, released_out) =
        (state.clone(), state.clone(), state.clone(), state);
    let press = |state: &gpui_kit::Entity<Pointer>, down: bool, cx: &mut App| {
        state.update(cx, |pointer, cx| {
            pointer.press.set(down, Instant::now());
            cx.notify();
        });
    };
    div()
        .id(id)
        .bg(bg)
        .on_hover(move |hover, _, cx| {
            hovered.update(cx, |pointer, cx| {
                pointer.hover.set(*hover, Instant::now());
                cx.notify();
            });
        })
        .on_mouse_down(MouseButton::Left, move |_, _, cx| press(&pressed, true, cx))
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            press(&released, false, cx)
        })
        .on_mouse_up_out(MouseButton::Left, move |_, _, cx| {
            press(&released_out, false, cx)
        })
}

/// White, black text: the one thing to do on a screen.
pub fn primary_button(
    id: impl Into<ElementId>,
    leading: Option<IconName>,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let shades = Shades {
        rest: rgb(0xffffff),
        hover: rgb(0xe4e4e7),
        press: rgb(0xd4d4d8),
    };
    eased(id, shades, window, cx)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .px(px(20.))
        .py(px(12.))
        .rounded(px(RADIUS))
        .text_color(rgb(0x000000))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .children(leading.map(|name| icon(name, BUTTON_ICON)))
        .child(label.into())
}

/// Outlined, for what comes second.
pub fn ghost_button(
    id: impl Into<ElementId>,
    leading: Option<IconName>,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    ghost_button_with(
        id,
        leading.map(|name| icon(name, BUTTON_ICON).into_any_element()),
        label,
        window,
        cx,
    )
}

/// An outlined button whose leading element is the caller's: an icon that eases from one to
/// another.
pub fn ghost_button_with(
    id: impl Into<ElementId>,
    leading: Option<AnyElement>,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let shades = Shades {
        rest: rgba(0x14141680),
        hover: line(),
        press: rgb(0x2e2e33),
    };
    eased(id, shades, window, cx)
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .px(px(20.))
        .py(px(12.))
        .rounded(px(RADIUS))
        .border_1()
        .border_color(rgb(0x3a3a40))
        .text_color(text())
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .children(leading)
        .child(label.into())
}
