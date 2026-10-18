//! The skip button of the player ("Skip intro", "Next episode"…): bottom right, over the
//! picture. It rides above the bottom controls while they show, and drops back to the margin
//! when they go.

use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, FontWeight, Stateful, Window, div, px, rgb};

use crate::icons::{IconName, icon};
use crate::player::controls::BOTTOM_CONTROLS_HEIGHT;
use crate::player::segments::Button;
use crate::shell::Shell;
use crate::theme;

/// The margin from the edges of the window, as the controls have.
const EDGE: f32 = 40.;
/// The air between the button and the controls under it.
const ABOVE_CONTROLS: f32 = 16.;

/// `shown`, from 0 to 1: the button fades in and slides in a little from the right. `controls`,
/// likewise: how much of the bottom controls shows.
pub fn button(
    button: Button,
    shown: f32,
    controls: f32,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    let shades = theme::Shades {
        rest: rgb(0xffffff),
        hover: rgb(0xe4e4e7),
        press: rgb(0xd4d4d8),
    };
    let rise = (BOTTOM_CONTROLS_HEIGHT + ABOVE_CONTROLS - EDGE) * controls;
    super::player::keeps_clicks(theme::eased("player-skip", shades, window, cx))
        .absolute()
        .right(px(EDGE - 16. * (1. - shown)))
        .bottom(px(EDGE + rise))
        .opacity(shown)
        .h(px(46.))
        .px(px(22.))
        .flex()
        .items_center()
        .gap(px(10.))
        .rounded_full()
        .text_color(rgb(0x000000))
        .text_size(px(15.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .debug_selector(|| "player-skip".into())
        .on_click(cx.listener(|shell, _, window, cx| shell.press_skip(window, cx)))
        .child(div().child(button.label.text()))
        .child(icon(IconName::SkipForward, 18.))
}
