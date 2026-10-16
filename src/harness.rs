//! A startup path for the headless checks, built only with the `harness` feature: it plays a
//! local file at once and reaches, without input, the states a nested compositor with no seat
//! cannot reach by hand (controls shown, panel open, fullscreen), then prints one `RESULT` line
//! with the numbers the pull request reports. Or it shows a screen with invented films from the
//! fake server. Never part of the application a user runs; it never touches the account file,
//! the keyring or a real server.
//!
//! `bobine --play FILE [--title T] [--sub URL] [--subtitle-track ID] [--controls] [--panel]
//! [--fullscreen-at S] [--windowed-at S] [--next FILE --next-at S] [--leave-at S]
//! [--measure-from S] --quit-at S`
//!
//! `bobine --screen login|home|home-rows|home-list-resume|home-list-next|home-list-latest|home-list-empty|library|detail|
//! detail-rows|detail-cast|playing|series|series-page|series-rows|up-next|collections|collection-page|
//! my-list|my-list-empty|film-list|series-list [--films N]
//! [--media FILE] [--segments SECONDS] [--direct-fails] [--trailer] [--image-delay-ms N] [--photos-fail-once] [--scenario menu|scroll|select|panel|blackout|search|search-slow|search-keys|tools|tools-keys|home-rotate|home-hover|home-jump|home-scroll|home-lists|home-play|home-poster-hover|grid-poster-hover|cast-hover|genre-hover|episode-hover|skip|settings|settings-screen|long-tracks|segment-marks]
//! [--search-delay-ms N] [--favourite-delay-ms N] [--logout-delay-ms N] [--favourite-fails] [--slower N] --quit-at S`
//!
//! A scenario sends clicks, keys and scroll events to the window as the compositor would, at set
//! times, and prints what they did (`MENU`, `SCROLL`, `SELECT`, `PANEL` lines) and the frame times
//! that followed each (`FRAMES`), and how often the shell and the Home's header were drawn
//! (`DRAWS`). `--segments N` gives the fake server's films the segments of a media N seconds long:
//! a recap 0 to 4 s, an intro 5 to 15 s, the credits for the last 15 s.
//! `--slower N` makes every move N times slower, to photograph it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobine::provider::{Connector, ItemId, Segment, SegmentKind, Trickplay};
use gpui_kit::{
    App, Bounds, Context, Entity, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta, ScrollWheelEvent,
    TouchPhase, WeakEntity, Window, point, px,
};

use crate::chrome::{MENU_HEADER, MENU_RULE};
use crate::fake::{FILM, FakeServer, SERIES};
use crate::film_page::From;
use crate::home_page::ListKind;
use crate::library_tools::LETTERS;
use crate::menu;
use crate::navigation::{NavEntry, Screen};
use crate::screens::player::EDGE;
use crate::shell::{Chip, Tool};

use crate::player::libmpv::Libmpv;
use crate::player::mpv::MpvClient;
use crate::player::{ExternalSubtitle, FirstSubtitle, Media, Stream};
use crate::shell::{Film, Shell};

/// `/proc` counts CPU time in USER_HZ ticks, 100 a second on every Linux architecture.
const USER_HZ: u64 = 100;

#[derive(Default)]
struct Args {
    file: String,
    title: Option<String>,
    sub: Option<String>,
    subtitle_track: Option<i64>,
    controls: bool,
    panel: bool,
    fullscreen_at: Option<f64>,
    windowed_at: Option<f64>,
    /// A second film, played after leaving the first, as a user would.
    next: Option<(String, f64)>,
    leave_at: Option<f64>,
    measure_from: f64,
    quit_at: f64,
    /// A screen to show with the fake server instead of a file to play.
    screen: Option<String>,
    /// How many films the fake server holds.
    films: usize,
    /// What the fake server's films play.
    media: Option<String>,
    /// How long that media is: the fake server then marks its recap, intro and credits.
    segments: Option<f64>,
    /// The file played directly is missing: the player falls back on the server's conversion,
    /// which plays `media`.
    direct_fails: bool,
    /// Every film and series of the fake server has a trailer file.
    trailer: bool,
    /// How long each image of the fake server takes, in milliseconds.
    image_delay: u64,
    /// The fake server fails the first time it is asked each person's photo.
    photos_fail_once: bool,
    /// How long the fake server takes to answer a search, in milliseconds.
    search_delay: u64,
    /// Input sent to the window as a user would give it, once the screen shows.
    scenario: Option<String>,
    /// How long the fake server takes to change the user's list, in milliseconds.
    favourite_delay: u64,
    logout_delay: u64,
    /// The fake server fails when the user's list is changed.
    favourite_fails: bool,
    /// JPEG files the fake server's trickplay tiles are, in turn.
    tiles: Vec<String>,
    /// The fake server's trickplay: how many thumbnails, one every so many milliseconds.
    trickplay: Option<(u32, u64)>,
    /// How long the fake server takes to answer each tile, in milliseconds.
    tile_delay: u64,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        measure_from: 3.,
        quit_at: f64::INFINITY,
        films: 1284,
        ..Args::default()
    };
    let mut playing = false;
    let seconds = |value: Option<String>| -> Result<f64, String> {
        value
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| "expected seconds".to_string())
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--play" => {
                parsed.file = args.next().ok_or("--play needs a file")?;
                playing = true;
            }
            "--title" => parsed.title = args.next(),
            "--sub" => parsed.sub = args.next(),
            "--subtitle-track" => {
                parsed.subtitle_track = Some(
                    args.next()
                        .and_then(|id| id.parse().ok())
                        .ok_or("--subtitle-track needs a track id")?,
                );
            }
            "--controls" => parsed.controls = true,
            "--panel" => parsed.panel = true,
            "--fullscreen-at" => parsed.fullscreen_at = Some(seconds(args.next())?),
            "--windowed-at" => parsed.windowed_at = Some(seconds(args.next())?),
            "--next" => {
                let file = args.next().ok_or("--next needs a file")?;
                parsed.next = Some((file, 0.));
            }
            "--next-at" => {
                let at = seconds(args.next())?;
                let next = parsed.next.as_mut().ok_or("--next-at comes after --next")?;
                next.1 = at;
            }
            "--leave-at" => parsed.leave_at = Some(seconds(args.next())?),
            "--measure-from" => parsed.measure_from = seconds(args.next())?,
            "--quit-at" => parsed.quit_at = seconds(args.next())?,
            "--screen" => parsed.screen = Some(args.next().ok_or("--screen needs a name")?),
            "--films" => {
                parsed.films = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--films needs a number")?;
            }
            "--media" => parsed.media = Some(args.next().ok_or("--media needs a file")?),
            "--segments" => parsed.segments = Some(seconds(args.next())?),
            "--photos-fail-once" => parsed.photos_fail_once = true,
            "--direct-fails" => parsed.direct_fails = true,
            "--trailer" => parsed.trailer = true,
            "--search-delay-ms" => {
                parsed.search_delay = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--search-delay-ms needs a number")?;
            }
            "--scenario" => parsed.scenario = args.next(),
            "--tile" => parsed.tiles.push(args.next().ok_or("--tile needs a file")?),
            "--trickplay" => {
                let value = args.next().ok_or("--trickplay needs COUNT:INTERVAL_MS")?;
                parsed.trickplay = value.split_once(':').and_then(|(count, interval)| {
                    Some((count.parse().ok()?, interval.parse().ok()?))
                });
                if parsed.trickplay.is_none() {
                    return Err("--trickplay needs COUNT:INTERVAL_MS".into());
                }
            }
            "--tile-delay-ms" => {
                parsed.tile_delay = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--tile-delay-ms needs a number")?;
            }
            "--favourite-fails" => parsed.favourite_fails = true,
            "--favourite-delay-ms" => {
                parsed.favourite_delay = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--favourite-delay-ms needs a number")?;
            }
            "--logout-delay-ms" => {
                parsed.logout_delay = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--logout-delay-ms needs a number")?;
            }
            "--slower" => {
                let times: u32 = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--slower needs a number")?;
                crate::motion::SLOWER.store(times, std::sync::atomic::Ordering::Relaxed);
            }
            "--image-delay-ms" => {
                parsed.image_delay = args
                    .next()
                    .and_then(|n| n.parse().ok())
                    .ok_or("--image-delay-ms needs a number")?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok((playing || parsed.screen.is_some()).then_some(parsed))
}

