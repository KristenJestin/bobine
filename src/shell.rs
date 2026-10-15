//! The root view: the screen the navigator points at, framed by the pill bar or the back link.

use gpui_kit::prelude::*;
use gpui_kit::{
    Context, FocusHandle, IntoElement, KeyDownEvent, Render, SharedString, Window, div, px,
};

use crate::chrome::{back_link, pill_bar};
use crate::navigation::Navigator;
use crate::screens;
use crate::theme;

pub struct Shell {
    pub navigator: Navigator,
    focus: FocusHandle,
    font: Option<SharedString>,
}

impl Shell {
    pub fn new(font: Option<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            navigator: Navigator::new(),
            focus: cx.focus_handle(),
            font,
        }
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

        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|app, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && app.navigator.back() {
                    cx.notify();
                }
            }))
            .bg(theme::background())
            .text_color(theme::text())
            .text_size(px(14.))
            .when_some(self.font.clone(), |root, font| root.font_family(font))
            .child(screens::render(screen, cx))
            .when(screen.shows_pill_bar(), |root| {
                root.child(pill_bar(screen, cx))
            })
            .when_some(
                back_label.filter(|_| screen.shows_back_link()),
                |root, label| root.child(back_link(label, cx)),
            )
    }
}
