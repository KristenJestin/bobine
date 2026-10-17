//! The Settings screen: one card per section (Playback, Screens, Notifications, Integrations,
//! About), in a narrow column under the bar. A change is made where it is shown and kept at once.

use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, Bounds, Context, Div, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, Pixels, Stateful, Window, anchored, canvas, deferred, div, point,
    px,
};

use super::library::GRID_TOP;
use super::player::switch;
use crate::config::Quality;
use crate::icons::{IconName, icon};
use crate::menu;
use crate::motion;
use crate::secret_field;
use crate::shell::{Loadable, QualityMenu, Shell};
use crate::theme;

/// The widest the column goes: the sections stay readable on a wide window.
const COLUMN: f32 = 680.;
const MENU_WIDTH: f32 = 200.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let now = Instant::now();
    let switch_on = {
        let wanted = shell.settings.black_out_other_monitors;
        let fade = &mut shell.blackout_switch;
        fade.set(wanted, now);
        if fade.moving(now) {
            window.request_animation_frame();
        }
        fade.level(now)
    };
    let episodes_on = {
        let wanted = shell.settings.notify_new_episodes;
        let fade = &mut shell.settings_page.episodes_switch;
        fade.set(wanted, now);
        if fade.moving(now) {
            window.request_animation_frame();
        }
        fade.level(now)
    };
    let open = shell.settings_page.menus.open();
    let menu_shown = shell.settings_page.menu_fade.follow(open, now);
    if shell.settings_page.menu_fade.moving(now) {
        window.request_animation_frame();
    }
    let about = shell.about();
    let unavailable = shell.blackout_unavailable();
    let quality = shell.settings.default_quality;

    let playback = section("settings-playback", IconName::Play, "Playback").child(row(
        "Default quality",
        Some("Where each movie starts. The player's settings change it for that movie."),
        quality_choice(shell, quality, open.is_some(), menu_shown, window, cx),
    ));

    let screens = section("settings-screens", IconName::Monitor, "Screens")
        .child(
            theme::eased("settings-blackout", theme::Shades::line(), window, cx)
                .debug_selector(|| "settings-blackout".into())
                .cursor_pointer()
                .on_click(cx.listener(|shell, _, window, cx| shell.toggle_blackout(window, cx)))
                .child(row(
                    "Black out other screens",
                    Some("The other monitors turn black while a movie plays fullscreen."),
                    switch(switch_on),
                )),
        )
        .children(unavailable.then(|| {
            div()
                .px(px(12.))
                .pb(px(8.))
                .text_size(px(12.))
                .text_color(theme::dim())
                .child("This compositor cannot cover other screens.")
        }));

    let notifications = section("settings-notifications", IconName::Bell, "Notifications").child(
        theme::eased("settings-new-episodes", theme::Shades::line(), window, cx)
            .debug_selector(|| "settings-new-episodes".into())
            .cursor_pointer()
            .on_click(cx.listener(|shell, _, _, cx| shell.toggle_new_episodes(cx)))
            .child(row(
                "New episodes",
                Some("The desktop is told when a series you follow gets a new episode."),
                switch(episodes_on),
            )),
    );

    let seerr_key = shell.seerr.key.clone();
    let fields: Vec<_> = shell
        .settings_page
        .secrets
        .iter()
        .filter(|field| Some(*field) != seerr_key.as_ref())
        .cloned()
        .collect();
    let seerr = seerr_block(shell, window, cx);
    let integrations = section("settings-integrations", IconName::Plug, "Integrations")
        .children(seerr)
        .children(
            (fields.is_empty() && shell.seerr.address.is_none()).then(|| {
                div()
                    .px(px(12.))
                    .py(px(10.))
                    .child(super::note("No integration yet."))
            }),
        )
        .children(
            fields
                .into_iter()
                .map(|field| div().px(px(12.)).py(px(10.)).child(field)),
        );

    let about = about.into_iter().fold(
        section("settings-about", IconName::Info, "About"),
        |card, (label, value)| {
            card.child(row(
                label,
                None,
                div()
                    .text_size(px(14.))
                    .text_color(theme::muted())
                    .child(value),
            ))
        },
    );

    div()
        .debug_selector(|| "settings-page".into())
        .id("settings")
        .size_full()
        .overflow_y_scroll()
        .flex()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(COLUMN))
                .px(px(24.))
                .pt(px(GRID_TOP + 24.))
                .pb(px(48.))
                .flex()
                .flex_col()
                .gap(px(16.))
                .child(
                    div()
                        .pb(px(8.))
                        .text_size(px(28.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Settings"),
                )
                .child(playback)
                .child(screens)
                .child(notifications)
                .child(integrations)
                .child(about),
        )
}

