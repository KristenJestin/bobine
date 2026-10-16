//! A playlist's page: its name and count, "Play all", then its titles in their order, one per
//! line. Only the lines in view are built, so only their pictures are asked.

use std::ops::Range;

use bobine::provider::PlaylistEntry;
use gpui_kit::prelude::*;
use gpui_kit::{Context, FontWeight, IntoElement, Stateful, Window, div, px, rgba, uniform_list};

use super::note;
use crate::grid::MARGIN;
use crate::icons::{IconName, icon};
use crate::images;
use crate::playlists;
use crate::shell::{Loadable, Shell};
use crate::theme;

/// One line of the list, its gap included.
const LINE: f32 = 92.;
/// The picture of a line, wide.
const STILL_WIDTH: f32 = 128.;

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let Some(page) = shell.playlist_pages.last() else {
        return div().into_any_element();
    };
    let (len, status) = match &page.entries {
        Loadable::Ready(entries) if entries.is_empty() => (
            0,
            Some("This playlist is empty. Add a film or an episode from its page."),
        ),
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
/// line opens its page; the buttons play it, or take it out.
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
    let play = round_button(("playlist-entry-play", at), IconName::Play, window, cx).on_click(
        cx.listener(move |shell, _, window, cx| {
            cx.stop_propagation();
            shell.play_entry(at, window, cx);
        }),
    );
    let remove = round_button(("playlist-entry-remove", at), IconName::X, window, cx)
        .debug_selector(move || format!("playlist-entry-remove {at}"))
        .on_click(cx.listener(move |shell, _, _, cx| {
            cx.stop_propagation();
            shell.remove_entry(removed.clone(), cx);
        }));
    div().px(px(MARGIN - 12.)).pb(px(8.)).child(
        theme::eased(("playlist-entry", at), theme::Shades::line(), window, cx)
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
                            .child(playlists::name(&entry.title)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::dim())
                            .child(playlists::caption(&entry.title)),
                    ),
            )
            .child(div().flex().gap(px(8.)).child(play).child(remove)),
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