struct Snapshot {
    at: Instant,
    cpu: u64,
    dropped: i64,
    delayed: i64,
}

impl Snapshot {
    fn take(mpv: &Libmpv) -> Snapshot {
        let number = |name: &str| mpv.get(name).and_then(|v| v.parse().ok()).unwrap_or(-1);
        Snapshot {
            at: Instant::now(),
            cpu: std::fs::read_to_string("/proc/self/stat")
                .ok()
                .and_then(|stat| cpu_ticks(&stat))
                .unwrap_or(0),
            dropped: number("frame-drop-count"),
            delayed: number("vo-delayed-frame-count"),
        }
    }
}

/// What the command line asked of the harness, if anything.
pub struct Harness {
    args: Option<Args>,
    fake: Option<FakeServer>,
}

impl Harness {
    pub fn from_args() -> Self {
        let args = match parse(std::env::args().skip(1)) {
            Ok(args) => args,
            Err(error) => {
                eprintln!("bobine harness: {error}");
                std::process::exit(2);
            }
        };
        let fake = args.as_ref().map(|args| {
            let fake = FakeServer::new(args.films, args.screen.as_deref() != Some("login"));
            let mut state = fake.state.lock().unwrap();
            if let Some(media) = &args.media {
                state.stream = media.clone();
                state.converted = media.clone();
            }
            if args.direct_fails {
                state.stream = "file:///nowhere-direct-play.mkv".into();
            }
            if args.trailer {
                state.trailers.local = true;
            }
            if let Some(length) = args.segments {
                state.segments = segments_of(length);
            }
            state.image_delay = Duration::from_millis(args.image_delay);
            state.photos_fail_once = args.photos_fail_once;
            state.search_delay = Duration::from_millis(args.search_delay);
            state.favourite_delay = Duration::from_millis(args.favourite_delay);
            state.logout_delay = Duration::from_millis(args.logout_delay);
            state.favourite_fails = args.favourite_fails;
            state.tile_delay = Duration::from_millis(args.tile_delay);
            state.tiles = args
                .tiles
                .iter()
                .map(|file| std::fs::read(file).expect("a tile file the harness can read"))
                .collect();
            // Thumbnails of the size Jellyfin makes by default, 10 by 10 to a tile.
            state.trickplay = args.trickplay.map(|(count, interval)| Trickplay {
                width: 320,
                height: 180,
                columns: 10,
                rows: 10,
                count,
                interval: Duration::from_millis(interval),
            });
            if args.screen.as_deref() == Some("my-list-empty") {
                state.favourites.clear();
            }
            if args.screen.as_deref() == Some("home-list-empty") {
                // Nothing started: "Continue watching" holds nothing.
                state.films.iter_mut().for_each(|film| film.resume = None);
                let episodes = state.episodes.iter_mut();
                episodes.for_each(|episode| episode.item.resume = None);
            }
            if args.scenario.as_deref() == Some("home-play") {
                // The file played is seconds long: what was started resumes near its start.
                let started = Duration::from_secs(1);
                let films = state.films.iter_mut();
                for film in films.filter(|film| film.resume.is_some()) {
                    film.resume = Some(started);
                }
            }
            drop(state);
            fake
        });
        Self { args, fake }
    }

    /// The fake server, whenever the harness runs.
    pub fn connector(&self) -> Option<std::sync::Arc<dyn Connector>> {
        self.fake
            .clone()
            .map(|fake| Arc::new(fake) as Arc<dyn Connector>)
    }

    /// Starts the harness when the command line asks for it; else the application starts as
    /// usual.
    pub fn start(self, shell: &Entity<Shell>, window: &mut Window, cx: &mut App) {
        let Some(args) = self.args else {
            return;
        };
        shell.update(cx, |shell, _| shell.mpv_options = vec![("ao", "null")]);
        match args.screen.clone() {
            Some(screen) => {
                let fake = self.fake.expect("the harness always has its fake server");
                show_screen(screen, args.scenario, args.quit_at, fake, shell, window, cx)
            }
            None => play(args, shell, window, cx),
        }
    }
}

/// The segments of a media `length` seconds long: a recap, an intro, then the credits.
fn segments_of(length: f64) -> Vec<Segment> {
    let segment = |kind, start: f64, end: f64| Segment {
        kind,
        start: Duration::from_secs_f64(start),
        end: Duration::from_secs_f64(end),
    };
    vec![
        segment(SegmentKind::Recap, 0., 4.),
        segment(SegmentKind::Intro, 5., 15.),
        segment(SegmentKind::Outro, (length - 15.).max(0.), length.max(0.)),
    ]
}