/// Seerr's address, its API key, and the test of the connection, with what it found.
fn seerr_block(shell: &Shell, window: &mut Window, cx: &mut Context<Shell>) -> Option<Div> {
    let address = shell.seerr.address.clone()?;
    let key = shell.seerr.key.clone()?;
    let save = theme::eased("seerr-save", theme::Shades::line(), window, cx)
        .debug_selector(|| "seerr-save".into())
        .px(px(12.))
        .py(px(7.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(theme::line())
        .text_size(px(13.))
        .cursor_pointer()
        .on_click(cx.listener(|shell, _, _, cx| shell.save_seerr(cx)))
        .child("Save");
    let address_row = div()
        .debug_selector(|| "seerr-address".into())
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.))
        .px(px(12.))
        .py(px(10.))
        .text_size(px(14.))
        .child("Address")
        .child(div().w(px(secret_field::FIELD)).child(address));
    let test = theme::eased("seerr-test", theme::Shades::line(), window, cx)
        .debug_selector(|| "seerr-test".into())
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(7.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(theme::line())
        .text_size(px(13.))
        .cursor_pointer()
        .on_click(cx.listener(|shell, _, _, cx| shell.test_seerr(cx)))
        .child(icon(IconName::Plug, 14.).text_color(theme::muted()))
        .child("Test");
    // A quiet hint while the form is half filled; the test's word once there is one.
    let hint = (shell.seerr.check.is_none())
        .then_some(shell.seerr.hint)
        .flatten()
        .map(|hint| {
            div()
                .debug_selector(|| "seerr-hint".into())
                .text_size(px(13.))
                .text_color(theme::dim())
                .child(hint)
                .with_animation(
                    gpui_kit::ElementId::Name(format!("seerr-hint-{hint}").into()),
                    Animation::new(motion::quick()).with_easing(motion::ease_out),
                    |line, t| line.opacity(t),
                )
        });
    let (said, failed) = match &shell.seerr.check {
        None => (None, false),
        Some(Loadable::Loading) => (Some("Testing…".to_string()), false),
        Some(Loadable::Ready(line)) => (Some(line.clone()), false),
        Some(Loadable::Failed(why)) => (Some(why.to_string()), true),
    };
    let check = said.map(|said| {
        div()
            .debug_selector(|| "seerr-check".into())
            .text_size(px(13.))
            .text_color(if failed {
                theme::danger()
            } else {
                theme::muted()
            })
            .child(said.clone())
            .with_animation(
                gpui_kit::ElementId::Name(format!("seerr-check-{said}").into()),
                Animation::new(motion::quick()).with_easing(motion::ease_out),
                |line, t| line.opacity(t),
            )
    });
    let owner = shell.seerr.owner_line.clone().map(|line| {
        div()
            .debug_selector(|| "seerr-owner".into())
            .px(px(12.))
            .pb(px(10.))
            .text_size(px(12.))
            .text_color(theme::dim())
            .child(line)
    });
    Some(
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(12.))
                    .pt(px(6.))
                    .pb(px(4.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Seerr"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::dim())
                            .child("Request movies and series that are not in the library."),
                    ),
            )
            .child(
                // One form: Enter in either field saves both.
                div()
                    .on_key_down(cx.listener(|shell, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "enter" {
                            shell.save_seerr(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .child(address_row)
                    .child(div().px(px(12.)).py(px(10.)).child(key)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(save)
                    .child(test)
                    .child(div().pl(px(4.)).children(check).children(hint)),
            )
            .children(owner),
    )
}

/// A card: its icon and title, then what the caller puts in it.
fn section(id: &'static str, glyph: IconName, title: &'static str) -> Div {
    div()
        .debug_selector(move || id.into())
        .flex()
        .flex_col()
        .p(px(8.))
        .rounded(px(theme::RADIUS_LARGE))
        .border_1()
        .border_color(theme::line())
        .bg(theme::panel())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(8.))
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon(glyph, 16.).text_color(theme::muted()))
                .child(title),
        )
}

/// A line of a card: a label, what it means under it, and the control at the end.
fn row(label: &'static str, hint: Option<&'static str>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .px(px(12.))
        .py(px(10.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().text_size(px(14.)).child(label))
                .children(hint.map(|hint| {
                    div()
                        .text_size(px(12.))
                        .text_color(theme::dim())
                        .child(hint)
                })),
        )
        .child(control)
}

/// The button showing the quality in force, which opens the menu of the others under it.
fn quality_choice(
    shell: &Shell,
    quality: Quality,
    open: bool,
    shown: Option<(QualityMenu, f32)>,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let drawn = shell.settings_page.quality_button.clone();
    let button = theme::eased("settings-quality", theme::Shades::line(), window, cx)
        .debug_selector(|| "settings-quality".into())
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(12.))
        .py(px(8.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(theme::line())
        .text_size(px(13.))
        .cursor_pointer()
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_quality_menu(cx)))
        .child(quality.label())
        .child(icon(IconName::ChevronDown, 14.).text_color(theme::muted()));
    let under = drawn.get().zip(shown);
    div()
        .relative()
        .child(button)
        // Where the button is drawn, for its menu.
        .child(
            canvas(move |at, _, _| drawn.set(Some(at)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .children(
            under
                .map(|(button, (_, shown))| quality_menu(button, shown, open, quality, window, cx)),
        )
}

/// The choices, over the page, `shown` of the way in. Only an open menu answers the mouse.
fn quality_menu(
    button: Bounds<Pixels>,
    shown: f32,
    open: bool,
    current: Quality,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (at, height) = menu::place(
        button,
        menu::height(Quality::CHOICES.len()),
        f32::from(window.viewport_size().height),
    );
    // It comes into place from a little nearer its button.
    let toward_button = if at.y > button.top() { -8. } else { 8. };
    let at = point(
        button.right() - px(MENU_WIDTH),
        at.y + px(toward_button * (1. - shown)),
    );
    let lines: Vec<Stateful<Div>> = Quality::CHOICES
        .into_iter()
        .map(|choice| {
            let kept = String::from(choice);
            theme::eased(format!("quality-{kept}"), theme::Shades::line(), window, cx)
                .debug_selector(move || format!("quality-{kept}"))
                .flex()
                .items_center()
                .justify_between()
                .flex_none()
                .h(px(menu::LINE))
                .px(px(12.))
                .rounded(px(7.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(open, |line| {
                    line.on_click(cx.listener(move |shell, _, _, cx| shell.set_quality(choice, cx)))
                })
                .child(choice.label())
                .children((choice == current).then(|| icon(IconName::Check, 14.)))
        })
        .collect();
    deferred(
        anchored()
            .position(at)
            .snap_to_window_with_margin(px(menu::MARGIN))
            .child(
                div()
                    .debug_selector(|| "quality-menu".into())
                    .id("quality-menu")
                    .opacity(shown)
                    .h(px(height))
                    .overflow_y_scroll()
                    .w(px(MENU_WIDTH))
                    .p(px(6.))
                    .flex()
                    .flex_col()
                    .rounded(px(theme::RADIUS_LARGE))
                    .border_1()
                    .border_color(theme::line())
                    .bg(theme::panel())
                    .when(open, |menu| {
                        menu.occlude().on_mouse_down_out(cx.listener(
                            |shell, event: &MouseDownEvent, _, cx| {
                                // A side button is Back: it closes the menu itself, once.
                                if matches!(event.button, MouseButton::Navigate(_)) {
                                    return;
                                }
                                shell.pressed_outside_quality_menu(event.position, cx);
                            },
                        ))
                    })
                    .children(lines),
            ),
    )
}
