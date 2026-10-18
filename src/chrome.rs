//! What frames the screens: the floating pill bar over every screen but the player, and the
//! back link of the detail pages under it.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::*;
use gpui_kit::{
    Bounds, Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent, Pixels, Point,
    Rgba, Stateful, Window, anchored, canvas, deferred, div, linear_color_stop, linear_gradient,
    point, px, rgb, rgba,
};

use crate::brand;
use crate::icons::{IconName, icon};
use crate::menu;
use crate::motion;
use crate::navigation::NavEntry;
use crate::shell::Shell;
use crate::theme;

/// The account menu is the only one of the bar: its button opens it, so that it is its own key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Account;

/// Where the account button was last drawn, shared with the shell for its menu.
pub type ButtonBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

/// The account button and its menu, as the bar draws them this frame.
pub struct AccountView {
    pub user: String,
    pub server: String,
    /// The menu is open: it answers the mouse.
    pub open: bool,
    /// The menu offers the Stats page: there is something to show.
    pub stats: bool,
    /// How much of the menu shows, from 0 to 1: it may be fading out.
    pub shown: Option<f32>,
    pub bounds: ButtonBounds,
}

const MENU_WIDTH: f32 = 240.;
/// The user and their server, above the choices.
pub const MENU_HEADER: f32 = 44.;
/// The rule between the header and the choices, with its margins.
pub const MENU_RULE: f32 = 13.;
/// Under the pill bar, which ends 60 px down.
pub const BACK_LINK_TOP: f32 = 76.;

/// Where the account menu goes and how tall it is: under the button, its right edge on the
/// button's (the button is at the bar's end), and above it when there is no room below.
pub fn account_menu_place(
    button: Bounds<Pixels>,
    lines: usize,
    window_height: f32,
) -> (Point<Pixels>, f32) {
    let wanted = menu::height(lines) + MENU_HEADER + MENU_RULE;
    let (at, height) = menu::place(button, wanted, window_height);
    (point(button.right() - px(MENU_WIDTH), at.y), height)
}

/// The fill of an entry of the bar: clear, then the brand's coral as it becomes the section.
fn nav_fill(level: f32) -> Rgba {
    let clear = Rgba {
        a: 0.,
        ..theme::brand()
    };
    motion::mix(clear, theme::brand(), level)
}

/// The text of an entry: `rest` while it waits, the black written on the coral when it is lit.
fn nav_text(rest: Rgba, level: f32) -> Rgba {
    motion::mix(rest, theme::on_brand(), level)
}