/// Reaches `screen` as a user would, by the shell's own steps, then waits to be photographed.
fn show_screen(
    screen: String,
    scenario: Option<String>,
    quit_at: f64,
    fake: FakeServer,
    shell: &Entity<Shell>,
    window: &mut Window,
    cx: &mut App,
) {
    // Every notification of the shell, which builds the whole window again.
    cx.observe(shell, |shell, cx| {
        shell
            .read(cx)
            .draws
            .borrow_mut()
            .notified
            .push(Instant::now());
    })
    .detach();
    let shell = shell.downgrade();
    let handle = window.window_handle();
    let begun = Instant::now();
    let wants = |names: &[&str]| names.contains(&screen.as_str());
    let to_library = wants(&[
        "library",
        "detail",
        "detail-rows",
        "detail-cast",
        "playing",
        "film-list",
    ]);
    let to_film = wants(&[
        "detail",
        "detail-rows",
        "detail-cast",
        "playing",
        "film-list",
    ]);
    let (scroll, play) = (wants(&["detail-rows"]), wants(&["playing"]));
    let to_cast = wants(&["detail-cast"]);
    let to_series = wants(&[
        "series",
        "series-page",
        "series-rows",
        "up-next",
        "series-list",
    ]);
    let to_series_page = wants(&["series-page", "series-rows", "up-next", "series-list"]);
    let to_collections = wants(&["collections", "collection-page"]);
    let to_collection = wants(&["collection-page"]);
    let to_list = wants(&["my-list", "my-list-empty"]);
    // The "My list" button pressed on the page that shows.
    let press_list = wants(&["film-list", "series-list"]);
    let (series_rows, up_next) = (wants(&["series-rows"]), wants(&["up-next"]));
    let home_rows = wants(&["home-rows"]);
    let list = match screen.as_str() {
        "home-list-resume" | "home-list-empty" => Some((ListKind::Resume, None)),
        "home-list-next" => Some((ListKind::NextUp, None)),
        "home-list-latest" => Some((
            ListKind::Latest(
                ItemId(crate::fake::FILMS_LIBRARY.into()),
                bobine::provider::LibraryKind::Films,
            ),
            Some("Films".to_string()),
        )),
        _ => None,
    };
    let mut done_steps = 0;
    // A scenario starts once the screen is reached: the login at once, a film page after it opened.
    let ready_after = if to_film || to_series_page || list.is_some() || to_collection {
        3
    } else {
        1
    };
    let mut ready_at: Option<f64> = None;
    // The pointer stays still in this scenario, so that the button fades by itself.
    let still = scenario.as_deref() == Some("skip");
    let mut acts = scenario.map(|name| Acts::new(&name));
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let elapsed = begun.elapsed().as_secs_f64();
            let quit = handle.update(cx, |_, window, cx| {
                if let Some(acts) = &mut acts
                    && done_steps >= ready_after
                {
                    let since = elapsed - *ready_at.get_or_insert(elapsed);
                    acts.run(since, &shell, window, cx);
                }
                shell
                    .update(cx, |shell, cx| {
                        let step = match done_steps {
                            0 if elapsed >= 0.5 => {
                                if let Some(form) = &shell.login {
                                    let (address, user, password) = (
                                        form.address.clone(),
                                        form.user.clone(),
                                        form.password.clone(),
                                    );
                                    address.update(cx, |f, cx| {
                                        f.set_text("https://media.example.org", cx)
                                    });
                                    user.update(cx, |f, cx| f.set_text("camille", cx));
                                    password.update(cx, |f, cx| f.set_text("invented", cx));
                                }
                                if to_library {
                                    shell.select(NavEntry::Films, cx);
                                }
                                if to_series {
                                    shell.select(NavEntry::Series, cx);
                                }
                                if to_collections {
                                    shell.select(NavEntry::Collections, cx);
                                }
                                if to_list {
                                    shell.select(NavEntry::MyList, cx);
                                }
                                true
                            }
                            1 if elapsed >= 1.5 => {
                                if to_film {
                                    shell.open_film(ItemId(FILM.into()), cx);
                                }
                                if to_series_page {
                                    shell.open_series(ItemId(SERIES.into()), cx);
                                }
                                if let Some((kind, library)) = list.clone() {
                                    shell.open_list(kind, library, cx);
                                }
                                if to_collection {
                                    // Three films, one watched and one started.
                                    shell.open_collection(ItemId("collection-1".into()), cx);
                                }
                                true
                            }
                            2 if elapsed >= 3. => {
                                if scroll && let Some(page) = shell.details.last() {
                                    let bottom = page.scroll.max_offset().y;
                                    page.scroll.set_offset(point(px(0.), -bottom));
                                }
                                if to_cast && let Some(page) = shell.details.last() {
                                    page.scroll.set_offset(point(px(0.), px(-420.)));
                                }
                                if press_list {
                                    shell.toggle_favourite(cx);
                                }
                                if play {
                                    // The local file is shorter than the fake film's resume point.
                                    shell.play_film(From::Beginning, window, cx);
                                }
                                if home_rows {
                                    let bottom = shell.home.scroll.max_offset().y;
                                    shell.home.scroll.set_offset(point(px(0.), -bottom));
                                }
                                if series_rows && let Some(page) = shell.series_pages.last() {
                                    let bottom = page.scroll.max_offset().y;
                                    page.scroll.set_offset(point(px(0.), -bottom));
                                }
                                // An episode never started: it plays to the end of the file.
                                let fifth = shell.series_pages.last().and_then(|page| {
                                    match page.shown_episodes()? {
                                        crate::shell::Loadable::Ready(episodes) => {
                                            episodes.iter().find(|e| e.number == Some(5)).cloned()
                                        }
                                        _ => None,
                                    }
                                });
                                if up_next && let Some(episode) = fifth {
                                    shell.play_episode(episode, window, cx);
                                }
                                true
                            }
                            3.. if play && !still && elapsed >= 4. => {
                                shell.player_mouse_moved(cx);
                                false
                            }
                            _ => false,
                        };
                        if step {
                            done_steps += 1;
                            cx.notify();
                        }
                    })
                    .ok();
                if elapsed < quit_at {
                    return false;
                }
                if let Some(acts) = &acts {
                    shell
                        .update(cx, |shell, _| {
                            acts.report(&shell.frames.borrow());
                            acts.report_draws(&shell.draws.borrow());
                        })
                        .ok();
                }
                shell.update(cx, |shell, _| shell.close_video()).ok();
                let images = fake.state.lock().unwrap().record.images.clone();
                let persons = images
                    .iter()
                    .filter(|(image, _)| image.item.0.starts_with("person-"))
                    .count();
                println!("IMAGES asked={} persons={persons}", images.len());
                let searches = fake.state.lock().unwrap().record.searches.clone();
                println!("SEARCHES asked={searches:?}");
                println!("TILES asked={:?}", fake.state.lock().unwrap().record.tiles);
                let record = &fake.state.lock().unwrap().record;
                println!(
                    "PLAYS asked={:?} reports={:?}",
                    record
                        .plays
                        .iter()
                        .map(|(film, _)| film.0.as_str())
                        .collect::<Vec<_>>(),
                    record
                        .reports
                        .iter()
                        .map(|report| match report {
                            crate::fake::Report::Started(r) => format!("started:{}", r.film.0),
                            crate::fake::Report::Progress(r) => format!("progress:{}", r.film.0),
                            crate::fake::Report::Stopped(r) => {
                                format!("stopped:{}@{}", r.film.0, r.position.as_secs())
                            }
                        })
                        .collect::<Vec<_>>()
                );
                let methods: Vec<String> = record
                    .reports
                    .iter()
                    .map(|report| match report {
                        crate::fake::Report::Started(r)
                        | crate::fake::Report::Progress(r)
                        | crate::fake::Report::Stopped(r) => format!("{:?}", r.method),
                    })
                    .collect();
                println!(
                    "CONVERSIONS asked_from={:?} report_methods={methods:?}",
                    record
                        .transcodes
                        .iter()
                        .map(|t| t.start.as_secs())
                        .collect::<Vec<_>>(),
                );
                cx.quit();
                true
            });
            if !matches!(quit, Ok(false)) {
                break;
            }
        }
    })
    .detach();
}

/// Where a scenario points the mouse.
#[derive(Clone, Copy, Debug)]
enum Target {
    Chip(Chip),
    /// The middle of the cast row.
    CastRow,
    /// The empty right of the film page's hero.
    Blank,
    /// The left of the player's picture, clear of its track panel.
    Picture,
    /// The player's "Audio et sous-titres" button, top right.
    TracksButton,
    /// The top of the window, right of the back link: the page, never a row.
    Top,
    /// The bottom of the window, right: under the Home's header.
    Bottom,
    /// The login's address field, this many pixels into its text.
    Address(f32),
    /// The Home's header, away from its buttons.
    Hero,
    /// The mark under the Home's header for this title.
    HeroMark(usize),
    /// The Home header's "Play" button.
    HeroPlay,
    /// The Home's row of this index, in its middle.
    HomeRow(usize),
    /// "See all" on the row of this index.
    SeeAll(usize),
    /// The card of this index in the row of that index, of this width: its middle.
    Card(usize, usize, f32),
    /// A chip above the library grid.
    Tool(Tool),
    /// The "Non vus" chip.
    Unwatched,
    /// A line of the open tool menu, from the first.
    MenuLine(Tool, usize),
    /// A letter of the alphabet bar.
    Letter(char),
    /// The empty top left, over no chip and no poster.
    Corner,
    /// The account button of the pill bar.
    Account,
    /// "Settings" in the account menu.
    SettingsLine,
    /// "Sign out" in the account menu.
    SignOut,
    /// The settings' default quality button.
    QualityButton,
    /// A line of its menu, from the first.
    QualityChoice(usize),
    /// The player's seek bar, this far along it (0 to 1).
    SeekBar(f32),
    /// The player's gear, bottom right, left of the fullscreen button.
    Gear,
    /// A point of the window, in pixels from its top left.
    At(f32, f32),
}

