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
//! `bobine --screen login|home|library|detail|detail-rows|detail-cast|playing [--films N]
//! [--media FILE] [--image-delay-ms N] [--photos-fail-once] --quit-at S`

use std::sync::Arc;
use std::time::{Duration, Instant};

use bobine::provider::{Connector, ItemId};
use gpui_kit::{App, Context, Entity, Window, point, px};

use crate::fake::{FILM, FakeServer};
use crate::film_page::From;
use crate::navigation::NavEntry;

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
    /// How long each image of the fake server takes, in milliseconds.
    image_delay: u64,
    /// The fake server fails the first time it is asked each person's photo.
    photos_fail_once: bool,
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
            "--photos-fail-once" => parsed.photos_fail_once = true,
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
            }
            state.image_delay = Duration::from_millis(args.image_delay);
            state.photos_fail_once = args.photos_fail_once;
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
                show_screen(screen, args.quit_at, fake, shell, window, cx)
            }
            None => play(args, shell, window, cx),
        }
    }
}

/// Reaches `screen` as a user would, by the shell's own steps, then waits to be photographed.
fn show_screen(
    screen: String,
    quit_at: f64,
    fake: FakeServer,
    shell: &Entity<Shell>,
    window: &mut Window,
    cx: &mut App,
) {
    let shell = shell.downgrade();
    let handle = window.window_handle();
    let begun = Instant::now();
    let wants = |names: &[&str]| names.contains(&screen.as_str());
    let to_library = wants(&["library", "detail", "detail-rows", "detail-cast", "playing"]);
    let to_film = wants(&["detail", "detail-rows", "detail-cast", "playing"]);
    let (scroll, play) = (wants(&["detail-rows"]), wants(&["playing"]));
    let to_cast = wants(&["detail-cast"]);
    let mut done_steps = 0;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let elapsed = begun.elapsed().as_secs_f64();
            let quit = handle.update(cx, |_, window, cx| {
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
                                        f.set_text("https://media.exemple.org", cx)
                                    });
                                    user.update(cx, |f, cx| f.set_text("camille", cx));
                                    password.update(cx, |f, cx| f.set_text("invented", cx));
                                }
                                if to_library {
                                    shell.select(NavEntry::Films, cx);
                                }
                                true
                            }
                            1 if elapsed >= 1.5 => {
                                if to_film {
                                    shell.open_film(ItemId(FILM.into()), cx);
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
                                if play {
                                    // The local file is shorter than the fake film's resume point.
                                    shell.play_film(From::Beginning, window, cx);
                                }
                                true
                            }
                            3.. if play && elapsed >= 4. => {
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
                shell.update(cx, |shell, _| shell.close_video()).ok();
                let images = fake.state.lock().unwrap().record.images.clone();
                let persons = images
                    .iter()
                    .filter(|(image, _)| image.item.0.starts_with("person-"))
                    .count();
                println!("IMAGES asked={} persons={persons}", images.len());
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
            .unwrap_or_else(|| "Film de test".to_string()),
        subtitle: Some("fichier local · lecture directe".to_string()),
    };
    let external = args.sub.as_deref().map(|url| ExternalSubtitle {
        url,
        title: "Sous-titres externes",
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