/// Centred at the top, over the screen, in the same place on every screen; the search field opens
/// the palette, the account button the menu to sign out. `highlight` is how much each entry
/// shows as the section of the page, from 0 to 1: it eases from one to the next.
pub fn pill_bar(
    highlight: [f32; NavEntry::ALL.len()],
    account: Option<AccountView>,
    discover: bool,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    // Discover is there only while a request server is configured.
    let shown = NavEntry::ALL
        .into_iter()
        .enumerate()
        .filter(move |(_, entry)| discover || *entry != NavEntry::Discover);
    let entries = shown.map(|(index, entry)| {
        let level = highlight[index];
        let live = entry.target().is_some();
        let rest = if live { theme::muted() } else { theme::dim() };
        let item = div()
            .id(("nav-entry", index))
            .debug_selector(move || format!("nav {}", entry.label()))
            .px(px(12.))
            .py(px(8.))
            .rounded(px(theme::RADIUS))
            .text_size(px(13.))
            .text_color(nav_text(rest, level))
            .bg(nav_fill(level))
            .child(entry.label());
        if live {
            item.cursor_pointer()
                .on_click(cx.listener(move |app, _, _, cx| app.select(entry, cx)))
        } else {
            item
        }
    });

    div()
        .absolute()
        .top(px(16.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .debug_selector(|| "pill-bar".into())
                .flex()
                .items_center()
                .gap(px(4.))
                .p(px(5.))
                .rounded(px(theme::RADIUS_LARGE))
                .border_1()
                .border_color(theme::line())
                // A click on the bar never reaches what is under it.
                .occlude()
                // Opaque enough to read over a hero image: there is no blur behind it.
                .bg(rgba(0x141416ec))
                .child(brand::mark(32., window, cx).mr(px(6.)))
                .children(entries)
                .child(search_field(window, cx))
                .children(account.map(|account| account_button(account, window, cx))),
        )
}

/// The user's avatar, filled; a click opens their menu under it.
fn account_button(
    account: AccountView,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let drawn = account.bounds.clone();
    let shades = theme::Shades {
        rest: rgba(0xffffff00),
        hover: theme::line(),
        press: rgb(0x2e2e33),
    };
    let button = theme::eased("nav-account", shades, window, cx)
        .debug_selector(|| "nav-account".into())
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_color(if account.open {
            theme::text()
        } else {
            theme::muted()
        })
        .cursor_pointer()
        .hover(|style| style.text_color(theme::text()))
        .on_click(cx.listener(|shell, _, _, cx| shell.toggle_account_menu(cx)))
        .child(icon(IconName::UserCircle, 22.));
    let under = account.bounds.get().zip(account.shown);
    div()
        .relative()
        .child(button)
        // Where the button is drawn, for its menu.
        .child(
            canvas(move |at, _, _| drawn.set(Some(at)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
        .children(under.map(|(button, shown)| account_menu(button, shown, &account, window, cx)))
}

/// Who is signed in, and the way out, over the rest of the page, `shown` of the way in. Only an
/// open menu answers the mouse: one fading out lets clicks through.
fn account_menu(
    button: Bounds<Pixels>,
    shown: f32,
    account: &AccountView,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (open, stats) = (account.open, account.stats);
    let (user, server) = (account.user.as_str(), account.server.as_str());
    let lines = 2 + usize::from(stats);
    let (at, height) = account_menu_place(button, lines, f32::from(window.viewport_size().height));
    // It comes into place from a little nearer its button.
    let toward_button = if at.y > button.top() { -8. } else { 8. };
    let rise = point(px(0.), px(toward_button * (1. - shown)));
    let settings = menu_line("account-settings", IconName::Gear, "Settings", window, cx)
        .when(open, |line| {
            line.on_click(cx.listener(|shell, _, _, cx| shell.open_settings(cx)))
        });
    let stats = stats.then(|| {
        menu_line("account-stats", IconName::ChartBar, "Stats", window, cx).when(open, |line| {
            line.on_click(cx.listener(|shell, _, _, cx| shell.open_stats(cx)))
        })
    });
    let sign_out = menu_line("sign-out", IconName::SignOut, "Sign out", window, cx)
        .when(open, |line| {
            line.on_click(cx.listener(|shell, _, window, cx| shell.sign_out(window, cx)))
        });
    deferred(
        anchored()
            .position(at + rise)
            .snap_to_window_with_margin(px(menu::MARGIN))
            .child(
                div()
                    .id("account-menu")
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
                                shell.pressed_outside_account_menu(event.position, cx);
                            },
                        ))
                    })
                    .child(
                        div()
                            .h(px(MENU_HEADER))
                            .flex_none()
                            .px(px(12.))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(user.to_string()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(theme::dim())
                                    .child(server.to_string()),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h(px(1.))
                            .mx(px(6.))
                            .my(px(6.))
                            .bg(theme::line()),
                    )
                    .child(settings)
                    .children(stats)
                    .child(sign_out),
            ),
    )
}

/// One choice of the account menu: an icon and a word.
fn menu_line(
    id: &'static str,
    glyph: IconName,
    label: &'static str,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    theme::eased(id, theme::Shades::line(), window, cx)
        .debug_selector(|| id.into())
        .flex()
        .items_center()
        .gap(px(10.))
        .flex_none()
        .h(px(menu::LINE))
        .px(px(12.))
        .rounded(px(7.))
        .text_size(px(13.))
        .cursor_pointer()
        .child(icon(glyph, 14.).text_color(theme::muted()))
        .child(label)
}