#[derive(Clone, Debug)]
enum Act {
    Click(Target),
    /// The left button pressed on one target, dragged to the other, released there.
    Drag(Target, Target),
    Key(&'static str),
    /// A mouse wheel turned, in lines; with Shift held when `shift`.
    Wheel {
        over: Target,
        lines: f32,
        shift: bool,
    },
    /// Text typed into the search palette's field.
    Type(&'static str),
    /// The pointer moved to a target, no button.
    Hover(Target),
    /// Nothing is done: the frames that follow are counted under this name.
    Mark,
    /// The "Black out other screens" switch pressed, as its click does.
    Blackout,
    /// The player's back arrow pressed, as its click does.
    Leave,
    /// A field for a secret added to the Integrations section, as the first integration will.
    DemoSecret,
    /// Text typed in that field.
    TypeSecret(&'static str),
    /// Its Save button pressed, as its click does.
    SaveSecret,
    /// Its Clear button pressed, as its click does.
    ClearSecret,
    /// A page of the player's settings menu opened, as its line's click does.
    Settings(crate::player::settings::Page),
    /// A quality chosen in the settings menu, as its line's click does.
    Quality(crate::player::settings::Quality),
    /// A subtitle track chosen, as its line in the panel does.
    Subtitle(i64),
    /// A touchpad gesture: its moves in pixels, from start to end.
    Swipe {
        over: Target,
        moves: Vec<(f32, f32)>,
    },
}

/// A scenario: what is done when (seconds after the screen is reached), and when it was done.
struct Acts {
    steps: Vec<(f64, &'static str, Act)>,
    next: usize,
    done: Vec<(&'static str, Instant)>,
    /// When the harness started, to tell when each act was done.
    begun: Instant,
}

impl Acts {
    fn new(name: &str) -> Self {
        use Act::*;
        use Target::*;
        let wheel = |shift| Wheel {
            over: CastRow,
            lines: -3.,
            shift,
        };
        let swipe = |over, moves: &[(f32, f32)]| Swipe {
            over,
            moves: moves.to_vec(),
        };
        use crate::player::settings::{Page, Quality::Cap};
        let steps = match name {
            "menu" => vec![
                (0.5, "open audio", Click(Chip(crate::shell::Chip::Audio))),
                (
                    1.5,
                    "click audio again",
                    Click(Chip(crate::shell::Chip::Audio)),
                ),
                (
                    2.5,
                    "open subtitles",
                    Click(Chip(crate::shell::Chip::Subtitle)),
                ),
                (3.5, "escape", Key("escape")),
                (4.5, "open audio", Click(Chip(crate::shell::Chip::Audio))),
                (5.5, "click outside", Click(Blank)),
                (6.5, "escape goes back", Key("escape")),
            ],
            // The account menu of the pill bar: it closes by Escape and by a click outside, and
            // "Sign out" brings the login back with the address filled in.
            "account" => vec![
                (0.5, "open the menu", Click(Account)),
                (1.5, "escape closes the menu", Key("escape")),
                (2.5, "open the menu", Click(Account)),
                (3.5, "click outside", Click(Corner)),
                (4.5, "open the menu", Click(Account)),
                (5.5, "click the button again", Click(Account)),
                (6.5, "open the menu", Click(Account)),
                (7.5, "sign out", Click(SignOut)),
                (7.7, "the login waits for the server", Mark),
                (9.5, "the login is usable", Mark),
            ],
            "scroll" => vec![
                (0.5, "wheel down over the cast", wheel(false)),
                (1.0, "shift+wheel over the cast", wheel(true)),
                (
                    1.5,
                    "swipe sideways over the cast",
                    swipe(CastRow, &[(-24., 3.), (-30., 6.), (-20., 8.)]),
                ),
                (
                    2.0,
                    "swipe down drifting sideways over the cast",
                    swipe(CastRow, &[(2., -12.), (9., -3.), (9., 0.), (1., -10.)]),
                ),
                (
                    2.5,
                    "swipe sideways off the rows",
                    swipe(Top, &[(-30., 2.), (-30., 2.)]),
                ),
            ],
            "select" => vec![
                (0.2, "click into the address", Click(Address(60.))),
                (0.5, "ctrl-a", Key("ctrl-a")),
                (1.0, "", Key("end")),
                (1.0, "", Key("shift-left")),
                (1.0, "", Key("shift-left")),
                (1.0, "end then shift-left three times", Key("shift-left")),
                (1.5, "shift-home", Key("shift-home")),
                (
                    2.0,
                    "drag across the address",
                    Drag(Address(10.), Address(90.)),
                ),
            ],
            // The player shows its controls once the mouse moves, a moment after it starts.
            "panel" => vec![
                (1.5, "click the tracks button", Click(TracksButton)),
                (2.5, "click outside the panel", Click(Picture)),
                (3.5, "click the tracks button", Click(TracksButton)),
                (4.5, "click the tracks button again", Click(TracksButton)),
                (5.5, "click the tracks button", Click(TracksButton)),
                (6.5, "escape", Key("escape")),
            ],
            // The other monitors turn black in fullscreen, and back when it ends or the film
            // stops (`--screen playing`).
            "blackout" => vec![
                (1.5, "click the tracks button", Click(TracksButton)),
                (2.5, "switch Black out other screens on", Blackout),
                (3.5, "f goes fullscreen", Key("f")),
                (6.5, "fullscreen", Mark),
                (8.5, "f leaves fullscreen", Key("f")),
                (11.5, "windowed", Mark),
                (12.5, "f goes fullscreen again", Key("f")),
                (15.5, "the film stops in fullscreen", Leave),
                (18.5, "stopped", Mark),
            ],
            // The Settings screen from the account menu: the quality menu, the switch, a secret
            // field.
            "settings-screen" => vec![
                (0.5, "open the account menu", Click(Account)),
                (1.5, "open the settings", Click(SettingsLine)),
                (2.5, "settings", Mark),
                (3.0, "open the quality menu", Click(QualityButton)),
                (4.0, "quality menu", Mark),
                (4.5, "choose 4 Mbps", Click(QualityChoice(4))),
                (5.5, "chosen", Mark),
                (6.0, "switch Black out other screens on", Blackout),
                (6.5, "add a field for a secret", DemoSecret),
                (
                    7.0,
                    "type the secret",
                    TypeSecret("invented-key-0123456789"),
                ),
                (7.5, "typed", Mark),
                (8.0, "save the secret", SaveSecret),
                (9.0, "saved", Mark),
                (9.5, "clear the secret", ClearSecret),
                (10.5, "cleared", Mark),
                (11.0, "escape goes back", Key("escape")),
            ],
            // Ctrl+K, typing, the arrows, Enter on a person, Escape from the person's page.
            "search" => vec![
                (0.5, "ctrl-k", Key("ctrl-k")),
                (2.0, "type hiv", Type("hiv")),
                (4.0, "down", Key("down")),
                (4.5, "down", Key("down")),
                (5.5, "type a word nobody has", Type("zzzz")),
                (7.0, "type a name", Type("mercier")),
                (8.5, "enter opens the person", Key("enter")),
                (11.0, "escape goes back", Key("escape")),
                (12.0, "ctrl-k again", Key("ctrl-k")),
                (13.0, "escape closes the palette", Key("escape")),
            ],
            // A slow server: the first answer comes back after the query has changed.
            "search-slow" => vec![
                (0.5, "ctrl-k", Key("ctrl-k")),
                (1.5, "type hiv", Type("hiv")),
                (1.9, "type mer", Type("mer")),
                (5.0, "waiting is over", Key("left")),
            ],
            // Many results: the arrows scroll the list; Ctrl+K closes it.
            "search-keys" => vec![
                (0.5, "ctrl-k", Key("ctrl-k")),
                (1.5, "type e", Type("e")),
                (3.0, "down", Key("down")),
                (3.4, "down", Key("down")),
                (3.8, "down", Key("down")),
                (4.2, "down", Key("down")),
                (4.6, "down", Key("down")),
                (5.0, "down", Key("down")),
                (5.4, "down", Key("down")),
                (5.8, "down", Key("down")),
                (6.2, "up", Key("up")),
                (7.0, "ctrl-k closes", Key("ctrl-k")),
            ],
            // Nothing but the pointer put away: the header runs.
            "home-idle" => vec![(0.0, "", Hover(Bottom))],
            // The header changes title by itself, twice, with no input.
            "home-rotate" => vec![
                // Before any move the window takes the pointer for being at its corner, over
                // the header: it goes where it is out of the way.
                (0.0, "", Hover(Bottom)),
                (7.8, "title one shows, the change is next", Mark),
                (15.8, "title two shows, the change is next", Mark),
                (20., "title three shows", Mark),
            ],
            // The pointer over the header stops the time; away from it, it goes on.
            "home-hover" => vec![
                // Before any move the window takes the pointer for being at its corner, over
                // the header: it goes where it is out of the way.
                (0.0, "", Hover(Bottom)),
                (1.0, "the pointer comes over the header", Hover(Hero)),
                (9.0, "eight seconds later, still held", Mark),
                (9.5, "the pointer leaves the header", Hover(Bottom)),
                (10.5, "a second later, going again", Mark),
                (18.0, "the next title came", Mark),
            ],
            // A click on a mark shows that title.
            "home-jump" => vec![
                // Before any move the window takes the pointer for being at its corner, over
                // the header: it goes where it is out of the way.
                (0.0, "", Hover(Bottom)),
                (1.0, "click the fourth mark", Click(HeroMark(3))),
                (3.0, "click the first mark", Click(HeroMark(0))),
                (5.0, "click the first mark again", Click(HeroMark(0))),
            ],
            // The wheel over a row goes sideways, over the header it goes down.
            "home-scroll" => vec![
                // Before any move the window takes the pointer for being at its corner, over
                // the header: it goes where it is out of the way.
                (0.0, "", Hover(Bottom)),
                (
                    1.0,
                    "wheel down over the header",
                    Wheel {
                        over: Hero,
                        lines: -3.,
                        shift: false,
                    },
                ),
                (
                    1.5,
                    "wheel down over the header",
                    Wheel {
                        over: Hero,
                        lines: -3.,
                        shift: false,
                    },
                ),
                (
                    2.0,
                    "wheel over the first row",
                    Wheel {
                        over: HomeRow(0),
                        lines: -3.,
                        shift: false,
                    },
                ),
                (
                    2.5,
                    "shift+wheel over the first row",
                    Wheel {
                        over: HomeRow(0),
                        lines: -3.,
                        shift: true,
                    },
                ),
                (
                    3.0,
                    "swipe sideways over the first row",
                    swipe(HomeRow(0), &[(-24., 3.), (-30., 6.), (-20., 8.)]),
                ),
                (
                    3.5,
                    "swipe down over the first row",
                    swipe(HomeRow(0), &[(2., -12.), (9., -3.), (9., 0.), (1., -10.)]),
                ),
            ],
            // "See all" opens the full grid; Escape goes back to the Home.
            "home-lists" => vec![
                // Before any move the window takes the pointer for being at its corner, over
                // the header: it goes where it is out of the way.
                (0.0, "", Hover(Bottom)),
                (1.0, "click See all on the first row", Click(SeeAll(0))),
                (3.0, "escape back to the Home", Key("escape")),
                (4.0, "click See all on the second row", Click(SeeAll(1))),
                (6.0, "escape back to the Home", Key("escape")),
                (
                    7.0,
                    "click a card of the first row",
                    Click(Card(0, 0, 340.)),
                ),
                (9.0, "escape back to the Home", Key("escape")),
                (10.0, "click the same card again", Click(Card(0, 0, 340.))),
                (12.0, "escape back to the Home again", Key("escape")),
            ],
            // The pointer over the player's seek bar: the preview follows it, stays inside the
            // window at both ends, goes when the pointer leaves; a drag along the bar seeks.
            "seek-bar" => vec![
                (0.5, "", Hover(Bottom)),
                (2.0, "the pointer on the seek bar", Hover(SeekBar(0.25))),
                (4.0, "the pointer further along", Hover(SeekBar(0.5))),
                (6.0, "the pointer at the far left", Hover(SeekBar(0.0))),
                (8.0, "the pointer at the far right", Hover(SeekBar(0.995))),
                (
                    10.0,
                    "a drag along the bar",
                    Drag(SeekBar(0.2), SeekBar(0.75)),
                ),
                (12.0, "the pointer leaves the bar", Hover(Bottom)),
            ],
            // The pointer over the second poster of the Home's first row of posters, the Home
            // scrolled to its foot (`--screen home-rows`, a window of 1440 × 810): the veil and the buttons
            // come, then go when it leaves.
            "home-poster-hover" => vec![
                (0.0, "", Hover(Bottom)),
                // Once the Home is scrolled to its foot.
                (3.0, "the pointer over a poster", Hover(At(281., 600.))),
                (6.0, "the pointer leaves it", Hover(Bottom)),
            ],
            // The same over the second poster of the Films grid (`--screen library`).
            "grid-poster-hover" => vec![
                (0.0, "", Hover(Bottom)),
                (1.0, "the pointer over a poster", Hover(At(290., 300.))),
                (3.0, "the pointer leaves it", Hover(Bottom)),
            ],
            // The pointer over the second person of the cast (`--screen detail-cast`, a window of
            // 1440 × 810): their cell lights up, then goes dark when it leaves.
            "cast-hover" => vec![
                (0.0, "", Hover(Bottom)),
                (1.0, "the pointer over a person", Hover(At(232., 400.))),
                (3.0, "the pointer leaves them", Hover(Bottom)),
            ],
            // The pointer over the second genre of the series page (`--screen series-page`).
            "genre-hover" => vec![
                (0.0, "", Hover(Bottom)),
                (1.0, "the pointer over a genre", Hover(At(432., 252.))),
                (3.0, "the pointer leaves it", Hover(Bottom)),
            ],
            // The pointer over the picture of the first episode (`--screen series-page`): the
            // veil and the play button come, then go.
            "episode-hover" => vec![
                (0.0, "", Hover(Bottom)),
                (
                    1.0,
                    "the pointer over an episode's picture",
                    Hover(At(146., 675.)),
                ),
                (3.0, "the pointer leaves it", Hover(Bottom)),
            ],
            // The header's "Play" plays the first title, Escape leaves, the Home is asked anew.
            "home-play" => vec![
                // Before any move the window takes the pointer for being at its corner, over
                // the header: it goes where it is out of the way.
                (0.0, "", Hover(Bottom)),
                (1.0, "click Play on the header", Click(HeroPlay)),
                (5.0, "escape leaves the player", Key("escape")),
                (8.0, "the Home is back", Mark),
            ],
            // The tools of the Films or Series grid: a genre, unwatched only, an order, a letter,
            // a decade; the menus close by Escape and by a click outside.
            "tools" => vec![
                (0.5, "open genre", Click(Tool(crate::shell::Tool::Genre))),
                (
                    1.5,
                    "choose the third genre",
                    Click(MenuLine(crate::shell::Tool::Genre, 2)),
                ),
                (3.0, "unwatched only", Click(Unwatched)),
                (4.0, "open the order", Click(Tool(crate::shell::Tool::Sort))),
                (
                    5.0,
                    "sort by title",
                    Click(MenuLine(crate::shell::Tool::Sort, 1)),
                ),
                (6.5, "jump to F", Click(Letter('F'))),
                (8.5, "jump to Z", Click(Letter('Z'))),
                (10.0, "jump to hash", Click(Letter('#'))),
                (
                    11.5,
                    "open the year",
                    Click(Tool(crate::shell::Tool::Decade)),
                ),
                (12.5, "escape closes the menu", Key("escape")),
                (
                    13.5,
                    "open the year",
                    Click(Tool(crate::shell::Tool::Decade)),
                ),
                (14.5, "click outside", Click(Corner)),
                (
                    15.5,
                    "open the order",
                    Click(Tool(crate::shell::Tool::Sort)),
                ),
                (
                    16.5,
                    "sort by rating",
                    Click(MenuLine(crate::shell::Tool::Sort, 3)),
                ),
                (18.0, "unwatched off", Click(Unwatched)),
            ],
            // Times count from when the screen is reached; the file starts playing four to seven
            // seconds later, so each press comes in the middle of its segment, after a pointer
            // move that brings the button back.
            "skip" => vec![
                (12.8, "", Hover(Picture)),
                (13., "enter skips the intro", Key("enter")),
                (17., "enter between the segments", Key("enter")),
                (27.8, "", Hover(Picture)),
                (28., "enter skips the credits", Key("enter")),
            ],
            // The settings menu, opened by its gear: a page, the playback info, a quality cap
            // that restarts the film through the server's conversion, the info again, Escape.
            "settings" => vec![
                (1.5, "click the gear", Click(Gear)),
                (3.0, "the speed page", Settings(Page::Speed)),
                (5.0, "the playback info", Settings(Page::Info)),
                (7.0, "choose 4 Mbps", Quality(Cap(4_000_000))),
                (14.0, "click the gear again", Click(Gear)),
                (14.5, "the playback info again", Settings(Page::Info)),
                (17.0, "escape closes the menu", Key("escape")),
            ],
            // A list of forty subtitles, the thirtieth shown: the panel opens on it.
            "long-tracks" => vec![
                (1.0, "show the thirtieth subtitle", Subtitle(30)),
                (2.0, "click the tracks button", Click(TracksButton)),
            ],
            // The pointer keeps moving over the picture so the controls and the seek bar stay.
            "segment-marks" => (0..12)
                .map(|n| (1. + f64::from(n), "", Hover(Picture)))
                .chain([(13., "the seek bar", Mark)])
                .collect(),
            other => {
                eprintln!("bobine harness: unknown scenario {other}");
                std::process::exit(2);
            }
        };
        Self {
            steps,
            next: 0,
            done: Vec::new(),
            begun: Instant::now(),
        }
    }

    /// Does what is due `elapsed` seconds after the screen was reached.
    fn run(&mut self, elapsed: f64, shell: &WeakEntity<Shell>, window: &mut Window, cx: &mut App) {
        while let Some((at, label, act)) = self.steps.get(self.next).cloned() {
            if elapsed < at {
                break;
            }
            self.next += 1;
            let Some(shell) = shell.upgrade() else {
                return;
            };
            self.done.push((label, Instant::now()));
            send(&act, &shell, window, cx);
            if !label.is_empty() {
                let at = self.begun.elapsed().as_secs_f64();
                println!("{} (at {at:.2} s)", state(label, shell.read(cx), cx));
            }
        }
    }

    /// How many times a second, after the first three seconds, the shell was drawn, its header
    /// was drawn and the shell was notified.
    fn report_draws(&self, draws: &crate::shell::Draws) {
        let from = self.begun + Duration::from_secs(3);
        let span = Instant::now().saturating_duration_since(from).as_secs_f64();
        let rate = |times: &[Instant]| {
            times.iter().filter(|time| **time >= from).count() as f64 / span.max(f64::EPSILON)
        };
        println!(
            "DRAWS per second: shell={:.1} hero={:.1} shell_notified={:.1}",
            rate(&draws.shell),
            rate(&draws.hero),
            rate(&draws.notified),
        );
    }

    /// The frame times after each act: the draws of the half second that followed it, then the
    /// whole run after its first three seconds (the start is not what is measured).
    fn report(&self, frames: &[Instant]) {
        let settled: Vec<Instant> = frames
            .iter()
            .copied()
            .filter(|frame| frame.duration_since(self.begun) >= Duration::from_secs(3))
            .collect();
        let gaps: Vec<u128> = settled
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]).as_millis())
            .collect();
        println!(
            "FRAMES whole run: draws={} longest_gap_ms={} gaps_over_40ms={}",
            settled.len(),
            gaps.iter().max().copied().unwrap_or(0),
            gaps.iter().filter(|gap| **gap > 40).count(),
        );
        let slower = crate::motion::SLOWER.load(std::sync::atomic::Ordering::Relaxed);
        let span = Duration::from_millis(500) * slower;
        for (label, at) in self.done.iter().filter(|(label, _)| !label.is_empty()) {
            let after: Vec<Instant> = frames
                .iter()
                .copied()
                .filter(|frame| *frame >= *at && *frame <= *at + span)
                .collect();
            let gaps: Vec<u128> = after
                .windows(2)
                .map(|pair| pair[1].duration_since(pair[0]).as_millis())
                .collect();
            println!(
                "FRAMES {label}: draws={} first_after_ms={} longest_gap_ms={} gaps_ms={gaps:?}",
                after.len(),
                after
                    .first()
                    .map_or(-1, |f| f.duration_since(*at).as_millis() as i64),
                gaps.iter().max().copied().unwrap_or(0),
            );
        }
    }
}

/// The Home's header is this tall, as the screen draws it.
fn hero_height(size: gpui_kit::Size<Pixels>) -> Pixels {
    (size.height * 0.72).clamp(px(460.), px(760.))
}

/// Where a tool's chip was last drawn, on the grid shown.
fn tool_chip(shell: &Shell, tool: Tool) -> Option<Bounds<Pixels>> {
    match shell.navigator.current() {
        Screen::Series => shell.series.chips[tool as usize].get(),
        _ => shell.library.chips[tool as usize].get(),
    }
}

/// Where `target` is in the window now.
fn position(target: Target, shell: &Shell, window: &Window) -> Point<Pixels> {
    let size = window.viewport_size();
    let page = shell.details.last();
    let center = |bounds: Option<Bounds<Pixels>>| bounds.map(|b| b.center());
    match target {
        Target::Chip(chip) => center(page.and_then(|p| p.chips[chip as usize].get())),
        Target::CastRow => center(page.map(|p| p.rows[0].bounds())),
        Target::Tool(tool) => center(tool_chip(shell, tool)),
        // Between the year and the order.
        Target::Unwatched => tool_chip(shell, Tool::Decade)
            .zip(tool_chip(shell, Tool::Sort))
            .map(|(year, order)| point((year.right() + order.left()) / 2., year.center().y)),
        // The menu opens under its chip: a six-pixel gap, a border and a padding, then lines.
        Target::MenuLine(tool, line) => tool_chip(shell, tool).map(|chip| {
            point(
                chip.left() + px(60.),
                chip.bottom() + px(menu::GAP + 7. + menu::LINE * (line as f32 + 0.5)),
            )
        }),
        // The bar runs from 150 px down to 24 px above the bottom, its letters spread evenly.
        Target::Letter(letter) => LETTERS.iter().position(|l| *l == letter).map(|n| {
            let span = f32::from(size.height) - 150. - 24. - 16.;
            point(
                size.width - px(18.),
                px(150. + 8. + span * n as f32 / (LETTERS.len() - 1) as f32),
            )
        }),
        Target::Corner => Some(point(px(40.), px(30.))),
        Target::Account => center(shell.account_button_bounds()),
        // The menu opens under the button, its right edge on the button's: a gap, a border and
        // a padding, the header, the rule, then the line.
        Target::SettingsLine => shell.account_button_bounds().map(|button| {
            point(
                button.right() - px(100.),
                button.bottom() + px(menu::GAP + 7. + MENU_HEADER + MENU_RULE + menu::LINE / 2.),
            )
        }),
        Target::SignOut => shell.account_button_bounds().map(|button| {
            point(
                button.right() - px(100.),
                button.bottom() + px(menu::GAP + 7. + MENU_HEADER + MENU_RULE + menu::LINE * 1.5),
            )
        }),
        Target::QualityButton => center(shell.settings_page.quality_button.get()),
        Target::QualityChoice(line) => shell.settings_page.quality_button.get().map(|button| {
            point(
                button.right() - px(100.),
                button.bottom() + px(menu::GAP + 7. + menu::LINE * (line as f32 + 0.5)),
            )
        }),
        Target::Blank => Some(point(size.width * 0.75, size.height * 0.3)),
        Target::TracksButton => Some(point(size.width - px(127.), px(48.))),
        // Two buttons 36 wide, 8 apart, at the right margin, in the middle of the 52 high row.
        Target::Gear => Some(point(
            size.width - px(EDGE + 36. + 8. + 18.),
            size.height - px(EDGE + 26.),
        )),
        Target::Picture => Some(point(size.width * 0.3, size.height * 0.5)),
        Target::Top => Some(point(size.width * 0.75, px(24.))),
        Target::Bottom => Some(point(size.width * 0.9, size.height * 0.97)),
        // The bar runs between the edges, its strip 113 px up from the bottom.
        Target::SeekBar(fraction) => Some(point(
            px(EDGE) + (size.width - px(2. * EDGE)) * fraction,
            size.height - px(113.5),
        )),
        Target::Hero => Some(point(size.width * 0.7, hero_height(size) * 0.3)),
        Target::HeroMark(n) => {
            let count = shell.home.featured().len().max(1) as f32;
            let each = (f32::from(size.width) - 80. - 8. * (count - 1.)) / count;
            Some(point(
                px(40. + n as f32 * (each + 8.) + each / 2.),
                hero_height(size) - px(20.),
            ))
        }
        Target::HeroPlay => Some(point(px(220.), hero_height(size) - px(94.))),
        Target::HomeRow(row) => center(shell.home.rows.get(row).map(|r| r.bounds())),
        Target::SeeAll(row) => shell
            .home
            .rows
            .get(row)
            .map(|r| point(size.width - px(80.), r.bounds().top() - px(30.))),
        Target::Card(row, at, width) => shell.home.rows.get(row).map(|r| {
            let bounds = r.bounds();
            point(
                // Off the middle: there the hover shows the play button.
                bounds.left() + px(40. + at as f32 * (width + 16.) + 30.),
                bounds.top() + px(width * 9. / 32.),
            )
        }),
        Target::At(x, y) => Some(point(px(x), px(y))),
        // The login card is centred; its address field's text starts 152 pixels left of the
        // middle, 38 pixels above it.
        Target::Address(x) => Some(point(
            size.width / 2. - px(152. - x),
            size.height / 2. - px(38.),
        )),
    }
    .unwrap_or_default()
}

/// Sends `act` to the window as the compositor would.
fn send(act: &Act, shell: &Entity<Shell>, window: &mut Window, cx: &mut App) {
    let at = |target, cx: &App, window: &Window| position(target, shell.read(cx), window);
    let events = match act {
        Act::Blackout => {
            shell.update(cx, |shell, cx| shell.toggle_blackout(window, cx));
            Vec::new()
        }
        Act::Leave => {
            shell.update(cx, |shell, cx| shell.player_back(window, cx));
            Vec::new()
        }
        Act::DemoSecret => {
            shell.update(cx, |shell, cx| {
                shell.add_secret_field("Demo key", "demo-key", cx);
            });
            Vec::new()
        }
        Act::TypeSecret(text) => {
            let field = shell.read(cx).settings_page.secrets.last().cloned();
            if let Some(field) = field {
                field.update(cx, |field, cx| field.type_in(text, cx));
            }
            Vec::new()
        }
        Act::SaveSecret | Act::ClearSecret => {
            let field = shell.read(cx).settings_page.secrets.last().cloned();
            if let Some(field) = field {
                field.update(cx, |field, cx| match act {
                    Act::SaveSecret => field.save(cx),
                    _ => field.clear(cx),
                });
            }
            Vec::new()
        }
        Act::Settings(page) => {
            shell.update(cx, |shell, cx| {
                if let Some(playback) = &mut shell.playback {
                    playback.session.show_settings_page(*page);
                }
                cx.notify();
            });
            Vec::new()
        }
        Act::Quality(quality) => {
            shell.update(cx, |shell, cx| {
                let request = shell.playback.as_mut().and_then(|playback| {
                    playback.session.clicked_outside_panel();
                    playback.session.choose_quality(*quality)
                });
                shell.player_request(request, window, cx);
                cx.notify();
            });
            Vec::new()
        }
        Act::Subtitle(id) => {
            shell.update(cx, |shell, cx| {
                if let Some(playback) = &mut shell.playback {
                    playback.session.select_subtitle(Some(*id));
                }
                cx.notify();
            });
            Vec::new()
        }
        Act::Type(text) => {
            shell.update(cx, |shell, cx| {
                if let Some(search) = &shell.search {
                    search
                        .input
                        .update(cx, |field, cx| field.set_text(text, cx));
                }
            });
            Vec::new()
        }
        Act::Click(target) => {
            let position = at(*target, cx, window);
            vec![
                PlatformInput::MouseMove(MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
            ]
        }
        Act::Drag(from, to) => {
            let (from, to) = (at(*from, cx, window), at(*to, cx, window));
            vec![
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: from,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: to,
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: to,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
            ]
        }
        Act::Hover(target) => {
            let position = at(*target, cx, window);
            vec![PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            })]
        }
        Act::Mark => Vec::new(),
        Act::Key(key) => vec![PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse(key).expect("a key the harness knows"),
            is_held: false,
            prefer_character_input: false,
        })],
        Act::Wheel { over, lines, shift } => {
            vec![PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: at(*over, cx, window),
                delta: ScrollDelta::Lines(point(0., *lines)),
                modifiers: Modifiers {
                    shift: *shift,
                    ..Modifiers::default()
                },
                touch_phase: TouchPhase::Moved,
            })]
        }
        Act::Swipe { over, moves } => {
            let position = at(*over, cx, window);
            let last = moves.len() - 1;
            moves
                .iter()
                .enumerate()
                .map(|(n, (dx, dy))| {
                    PlatformInput::ScrollWheel(ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(*dx), px(*dy))),
                        modifiers: Modifiers::default(),
                        touch_phase: match n {
                            0 => TouchPhase::Started,
                            n if n == last => TouchPhase::Ended,
                            _ => TouchPhase::Moved,
                        },
                    })
                })
                .collect()
        }
    };
    for event in events {
        window.dispatch_event(event, cx);
    }
}

