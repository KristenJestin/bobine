//! The rows of a list's page and the episode rows of a series page keep to a content width: the
//! drawn bounds are measured in a wide window and in a narrow one.

use std::sync::Arc;

use bobine::provider::ItemId;
use gpui_kit::{Bounds, Pixels, TestAppContext, VisualTestContext, WindowHandle, px, size};

use crate::fake::{FakeServer, SERIES};
use crate::grid::MARGIN;
use crate::navigation::NavEntry;
use crate::screens::CONTENT_WIDTH;
use crate::shell::Shell;

/// What a bound is allowed to be off by.
const SLACK: f32 = 0.5;
/// The padding of a row, between its edge and its last button.
const ROW_PADDING: f32 = 12.;
/// A window wider than the content width and its margins, and one narrower.
const WIDE: f32 = 2400.;
const NARROW: f32 = 800.;

fn signed_in(cx: &mut TestAppContext) -> WindowHandle<Shell> {
    let server = FakeServer::new(6, true);
    let connector: Arc<dyn bobine::provider::Connector> = Arc::new(server);
    let window = cx.add_window(|_, cx| {
        crate::keys::bind(cx);
        Shell::new(
            None,
            Box::new(move || Ok(connector)),
            None,
            Arc::new(crate::fake::MemoryKeyring::default()),
            cx,
        )
    });
    cx.run_until_parked();
    window
}

/// The window `width` wide, drawn until the page it shows is in place.
fn drawn(
    window: &WindowHandle<Shell>,
    width: f32,
    cx: &mut TestAppContext,
) -> &'static mut VisualTestContext {
    let visual = VisualTestContext::from_window((*window).into(), cx).into_mut();
    visual.simulate_resize(size(px(width), px(1200.)));
    for _ in 0..3 {
        visual.run_until_parked();
    }
    // The page's slide is the only animation on the real clock (the others go by the test's
    // own): a row is measured once it is in place.
    std::thread::sleep(crate::motion::page());
    visual.update(|window, _| window.refresh());
    visual.run_until_parked();
    visual
}

/// The Watchlist's page, in a window `width` wide.
fn list_page(width: f32, cx: &mut TestAppContext) -> &'static mut VisualTestContext {
    let window = signed_in(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.select(NavEntry::MyList, cx);
            shell.open_playlist(ItemId("playlist-watchlist".into()), cx);
        })
        .unwrap();
    cx.run_until_parked();
    drawn(&window, width, cx)
}

/// A series' page, in a window `width` wide.
fn series_page(width: f32, cx: &mut TestAppContext) -> &'static mut VisualTestContext {
    let window = signed_in(cx);
    window
        .update(cx, |shell, _, cx| {
            shell.open_series(ItemId(SERIES.into()), cx)
        })
        .unwrap();
    cx.run_until_parked();
    drawn(&window, width, cx)
}

fn bounds(visual: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    (visual.debug_bounds(selector)).unwrap_or_else(|| panic!("{selector} was not drawn"))
}

fn width(of: Bounds<Pixels>) -> f32 {
    f32::from(of.size.width)
}

fn right(of: Bounds<Pixels>) -> f32 {
    f32::from(of.right())
}

#[gpui_kit::test]
fn a_list_row_is_no_wider_than_the_content_width_on_a_wide_window(cx: &mut TestAppContext) {
    let visual = list_page(WIDE, cx);
    let row = bounds(visual, "playlist-entry 0");
    let play = bounds(visual, "playlist-entry-play 0");
    assert!(
        (width(row) - CONTENT_WIDTH).abs() <= SLACK,
        "{row:?} is not {CONTENT_WIDTH} wide"
    );
    // The buttons are at the end of the row, not at the window's edge.
    assert!(
        right(row) - right(play) <= ROW_PADDING + SLACK,
        "{play:?} is not at the end of {row:?}"
    );
}

#[gpui_kit::test]
fn an_episode_row_is_no_wider_than_the_content_width_on_a_wide_window(cx: &mut TestAppContext) {
    let visual = series_page(WIDE, cx);
    let row = bounds(visual, "episode 0");
    let check = bounds(visual, "episode-watched 0");
    assert!(
        (width(row) - CONTENT_WIDTH).abs() <= SLACK,
        "{row:?} is not {CONTENT_WIDTH} wide"
    );
    // The watched check is at the end of the row, beside its content.
    assert!(
        right(row) - right(check) <= 2. * ROW_PADDING,
        "{check:?} is not at the end of {row:?}"
    );
}

#[gpui_kit::test]
fn a_narrow_window_keeps_its_rows_as_wide_as_it_allows(cx: &mut TestAppContext) {
    // The window less its margins: the rows fill what they are given.
    let room = NARROW - 2. * (MARGIN - ROW_PADDING);
    let visual = list_page(NARROW, cx);
    let row = bounds(visual, "playlist-entry 0");
    assert!(
        (width(row) - room).abs() <= SLACK,
        "{row:?} is not {room} wide"
    );
    let visual = series_page(NARROW, cx);
    let row = bounds(visual, "episode 0");
    assert!(
        width(row) > room - 2. * MARGIN && width(row) < CONTENT_WIDTH,
        "{row:?} does not fill a narrow window"
    );
}

#[gpui_kit::test]
fn the_play_all_button_lines_up_with_the_end_of_the_rows(cx: &mut TestAppContext) {
    let visual = list_page(WIDE, cx);
    let play_all = bounds(visual, "playlist-play-all");
    let play = bounds(visual, "playlist-entry-play 0");
    assert!(
        (right(play_all) - right(play)).abs() <= SLACK,
        "{play_all:?} is not above {play:?}"
    );
    assert!(
        right(play_all) <= MARGIN + CONTENT_WIDTH,
        "{play_all:?} is past the content width"
    );
}