/// Opens the search palette, like Ctrl+K.
fn search_field(window: &mut Window, cx: &mut Context<Shell>) -> Stateful<Div> {
    theme::eased("nav-search", theme::Shades::line(), window, cx)
        .ml(px(6.))
        .flex()
        .items_center()
        .gap(px(10.))
        .py(px(6.))
        .pl(px(10.))
        .pr(px(8.))
        .rounded(px(theme::RADIUS))
        .border_1()
        .border_color(theme::line())
        .text_size(px(13.))
        .text_color(theme::muted())
        .cursor_pointer()
        .on_click(cx.listener(|app, _, window, cx| {
            app.open_search(window, cx);
        }))
        .child(icon(IconName::Search, 14.))
        .child("Search")
        .child(
            div()
                .px(px(6.))
                .py(px(1.))
                .rounded(px(5.))
                .bg(theme::line())
                .text_size(px(11.))
                .text_color(theme::muted())
                .child("Ctrl K"),
        )
}

/// Darkens the top of a backdrop a little, so that the bar over it stays readable.
pub fn bar_fade() -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(BAR_FADE))
        .bg(linear_gradient(
            180.,
            linear_color_stop(rgba(0x09090a99), 0.),
            linear_color_stop(rgba(0x09090a00), 1.),
        ))
}

/// How far down the shade behind the bar goes.
const BAR_FADE: f32 = 140.;

/// Top left of a detail page, under the pill bar: an arrow and `Movies`.
pub fn back_link(
    label: &'static str,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    // Readable over a page scrolled under it.
    let shades = theme::Shades {
        rest: rgba(0x09090acc),
        hover: theme::line(),
        press: rgb(0x2e2e33),
    };
    theme::eased("back-link", shades, window, cx)
        .debug_selector(|| "back-link".into())
        .flex()
        .items_center()
        .gap(px(6.))
        .absolute()
        .top(px(BACK_LINK_TOP))
        .left(px(40.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(theme::RADIUS))
        .text_size(px(14.))
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|style| style.text_color(theme::text()))
        .on_click(cx.listener(|app, _, _, cx| {
            app.go_back(cx);
        }))
        .child(icon(IconName::ArrowLeft, 16.))
        .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::Menus;
    use gpui_kit::size;

    #[test]
    fn the_active_pill_is_coral_with_text_that_reads_on_it() {
        assert_eq!(nav_fill(1.), theme::brand());
        assert_eq!(nav_text(theme::muted(), 1.), theme::on_brand());
        assert!(crate::theme::contrast(nav_text(theme::muted(), 1.), nav_fill(1.)) >= 4.5);
        // A section that is not the page's stays clear.
        assert_eq!(nav_fill(0.).a, 0.);
    }

    fn button_at(top: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(900.), px(top)), size(px(32.), px(32.)))
    }

    #[test]
    fn the_account_menu_opens_under_its_button_with_the_same_right_edge() {
        let (at, height) = account_menu_place(button_at(16.), 2, 810.);
        assert_eq!(at, point(px(932. - MENU_WIDTH), px(54.)));
        assert_eq!(height, menu::height(2) + MENU_HEADER + MENU_RULE);
    }

    #[test]
    fn the_account_menu_closes_on_an_outside_click() {
        let mut menus = Menus::default();
        menus.toggle(Account);
        menus.pressed_outside(None);
        assert_eq!(menus.open(), None);
    }

    #[test]
    fn the_account_menu_closes_on_escape() {
        let mut menus = Menus::default();
        menus.toggle(Account);
        assert!(menus.escape());
        assert_eq!(menus.open(), None);
        // With none open, Escape is for the page.
        assert!(!menus.escape());
    }

    #[test]
    fn a_click_on_the_account_button_closes_its_open_menu() {
        let mut menus = Menus::default();
        menus.toggle(Account);
        // The press reaches the menu's outside first, then the button's click.
        menus.pressed_outside(Some(Account));
        menus.toggle(Account);
        assert_eq!(menus.open(), None);
    }
}