/// What the act changed, in one line.
fn state(label: &str, shell: &Shell, cx: &App) -> String {
    let screen = shell.navigator.current();
    if let Some(form) = &shell.login
        && screen == Screen::Login
    {
        return format!(
            "SELECT {label}: {:?} address={:?} busy={} error={:?}",
            form.address.read(cx).selected(),
            form.address.read(cx).text(),
            form.busy,
            form.error,
        );
    }
    if let Some(search) = &shell.search {
        return format!(
            "SEARCH {label}: open={} query={:?} hits={} selected={} screen={screen:?}",
            search.open,
            search.input.read(cx).text(),
            search.palette.hits().len(),
            search.palette.selected(),
        );
    }
    if screen == Screen::Settings {
        // What a secret is, never its value.
        let secrets: Vec<(Option<&str>, Option<&str>)> = shell
            .settings_page
            .secrets
            .iter()
            .map(|field| (field.read(cx).shown(), field.read(cx).error()))
            .collect();
        return format!(
            "SETTINGS {label}: quality={} blackout={} menu_open={} secrets={secrets:?}",
            shell.settings.default_quality.label(),
            shell.settings.black_out_other_monitors,
            shell.settings_page.menus.open().is_some(),
        );
    }
    if let Some(playback) = &shell.playback {
        return format!(
            "PANEL {label}: panel_open={} settings={:?} quality={:?} info={:?} chosen_line={} blackout={} offered={} screen={screen:?} position={:.0} skip={:?}",
            playback.session.panel_open(),
            playback
                .session
                .settings_open()
                .then(|| playback.session.settings_page()),
            playback.session.quality(),
            playback.session.playback_info(),
            playback.session.chosen_line(),
            shell.settings.black_out_other_monitors,
            shell.blackout_offered(),
            playback.session.position,
            playback
                .session
                .skip_button()
                .map(|button| button.label.text()),
        );
    }
    if screen == Screen::Home {
        let home = &shell.home;
        let now = Instant::now();
        return format!(
            "HOME {label}: title={} of {} progress={:.2} paused={} page_y={} rows_x={:?} busy={}",
            home.carousel.current(),
            home.carousel.len(),
            home.carousel.progress(now),
            home.carousel.paused(),
            f32::from(home.scroll.offset().y),
            home.rows
                .iter()
                .map(|row| f32::from(row.offset().x))
                .collect::<Vec<_>>(),
            home.busy,
        );
    }
    let grid = |tools: &crate::library_tools::Tools, menu, total, jump| {
        format!(
            "LIBRARY {label}: genre={:?} decade={:?} unwatched={} sort={:?} menu={menu:?} \
             total={total:?} jump={jump:?} screen={screen:?}",
            tools.filters.genre, tools.filters.decade, tools.filters.unwatched, tools.sort,
        )
    };
    match screen {
        Screen::Library => {
            let p = &shell.library;
            return grid(&p.tools, p.menus.open(), p.grid.total, p.jump);
        }
        Screen::Series => {
            let p = &shell.series;
            return grid(&p.tools, p.menus.open(), p.grid.total, p.jump);
        }
        _ => {}
    }
    match shell.details.last() {
        Some(page) if screen == Screen::FilmDetail => format!(
            "PAGE {label}: menu={:?} page_y={} cast_x={}",
            page.menus.open(),
            f32::from(page.scroll.offset().y),
            f32::from(page.rows[0].offset().x),
        ),
        _ => format!("PAGE {label}: screen={screen:?}"),
    }
}

