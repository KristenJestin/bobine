//! A playlist's page: its name and count, "Play all", then its titles in their order, one per
//! line. Only the lines in view are built, so only their pictures are asked.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use bobine::provider::{ItemId, PlaylistEntry};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Context, Div, FontWeight, IntoElement, MouseButton, MouseDownEvent, Pixels,
    SharedString, Stateful, Window, anchored, canvas, deferred, div, point, px, rgba, uniform_list,
};

use super::note;
use crate::grid::MARGIN;
use crate::icons::{IconName, icon};
use crate::images;
use crate::menu;
use crate::playlists;
use crate::shell::{Loadable, Shell};
use crate::theme;

/// The width of the "Add to playlist" menu.
const MENU_WIDTH: f32 = 260.;
/// The name field of a new playlist, with its margins.
const FIELD: f32 = 44.;

/// One line of the list, its gap included.
const LINE: f32 = 92.;
/// The picture of a line, wide.
const STILL_WIDTH: f32 = 128.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.playlist_pages.last() else {
        return div().into_any_element();
    };
    let (len, status) = match &page.entries {
        Loadable::Ready(entries) if entries.is_empty() => (0, Some(playlists::EMPTY)),
        Loadable::Ready(entries) => (entries.len(), None),
        Loadable::Failed(why) => (0, Some(*why)),
        Loadable::Loading => (0, Some("Loading…")),
    };
    let name = page.name.clone();
    let busy = page.busy;
    let scroll = page.scroll.clone();
    let count = matches!(page.entries, Loadable::Ready(_)).then(|| playlists::titles(len));

    let play_all = (len > 0).then(|| {
        theme::primary_button(
            "playlist-play-all",
            Some(IconName::Play),
            "Play all",
            window,
            cx,
        )
        .when(busy, |button| button.opacity(0.6))
        .on_click(cx.listener(|shell, _, window, cx| shell.play_all(window, cx)))
    });
    let list = uniform_list(
        "playlist",
        len,
        cx.processor(move |shell: &mut Shell, range: Range<usize>, window, cx| {
            let entries = match shell.playlist_pages.last().map(|page| &page.entries) {
                Some(Loadable::Ready(entries)) => {
                    entries[range.start..range.end.min(entries.len())].to_vec()
                }
                _ => Vec::new(),
            };
            entries
                .into_iter()
                .enumerate()
                .map(|(offset, entry)| line(shell, range.start + offset, entry, window, cx))
                .collect()
        }),
    )
    .track_scroll(&scroll)
    .flex_1();

    div()
        .size_full()
        .flex()
        .flex_col()
        .pt(px(96.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px(px(MARGIN))
                .pb(px(24.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(14.))
                        .child(
                            div()
                                .text_size(px(28.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(name),
                        )
                        .children(count.map(|count| {
                            div()
                                .text_size(px(14.))
                                .text_color(theme::dim())
                                .child(count)
                        })),
                )
                .children(play_all),
        )
        .children(status.map(|why| div().px(px(MARGIN)).child(note(why))))
        .child(list)
        .into_any_element()
}

/// A title of the playlist: its place, its picture, what it is, and its buttons. A click on the
/// line opens its page; the buttons play it, or take it out. "Remove" shows only while the
/// pointer is over the line, and stands apart from Play.
fn line(
    shell: &mut Shell,
    at: usize,
    entry: PlaylistEntry,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> gpui_kit::Div {
    let still = shell.image(playlists::still(&entry.title), images::POSTER, cx);
    let opened = entry.title.clone();
    let removed = entry.id.clone();
    let (name, caption) = (
        playlists::name(&entry.title),
        playlists::caption(&entry.title),
    );
    let group = SharedString::from(format!("playlist-entry-{at}"));
    let play = round_button(("playlist-entry-play", at), IconName::Play, window, cx).on_click(
        cx.listener(move |shell, _, window, cx| {
            cx.stop_propagation();
            shell.play_entry(entry.clone(), false, window, cx);
        }),
    );
    let remove = theme::ghost_button(("playlist-entry-remove", at), None, "Remove", window, cx)
        .debug_selector(move || format!("playlist-entry-remove {at}"))
        .invisible()
        .group_hover(group.clone(), |style| style.visible())
        .on_click(cx.listener(move |shell, _, _, cx| {
            cx.stop_propagation();
            shell.remove_entry(removed.clone(), cx);
        }));
    div().px(px(MARGIN - 12.)).pb(px(8.)).child(
        theme::eased(("playlist-entry", at), theme::Shades::line(), window, cx)
            .group(group)
            .h(px(LINE - 8.))
            .px(px(12.))
            .rounded(px(theme::RADIUS))
            .flex()
            .items_center()
            .gap(px(16.))
            .cursor_pointer()
            .on_click(cx.listener(move |shell, _, _, cx| shell.open_entry(opened.clone(), cx)))
            .child(
                div()
                    .w(px(24.))
                    .flex_none()
                    .text_size(px(13.))
                    .text_color(theme::dim())
                    .child((at + 1).to_string()),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(STILL_WIDTH))
                    .h(px(STILL_WIDTH * 9. / 16.))
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(theme::panel())
                    .children(super::picture(still, 6.)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::dim())
                            .child(caption),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(24.))
                    .child(remove)
                    .child(play),
            ),
    )
}

/// A small round button of a line.
fn round_button(
    id: impl Into<gpui_kit::ElementId>,
    name: IconName,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<gpui_kit::Div> {
    let shades = theme::Shades {
        rest: rgba(0x14141680),
        hover: theme::line(),
        press: rgba(0x2e2e33ff),
    };
    theme::eased(id, shades, window, cx)
        .size(px(34.))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(theme::line())
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(icon(name, 14.))
}

/// "Add to playlist" beside "Favorite": a click opens the menu under it, for `item`.
pub fn playlist_button(
    id: &'static str,
    item: ItemId,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Div {
    let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::default();
    let drawn = bounds.clone();
    div()
        .relative()
        .child(
            theme::ghost_button(id, Some(IconName::ListPlus), "Add to playlist", window, cx)
                .debug_selector(move || id.to_string())
                .on_click(cx.listener(move |shell, _, _, cx| {
                    shell.open_playlist_menu(item.clone(), bounds.get(), cx)
                })),
        )
        // Where the button is drawn, for the menu.
        .child(
            canvas(move |at, _, _| drawn.set(Some(at)), |_, _, _, _| {})
                .absolute()
                .inset_0(),
        )
}

/// The "Add to playlist" menu, over everything, under the button that opened it: the
/// playlists, a check on those that hold the title, and "New playlist…". It fades in rising a
/// little toward its place; only an open menu answers the mouse.
pub fn add_menu(
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let now = Instant::now();
    let menu = shell.playlist_menu.as_ref()?;
    let shown = menu.fade.level(now);
    let open = menu.open;
    let picker = &menu.picker;
    let status = match &picker.playlists {
        Some(Loadable::Failed(why)) => Some(*why),
        Some(Loadable::Ready(_)) => None,
        _ => Some("Loading…"),
    };
    let lines = picker.lines();
    let error = picker.error;
    let told = picker.told.clone();
    let naming = menu.naming.clone();
    let anchor = menu.anchor;

    let mut rows: Vec<AnyElement> = Vec::new();
    rows.extend(status.map(|why| {
        div()
            .h(px(menu::LINE))
            .flex_none()
            .px(px(12.))
            .flex()
            .items_center()
            .text_color(theme::dim())
            .child(why)
            .into_any_element()
    }));
    for (index, line) in lines.iter().enumerate() {
        let id = line.id.clone();
        let takes = open && !line.checked && id.is_some();
        let name = line.name.clone();
        rows.push(
            theme::eased(("playlist-line", index), theme::Shades::line(), window, cx)
                .debug_selector(move || format!("playlist-line {name}"))
                .flex()
                .items_center()
                .gap(px(10.))
                .flex_none()
                .h(px(menu::LINE))
                .px(px(12.))
                .rounded(px(7.))
                .when(takes, |row| {
                    row.cursor_pointer()
                        .on_click(cx.listener(move |shell, _, _, cx| {
                            if let Some(id) = &id {
                                shell.pick_playlist(id.clone(), cx);
                            }
                        }))
                })
                .child(
                    div()
                        .size(px(14.))
                        .flex_none()
                        .text_color(theme::accent())
                        .when(line.checked, |mark| mark.child(icon(IconName::Check, 14.))),
                )
                .child(div().truncate().child(line.name.clone()))
                .into_any_element(),
        );
    }
    rows.push(
        div()
            .flex_none()
            .h(px(1.))
            .mx(px(6.))
            .my(px(6.))
            .bg(theme::line())
            .into_any_element(),
    );
    rows.push(match naming {
        Some(input) => div()
            .h(px(FIELD))
            .flex_none()
            .px(px(6.))
            .flex()
            .items_center()
            .child(
                div()
                    .debug_selector(|| "playlist-name".into())
                    .w_full()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(7.))
                    .border_1()
                    .border_color(theme::muted())
                    .bg(theme::background())
                    .child(input),
            )
            .into_any_element(),
        None => theme::eased("playlist-new", theme::Shades::line(), window, cx)
            .debug_selector(|| "playlist-new".into())
            .flex()
            .items_center()
            .gap(px(10.))
            .flex_none()
            .h(px(menu::LINE))
            .px(px(12.))
            .rounded(px(7.))
            .when(open, |row| {
                row.cursor_pointer()
                    .on_click(cx.listener(|shell, _, window, cx| shell.start_naming(window, cx)))
            })
            .child(icon(IconName::Plus, 14.).text_color(theme::muted()))
            .child("New playlist…")
            .into_any_element(),
    });
    let said = error.is_some() || told.is_some();
    rows.extend(error.map(|why| {
        div()
            .px(px(12.))
            .py(px(8.))
            .flex_none()
            .text_size(px(12.))
            .text_color(theme::danger())
            .child(why)
            .into_any_element()
    }));
    rows.extend(told.filter(|_| error.is_none()).map(|said| {
        div()
            .debug_selector(|| "playlist-told".into())
            .px(px(12.))
            .py(px(8.))
            .flex_none()
            .text_size(px(12.))
            .text_color(theme::muted())
            .child(said)
            .into_any_element()
    }));

    let window_size = window.viewport_size();
    // The rule and the field or "New playlist…" are counted apart.
    let wanted = menu::height(rows.len() - 2) + FIELD + if said { 32. } else { 0. };
    // Opened with no button to stand under: a third of the way down, in the middle.
    let button = anchor.unwrap_or_else(|| {
        Bounds::new(
            point(
                (window_size.width - px(MENU_WIDTH)) / 2.,
                window_size.height / 3.,
            ),
            gpui_kit::size(px(MENU_WIDTH), px(1.)),
        )
    });
    let (at, height) = menu::place(button, wanted, f32::from(window_size.height));
    let toward_button = if at.y > button.top() { -8. } else { 8. };
    let rise = point(px(0.), px(toward_button * (1. - shown)));
    Some(
        deferred(
            anchored()
                .position(at + rise)
                .snap_to_window_with_margin(px(menu::MARGIN))
                .child(
                    div()
                        .id("playlist-menu")
                        .debug_selector(|| "playlist-menu".into())
                        .opacity(shown)
                        .max_h(px(height))
                        .overflow_y_scroll()
                        .w(px(MENU_WIDTH))
                        .p(px(6.))
                        .flex()
                        .flex_col()
                        .rounded(px(theme::RADIUS_LARGE))
                        .border_1()
                        .border_color(theme::line())
                        .bg(theme::panel())
                        .text_size(px(13.))
                        .when(open, |menu| {
                            menu.occlude().on_mouse_down_out(cx.listener(
                                |shell, event: &MouseDownEvent, _, cx| {
                                    // A side button is Back: it closes the menu itself, once.
                                    if matches!(event.button, MouseButton::Navigate(_)) {
                                        return;
                                    }
                                    shell.pressed_outside_playlist_menu(event.position, cx);
                                },
                            ))
                        })
                        .children(rows),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}
