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

/// Near-white: the interface is monochrome, only `danger` has a colour.
pub fn accent() -> Rgba {
    rgb(0xe5e5e5)
}

/// The brand's coral on dark (`assets/brand/couleurs.txt`).
pub fn brand() -> Rgba {
    rgb(0xf45246)
}

/// What is written on the coral: the interface's black, which reads there.
pub fn on_brand() -> Rgba {
    rgb(0x000000)
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

/// The height of every button: a filled one and an outlined one side by side stand level.
pub const BUTTON_HEIGHT: f32 = 44.;

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

/// What every button shares: one height, whatever its border, so that a filled one and an
/// outlined one side by side stand level.
fn button(
    id: impl Into<ElementId>,
    shades: Shades,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let id = id.into();
    let selector = id.to_string();
    eased(id, shades, window, cx)
        .debug_selector(move || selector.clone())
        .flex()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .h(px(BUTTON_HEIGHT))
        .px(px(20.))
        .rounded(px(RADIUS))
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
}

/// Coral at rest, a little lighter under the pointer, a little darker pressed.
pub fn primary_shades() -> Shades {
    Shades {
        rest: brand(),
        hover: rgb(0xf8695e),
        press: rgb(0xe04a3f),
    }
}

/// Coral, black text: the one thing to do on a screen.
pub fn primary_button(
    id: impl Into<ElementId>,
    leading: Option<IconName>,
    label: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    button(id, primary_shades(), window, cx)
        .text_color(on_brand())
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
    button(id, shades, window, cx)
        .border_1()
        .border_color(rgb(0x3a3a40))
        .text_color(text())
        .children(leading)
        .child(label.into())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_accent_is_a_light_neutral_so_the_interface_stays_monochrome() {
        let accent = accent();
        assert_eq!(
            (accent.r, accent.g),
            (accent.b, accent.b),
            "grey, not tinted"
        );
        assert!(accent.r >= 0.85, "near-white on the dark theme");
    }

    #[test]
    fn the_accent_reads_against_the_dark_background_and_its_own_marks() {
        // A watched mark is the accent with the background drawn on it, a progress bar is the
        // accent over a clear veil: both need contrast.
        let (accent, background) = (accent(), background());
        assert!(accent.r - background.r > 0.7);
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

    pub fn contrast(a: Rgba, b: Rgba) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn the_brand_colour_is_the_coral_of_the_brand_files() {
        assert_eq!(brand(), rgb(0xf45246));
        assert!(include_str!("../assets/brand/couleurs.txt").contains("#f45246"));
    }

    #[test]
    fn the_primary_button_is_coral_and_its_text_reads_on_every_shade() {
        let shades = primary_shades();
        assert_eq!(shades.rest, brand());
        for shade in [shades.rest, shades.hover, shades.press] {
            let ratio = contrast(on_brand(), shade);
            assert!(ratio >= 4.5, "{shade:?}: contrast {ratio}");
        }
    }

    #[test]
    fn the_danger_stays_red() {
        let danger = danger();
        assert!(danger.r > danger.g + 0.3 && danger.r > danger.b + 0.3);
    }
}