/// Plays the file of `--play` and measures it.
fn play(mut args: Args, shell: &Entity<Shell>, window: &mut Window, cx: &mut App) {
    let mpv = match Libmpv::new(&[("ao", "null")]) {
        Ok(mpv) => Arc::new(mpv),
        Err(error) => {
            eprintln!("bobine harness: {error}");
            std::process::exit(1);
        }
    };
    let shell = shell.downgrade();
    let handle = window.window_handle();
    let begun = Instant::now();
    cx.spawn(async move |cx| {
        let mut start: Option<Snapshot> = None;
        let mut panel_opened = false;
        let (mut went_fullscreen, mut went_windowed, mut left) = (false, false, false);
        let mut next = args.next.take();
        let mut played = false;
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let elapsed = begun.elapsed().as_secs_f64();
            let done = handle.update(cx, |_, window, cx| {
                // The film starts once the start of the application is over: it would otherwise
                // put its first screen over the player.
                if !played {
                    if shell.update(cx, |shell, _| shell.starting).unwrap_or(true) {
                        return false;
                    }
                    played = true;
                    let started =
                        shell.update(cx, |shell, cx| play_file(&args, &mpv, shell, window, cx));
                    if let Ok(Err(error)) = started {
                        eprintln!("bobine harness: {error}");
                        std::process::exit(1);
                    }
                }
                shell
                    .update(cx, |shell, cx| {
                        if args.controls {
                            shell.player_mouse_moved(cx);
                        }
                        if args.panel
                            && !panel_opened
                            && let Some(playback) = &mut shell.playback
                        {
                            playback.session.toggle_panel();
                            panel_opened = true;
                            cx.notify();
                        }
                    })
                    .ok();
                if args.fullscreen_at.is_some_and(|at| elapsed >= at) && !went_fullscreen {
                    went_fullscreen = true;
                    if !window.is_fullscreen() {
                        window.toggle_fullscreen();
                    }
                }
                if args.windowed_at.is_some_and(|at| elapsed >= at) && !went_windowed {
                    went_windowed = true;
                    if window.is_fullscreen() {
                        window.toggle_fullscreen();
                    }
                }
                if let Some((file, _)) = next.take_if(|(_, at)| elapsed >= *at) {
                    // Back from the first film, then the second on a new mpv, as the film page
                    // would.
                    shell
                        .update(cx, |shell, cx| {
                            shell.player_back(window, cx);
                            let media = Media {
                                stream: Stream {
                                    url: &file,
                                    headers: &[],
                                },
                                start: 0.,
                                audio: None,
                                subtitle: FirstSubtitle::Default,
                                external_subtitles: &[],
                            };
                            let film = Film {
                                title: "Second film".to_string(),
                                subtitle: None,
                                list: None,
                            };
                            let played = Libmpv::new(&[("ao", "null")])
                                .map_err(|e| e.to_string())
                                .and_then(|mpv| {
                                    shell.play(Arc::new(mpv), media, film, None, window, cx)
                                });
                            if let Err(error) = played {
                                eprintln!("bobine harness: the second film: {error}");
                            }
                        })
                        .ok();
                }
                if args.leave_at.is_some_and(|at| elapsed >= at) && !left {
                    // The back arrow: mpv and the video surface go, the window stays.
                    left = true;
                    shell
                        .update(cx, |shell, cx| shell.player_back(window, cx))
                        .ok();
                }
                if start.is_none() && elapsed >= args.measure_from {
                    start = Some(Snapshot::take(&mpv));
                }
                if elapsed < args.quit_at {
                    return false;
                }
                if let Some(start) = &start {
                    report(&mpv, start, window);
                }
                shell.update(cx, |shell, _| shell.close_video()).ok();
                cx.quit();
                true
            });
            if !matches!(done, Ok(false)) {
                break;
            }
        }
    })
    .detach();
}

