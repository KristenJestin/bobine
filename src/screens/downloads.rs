//! The Downloads page: what is kept on disk and what is coming, films first, then each series by
//! season, with the disk's use at the top. Each line has its poster, its size, its state, a bar
//! and its speed while it comes, and its buttons; a line removed fades and collapses.

use std::time::Instant;

use bobine::provider::{ImageRef, ItemId};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt, AnyElement, Context, Div, FontWeight, IntoElement, SharedString,
    Stateful, Window, div, px, rgba,
};

use super::home_list::page_under_the_bar;
use super::{CONTENT_WIDTH, note, picture, row_heading};
use crate::downloads::{Download, Halt, Snapshot, State};
use crate::grid::MARGIN;
use crate::icons::{IconName, icon};
use crate::images;
use crate::motion::{self, Glide};
use crate::shell::Shell;
use crate::theme;

/// One line, its gap included: what a line removed collapses from.
const LINE: f32 = 96.;
const POSTER_WIDTH: f32 = 54.;
const BAR_WIDTH: f32 = 220.;

/// What the film page's and an episode's button says: Download, how much has come, Downloaded.
pub fn button_label(download: Option<&Download>) -> String {
    match download {
        None => "Download".into(),
        Some(download) if download.manifest.state == State::Done => "Downloaded".into(),
        Some(download) => format!("{}%", percent(download)),
    }
}

/// How much of the file has come, from 0 to 100, rounded down: 100 only once it has all.
fn percent(download: &Download) -> u64 {
    match download.manifest.size {
        Some(size) if size > 0 => (download.received.min(size) * 100 / size).min(99),
        _ => 0,
    }
}

/// `1.4 GB`, `350 MB`.
pub fn bytes(count: u64) -> String {
    if count >= 1_000_000_000 {
        format!("{:.1} GB", count as f64 / 1e9)
    } else {
        format!("{} MB", count / 1_000_000)
    }
}

/// Where a download is, in words.
fn state_text(download: &Download, speed: Option<f64>) -> String {
    let done = percent(download);
    match &download.manifest.state {
        State::Queued => "Queued".into(),
        State::Downloading => match speed {
            Some(speed) => format!("{done}% · {}/s", bytes(speed as u64)),
            None => format!("{done}%"),
        },
        State::Paused(Halt::Asked) => format!("Paused · {done}%"),
        State::Paused(Halt::DiskFull) => format!("Disk full · {done}%"),
        State::Paused(Halt::NoRoom) => format!("Not enough space · {done}%"),
        State::Done => "Downloaded".into(),
        State::Failed(why) => why.clone(),
    }
}

/// The films, then each series by title, its seasons and episodes in their order.
#[derive(Debug, PartialEq)]
pub struct Sections<'a> {
    pub films: Vec<&'a Download>,
    pub series: Vec<SeriesGroup<'a>>,
}

#[derive(Debug, PartialEq)]
pub struct SeriesGroup<'a> {
    pub id: ItemId,
    pub title: String,
    pub seasons: Vec<SeasonGroup<'a>>,
}

#[derive(Debug, PartialEq)]
pub struct SeasonGroup<'a> {
    pub id: ItemId,
    pub number: Option<u32>,
    pub episodes: Vec<&'a Download>,
}

/// `downloads` in the order of the queue, put in their sections.
pub fn sections<'a>(downloads: &[&'a Download]) -> Sections<'a> {
    let mut films = Vec::new();
    let mut series: Vec<SeriesGroup<'a>> = Vec::new();
    for &download in downloads {
        let Snapshot::Episode(episode, _) = &download.manifest.title else {
            films.push(download);
            continue;
        };
        let at = match series.iter().position(|group| group.id == episode.series) {
            Some(at) => at,
            None => {
                series.push(SeriesGroup {
                    id: episode.series.clone(),
                    title: episode.series_title.clone(),
                    seasons: Vec::new(),
                });
                series.len() - 1
            }
        };
        let seasons = &mut series[at].seasons;
        let at = match seasons.iter().position(|group| group.id == episode.season) {
            Some(at) => at,
            None => {
                seasons.push(SeasonGroup {
                    id: episode.season.clone(),
                    number: episode.season_number,
                    episodes: Vec::new(),
                });
                seasons.len() - 1
            }
        };
        seasons[at].episodes.push(download);
    }
    series.sort_by(|a, b| a.title.cmp(&b.title));
    for group in &mut series {
        group.seasons.sort_by_key(|season| season.number);
        for season in &mut group.seasons {
            season
                .episodes
                .sort_by_key(|download| match &download.manifest.title {
                    Snapshot::Episode(episode, _) => episode.number,
                    Snapshot::Film(_) => None,
                });
        }
    }
    Sections { films, series }
}

