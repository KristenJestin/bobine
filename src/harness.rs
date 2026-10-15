//! A startup path for the headless checks, built only with the `harness` feature: it plays a
//! local file at once and reaches, without input, the states a nested compositor with no seat
//! cannot reach by hand (controls shown, panel open, fullscreen), then prints one `RESULT` line
//! with the numbers the pull request reports. Never part of the application a user runs.
//!
//! `bobine --play FILE [--title T] [--sub URL] [--subtitle-track ID] [--controls] [--panel]
//! [--fullscreen-at S] [--windowed-at S] [--next FILE --next-at S] [--leave-at S]
//! [--measure-from S] --quit-at S`

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::{App, Entity, Window};

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
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut parsed = Args {
        measure_from: 3.,
        quit_at: f64::INFINITY,
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
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(playing.then_some(parsed))
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

/// Starts the harness when the command line asks for it; else the application starts as usual.
pub fn start(shell: &Entity<Shell>, window: &mut Window, cx: &mut App) {
    let mut args = match parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => return,
        Err(error) => {
            eprintln!("bobine harness: {error}");
            std::process::exit(2);
        }
    };
    let mpv = match Libmpv::new(&[("ao", "null")]) {
        Ok(mpv) => Arc::new(mpv),
        Err(error) => {
            eprintln!("bobine harness: {error}");
            std::process::exit(1);
        }
    };
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
    let started = shell.update(cx, |shell, cx| {
        shell.play(mpv.clone(), media, film, window, cx)
    });
    if let Err(error) = started {
        eprintln!("bobine harness: {error}");
        std::process::exit(1);
    }

    let shell = shell.downgrade();
    let handle = window.window_handle();
    let begun = Instant::now();
    cx.spawn(async move |cx| {
        let mut start: Option<Snapshot> = None;
        let mut panel_opened = false;
        let (mut went_fullscreen, mut went_windowed, mut left) = (false, false, false);
        let mut next = args.next.take();
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let elapsed = begun.elapsed().as_secs_f64();
            let done = handle.update(cx, |_, window, cx| {
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
                                .and_then(|mpv| shell.play(Arc::new(mpv), media, film, window, cx));
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