/// The file of `--play`, with its subtitles, from the start.
fn play_file(
    args: &Args,
    mpv: &Arc<Libmpv>,
    shell: &mut Shell,
    window: &mut Window,
    cx: &mut Context<Shell>,
) -> Result<(), String> {
    let film = Film {
        title: args
            .title
            .clone()
            .unwrap_or_else(|| "Test film".to_string()),
        subtitle: Some("local file · direct play".to_string()),
        list: None,
    };
    let external = args.sub.as_deref().map(|url| ExternalSubtitle {
        url,
        title: "External subtitles",
        language: None,
    });
    let media = Media {
        stream: Stream {
            url: &args.file,
            headers: &[],
        },
        start: 0.,
        audio: None,
        subtitle: match (args.subtitle_track, &external) {
            (Some(id), _) => FirstSubtitle::Track(id),
            (None, Some(_)) => FirstSubtitle::External(0),
            (None, None) => FirstSubtitle::Default,
        },
        external_subtitles: external.as_slice(),
    };
    shell.play(mpv.clone(), media, film, None, window, cx)
}

fn report(mpv: &Libmpv, start: &Snapshot, window: &Window) {
    let end = Snapshot::take(mpv);
    let seconds = (end.at - start.at).as_secs_f64();
    let get = |name: &str| mpv.get(name).unwrap_or_else(|| "-".into());
    let size = window.viewport_size();
    println!(
        "RESULT seconds={seconds:.1} cpu_percent={cpu:.1} frame_drop={dropped} \
         vo_delayed={delayed} hwdec={hwdec} video={width}x{height}@{fps} \
         window={window_width}x{window_height} scale={scale} fullscreen={fullscreen} \
         sub_codec={sub_codec}",
        cpu = cpu_percent(end.cpu - start.cpu, seconds, USER_HZ),
        dropped = end.dropped - start.dropped,
        delayed = end.delayed - start.delayed,
        hwdec = get("hwdec-current"),
        width = get("width"),
        height = get("height"),
        fps = get("container-fps"),
        window_width = f32::from(size.width),
        window_height = f32::from(size.height),
        scale = window.scale_factor(),
        fullscreen = window.is_fullscreen(),
        sub_codec = get("current-tracks/sub/codec"),
    );
}