fn season_heading(number: Option<u32>) -> String {
    match number {
        Some(0) => "Specials".into(),
        Some(n) => format!("Season {n}"),
        None => "Episodes".into(),
    }
}

pub fn render(shell: &mut Shell, window: &mut Window, cx: &mut Context<Shell>) -> impl IntoElement {
    let now = Instant::now();
    // The lines removed stay drawn while they fade and collapse, at their place.
    let duration = motion::page();
    shell
        .downloads
        .leaving
        .retain(|(_, since)| now.duration_since(*since) < duration);
    if !shell.downloads.leaving.is_empty() {
        window.request_animation_frame();
    }
    let listed = shell.downloads.listed.clone();
    let leaving: Vec<(Download, f32)> = shell
        .downloads
        .leaving
        .iter()
        .filter(|(gone, _)| !listed.iter().any(|d| d.item == gone.item))
        .map(|(gone, since)| {
            let t = now.duration_since(*since).as_secs_f32() / duration.as_secs_f32();
            (gone.clone(), motion::ease_out(t))
        })
        .collect();
    let mut all: Vec<&Download> = listed
        .iter()
        .chain(leaving.iter().map(|(d, _)| d))
        .collect();
    all.sort_by_key(|download| download.manifest.order);
    let gone = |item: &ItemId| {
        leaving
            .iter()
            .find(|(d, _)| &d.item == item)
            .map(|(_, t)| *t)
    };

    let used: u64 = listed.iter().map(|download| download.received).sum();
    let disk = match shell.downloads.free {
        Some(free) => format!("{} used · {} free", bytes(used), bytes(free)),
        None => format!("{} used", bytes(used)),
    };
    let heading = div()
        .flex()
        .items_baseline()
        .gap(px(14.))
        .pb(px(24.))
        .child(
            div()
                .text_size(px(28.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Downloads"),
        )
        .child(
            div()
                .debug_selector(|| "downloads-disk".into())
                .text_size(px(14.))
                .text_color(theme::dim())
                .child(disk),
        );

    let sections = sections(&all);
    let mut lines: Vec<AnyElement> = Vec::new();
    if sections.films.is_empty() && sections.series.is_empty() {
        lines.push(
            note("Nothing is downloaded yet. A movie's or an episode's Download keeps it here.")
                .into_any_element(),
        );
    }
    if !sections.films.is_empty() {
        lines.push(row_heading("Movies").pb(px(4.)).into_any_element());
    }
    for download in &sections.films {
        lines.push(line(shell, download, gone(&download.item), now, window, cx));
    }
    for series in &sections.series {
        let id = series.id.0.clone();
        lines.push(
            row_heading(series.title.clone())
                .debug_selector(move || format!("downloads-series {id}"))
                .pt(px(16.))
                .pb(px(4.))
                .into_any_element(),
        );
        for season in &series.seasons {
            let id = season.id.0.clone();
            lines.push(
                div()
                    .debug_selector(move || format!("downloads-season {id}"))
                    .pt(px(6.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::muted())
                    .child(season_heading(season.number))
                    .into_any_element(),
            );
            for download in &season.episodes {
                lines.push(line(shell, download, gone(&download.item), now, window, cx));
            }
        }
    }

    let scroll = shell.downloads.scroll.clone();
    shell.scrolls.push(scroll.clone());
    page_under_the_bar().child(
        div()
            .id("downloads-page")
            .debug_selector(|| "downloads-page".into())
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .child(
                div()
                    .px(px(MARGIN))
                    .pb(px(64.))
                    .max_w(px(CONTENT_WIDTH + 2. * MARGIN))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(heading)
                    .children(lines),
            ),
    )
}

/// The poster a line shows: the film's, or the episode's series'.
fn poster_of(download: &Download) -> Option<&ImageRef> {
    match &download.manifest.title {
        Snapshot::Film(film) => film.film.poster.as_ref(),
        Snapshot::Episode(_, series) => series.series.poster.as_ref(),
    }
}

/// What a line says of its title: the film's title and year, or the episode's number and title.
fn names(download: &Download) -> (String, Option<String>, bool) {
    match &download.manifest.title {
        Snapshot::Film(film) => (
            film.film.title.clone(),
            film.film.year.map(|year| year.to_string()),
            film.film.watched,
        ),
        Snapshot::Episode(episode, _) => {
            let title = match episode.number {
                Some(number) => format!("{number} · {}", episode.item.title),
                None => episode.item.title.clone(),
            };
            (title, None, episode.item.watched)
        }
    }
}

/// One download. `gone`: it was removed, and is this far through fading and collapsing.
fn line(
    shell: &mut Shell,
    download: &Download,
    gone: Option<f32>,
    now: Instant,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let id = download.item.0.clone();
    let poster = shell.image(poster_of(download), images::POSTER, cx);
    let (title, year, watched) = names(download);
    let state = &download.manifest.state;
    let speed = shell.downloads.speed(&download.item);
    let said = state_text(download, speed);
    let coming = !matches!(state, State::Done);
    let share = match download.manifest.size {
        Some(size) if size > 0 => download.received.min(size) as f32 / size as f32,
        _ => 0.,
    };
    // The bar eases to each new share instead of jumping.
    let bar = shell
        .downloads
        .bars
        .entry(download.item.clone())
        .or_insert_with(|| Glide::new(share, motion::quick()));
    bar.set(share, now);
    if bar.moving(now) {
        window.request_animation_frame();
    }
    let shown = bar.value(now);

    let item = download.item.clone();
    let mut buttons: Vec<Stateful<Div>> = Vec::new();
    if matches!(state, State::Queued | State::Downloading) {
        let item = item.clone();
        buttons.push(
            small_button(
                SharedString::from(format!("download-pause {id}")),
                IconName::Pause,
                "Pause",
                window,
                cx,
            )
            .debug_selector({
                let id = id.clone();
                move || format!("download-pause {id}")
            })
            .on_click(cx.listener(move |shell, _, _, cx| shell.pause_download(item.clone(), cx))),
        );
    }
    if matches!(state, State::Paused(_) | State::Failed(_)) {
        let item = item.clone();
        buttons.push(
            small_button(
                SharedString::from(format!("download-resume {id}")),
                IconName::Play,
                "Resume",
                window,
                cx,
            )
            .debug_selector({
                let id = id.clone();
                move || format!("download-resume {id}")
            })
            .on_click(cx.listener(move |shell, _, _, cx| shell.resume_download(item.clone(), cx))),
        );
    }
    let (remove, label, name) = if coming {
        (IconName::X, "Cancel", "download-cancel")
    } else {
        (IconName::Trash, "Remove", "download-remove")
    };
    buttons.push(
        small_button(
            SharedString::from(format!("{name} {id}")),
            remove,
            label,
            window,
            cx,
        )
        .debug_selector({
            let id = id.clone();
            move || format!("{name} {id}")
        })
        .on_click(cx.listener(move |shell, _, _, cx| shell.remove_download(item.clone(), cx))),
    );

    let size = download.manifest.size.map(bytes);
    let row = div()
        .debug_selector({
            let id = id.clone();
            move || format!("download-row {id}")
        })
        .h(px(LINE - 6.))
        .flex()
        .items_center()
        .gap(px(16.))
        .p(px(8.))
        .rounded(px(theme::RADIUS))
        .child(
            div()
                .relative()
                .flex_none()
                .w(px(POSTER_WIDTH))
                .h(px(POSTER_WIDTH * 1.5))
                .rounded(px(theme::RADIUS))
                .overflow_hidden()
                .bg(theme::panel())
                .children(picture(poster, theme::RADIUS)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .children(year.map(|year| {
                            div()
                                .text_size(px(12.))
                                .text_color(theme::dim())
                                .child(year)
                        }))
                        .children(watched.then(|| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(4.))
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(icon(IconName::Check, 12.))
                                .child("Watched")
                        })),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .children(size.map(|size| div().child(size)))
                        .child(
                            div()
                                .text_color(match state {
                                    State::Failed(_)
                                    | State::Paused(Halt::DiskFull | Halt::NoRoom) => {
                                        theme::danger()
                                    }
                                    _ => theme::muted(),
                                })
                                .child(said),
                        ),
                )
                .children(coming.then(|| download_bar(shown, BAR_WIDTH))),
        )
        .child(div().flex_none().flex().gap(px(8.)).children(buttons));

    match gone {
        // Removed: it fades and its room closes.
        Some(t) => div()
            .h(px(LINE * (1. - t)))
            .opacity(1. - t)
            .overflow_hidden()
            .child(row)
            .into_any_element(),
        // A new line fades in.
        None => div()
            .id(SharedString::from(format!("download-line {id}")))
            .child(row)
            .with_animation(
                SharedString::from(format!("download-line-in {id}")),
                Animation::new(motion::page()).with_easing(motion::ease_out),
                |line, t| line.opacity(t),
            )
            .into_any_element(),
    }
}

/// How much of a download has come: near-white, not the coral of what was watched.
fn download_bar(share: f32, width: f32) -> Div {
    div()
        .w(px(width))
        .h(px(3.))
        .rounded_full()
        .bg(rgba(0xffffff26))
        .child(
            div()
                .h_full()
                .w(px(width * share.clamp(0., 1.)))
                .rounded_full()
                .bg(theme::accent()),
        )
}

/// A line's button: an icon and a word, outlined, small.
fn small_button(
    id: impl Into<gpui_kit::ElementId>,
    name: IconName,
    label: &'static str,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Stateful<Div> {
    theme::ghost_button(id, Some(name), label, window, cx)
        .px(px(12.))
        .py(px(6.))
        .text_size(px(13.))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::downloads::{FORMAT, Manifest};
    use crate::fake::{FakeServer, SERIES};
    use bobine::provider::{MediaServer, MediaSourceId, Title};

    fn download(title: Snapshot, order: u64, state: State, received: u64) -> Download {
        Download {
            item: title.item().clone(),
            manifest: Manifest {
                format: FORMAT,
                order,
                title,
                source: MediaSourceId("source".into()),
                size: Some(200),
                version: "v1".into(),
                container: Some("mkv".into()),
                validator: None,
                subtitles: Vec::new(),
                segments: Vec::new(),
                trickplay: None,
                state,
                gone: false,
                update: false,
            },
            received,
        }
    }

    fn episode(server: &FakeServer, id: &str) -> Snapshot {
        let Title::Episode(episode) = server.title(&ItemId(id.into())).unwrap() else {
            panic!("an episode");
        };
        let series = server.series_detail(&episode.series).unwrap();
        Snapshot::Episode(Box::new(episode), Box::new(series))
    }

    #[test]
    fn the_button_says_download_then_how_much_came_then_downloaded() {
        let server = FakeServer::new(2, true);
        let film = Snapshot::Film(Box::new(server.film(&ItemId("film-1".into())).unwrap()));
        assert_eq!(button_label(None), "Download");
        let coming = download(film.clone(), 0, State::Downloading, 84);
        assert_eq!(button_label(Some(&coming)), "42%");
        // All the bytes, the extras still coming: not yet 100.
        let almost = download(film.clone(), 0, State::Downloading, 200);
        assert_eq!(button_label(Some(&almost)), "99%");
        let done = download(film, 0, State::Done, 200);
        assert_eq!(button_label(Some(&done)), "Downloaded");
    }

    #[test]
    fn films_come_first_then_each_series_by_season_and_episode() {
        let server = FakeServer::new(2, true);
        let film = Snapshot::Film(Box::new(server.film(&ItemId("film-1".into())).unwrap()));
        let all = [
            download(episode(&server, "series-0-s2-e1"), 0, State::Queued, 0),
            download(episode(&server, "series-0-s1-e2"), 1, State::Queued, 0),
            download(film, 2, State::Queued, 0),
            download(episode(&server, "series-0-s1-e1"), 3, State::Queued, 0),
        ];
        let refs: Vec<&Download> = all.iter().collect();
        let found = sections(&refs);
        assert_eq!(found.films.len(), 1);
        assert_eq!(found.series.len(), 1);
        assert_eq!(found.series[0].id.0, SERIES);
        let order: Vec<Vec<&str>> = found.series[0]
            .seasons
            .iter()
            .map(|season| season.episodes.iter().map(|d| d.item.0.as_str()).collect())
            .collect();
        assert_eq!(
            order,
            [
                vec!["series-0-s1-e1", "series-0-s1-e2"],
                vec!["series-0-s2-e1"]
            ]
        );
    }

    #[test]
    fn a_line_says_where_its_download_is() {
        let server = FakeServer::new(2, true);
        let film = Snapshot::Film(Box::new(server.film(&ItemId("film-1".into())).unwrap()));
        let at = |state| download(film.clone(), 0, state, 100);
        assert_eq!(state_text(&at(State::Queued), None), "Queued");
        assert_eq!(
            state_text(&at(State::Downloading), Some(4_200_000.)),
            "50% · 4 MB/s"
        );
        assert_eq!(
            state_text(&at(State::Paused(Halt::DiskFull)), None),
            "Disk full · 50%"
        );
        assert_eq!(state_text(&at(State::Done), None), "Downloaded");
        assert_eq!(bytes(18_400_000_000), "18.4 GB");
        assert_eq!(bytes(350_000_000), "350 MB");
    }
}