/// User plus system CPU time of this process, in clock ticks, from a `/proc/<pid>/stat` line.
fn cpu_ticks(stat: &str) -> Option<u64> {
    // The command name is in parentheses and may hold anything: the fields are read from the
    // last `)`. utime and stime are fields 14 and 15 (proc(5)).
    let after_name = &stat[stat.rfind(')')? + 1..];
    let mut fields = after_name.split_whitespace().skip(11);
    let user: u64 = fields.next()?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(user + system)
}

/// CPU load over an interval, in percent of one core.
fn cpu_percent(ticks: u64, seconds: f64, ticks_per_second: u64) -> f64 {
    ticks as f64 / ticks_per_second as f64 / seconds * 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_of_a_media_shorter_than_the_credits_are_made_all_the_same() {
        let segments = segments_of(10.);
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[2].start, Duration::ZERO);
    }

    #[test]
    fn cpu_ticks_adds_user_and_system_time() {
        let stat = "4242 (bobine) S 1 4242 4242 0 -1 4194560 1 0 0 0 150 37 0 0 20 0 9 0";
        assert_eq!(cpu_ticks(stat), Some(187));
    }

    #[test]
    fn cpu_percent_is_relative_to_one_core() {
        assert_eq!(cpu_percent(200, 2.0, 100), 100.0);
    }

    #[test]
    fn without_play_the_application_starts_as_usual() {
        assert!(parse(std::iter::empty()).unwrap().is_none());
    }
}
