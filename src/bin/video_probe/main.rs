//! A probe, not the application: it opens a GPUI window, plays a local file through one of three
//! ways of getting libmpv's video into it, measures the playback and prints the numbers (issue #1).
//!
//! It runs a fixed scenario so that a headless run can be measured and captured: warm-up, a
//! measured stretch, then the video area shrinks inside the window (`PHASE layout`), then the
//! window itself is resized (`PHASE resize`), then the player is closed while the window stays
//! (`PHASE closed`), then it quits. A control bar and a caption drawn by GPUI stay over the video
//! the whole time.

mod args;
mod measure;
mod mpv;
mod render;
mod software;
mod subsurface;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, Application, Bounds, Context, Corners, IntoElement, Render, RenderImage, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, canvas, div, prelude::*, px, relative,
    rgb, rgba, size,
};
use smallvec::SmallVec;

use args::{Args, USAGE, Way};
use measure::{cpu_percent, cpu_ticks, placement};
use mpv::Mpv;
use software::{Frame, SoftwareVideo};
use subsurface::VideoSubsurface;

/// `/proc` counts CPU time in USER_HZ ticks, 100 a second on every Linux architecture.
const USER_HZ: u64 = 100;

type Image = Rc<RefCell<Option<Arc<RenderImage>>>>;

enum Video {
    Subsurface(Rc<RefCell<Option<VideoSubsurface>>>),
    Window,
    Software { video: SoftwareVideo, image: Image },
}

struct Snapshot {
    at: Instant,
    cpu: u64,
    dropped: i64,
    delayed: i64,
    drawn: u64,
    handed: u64,
    painted: u64,
}

/// The scenario's stages, each timed from the start of the previous one.
enum Stage {
    Warmup,
    Measuring,
    Layout,
    Resize,
    /// The player is gone and the window keeps drawing, as when the user leaves playback.
    Closed,
}

struct Probe {
    args: Args,
    mpv: Option<Arc<Mpv>>,
    video: Option<Video>,
    stage: Stage,
    start: Option<Snapshot>,
    stage_since: Instant,
    /// Frames the software way handed to GPUI.
    handed: u64,
    /// Frames GPUI painted with the video area in them.
    painted: Rc<Cell<u64>>,
    inset: bool,
    position: (f64, f64),
}

fn main() {
    let args = Args::parse(std::env::args().skip(1)).unwrap_or_else(|error| {
        eprintln!("{error}\n{USAGE}");
        std::process::exit(2);
    });
    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1600.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            // Way 1 needs the hole: everything but the video area is painted opaque below.
            window_background: WindowBackgroundAppearance::Transparent,
            app_id: Some("bobine-video-probe".into()),
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                Probe::new(args, window, cx).unwrap_or_else(|error| {
                    eprintln!("video_probe: {error}");
                    std::process::exit(1);
                })
            })
        });
        if let Err(error) = opened {
            eprintln!("video_probe: cannot open the window: {error}");
            std::process::exit(1);
        }
    });
}

impl Probe {
    fn new(args: Args, window: &mut Window, cx: &mut Context<Probe>) -> Result<Probe, String> {
        let mpv = Arc::new(Mpv::new()?);
        for (name, value) in [
            ("config", "no"),
            ("terminal", "yes"),
            ("msg-level", "all=warn"),
            ("input-default-bindings", "no"),
            // Never a sound on the machine's real output.
            ("ao", "null"),
            ("loop-file", "inf"),
            // The first subtitle track, embedded or external.
            ("sid", "1"),
        ] {
            mpv.set_option(name, value)?;
        }
        match args.way {
            Way::Subsurface => {
                mpv.set_option("vo", "libmpv")?;
                mpv.set_option("hwdec", "auto")?;
            }
            Way::Software => {
                mpv.set_option("vo", "libmpv")?;
                // The software renderer reads frames from memory: decoded on the GPU, copied back.
                mpv.set_option("hwdec", "auto-copy")?;
            }
            Way::Window => {
                mpv.set_option("vo", "gpu-next")?;
                mpv.set_option("hwdec", "auto")?;
                mpv.set_option("force-window", "yes")?;
                mpv.set_option("geometry", "1280x720")?;
                mpv.set_option("title", "Bobine video probe: mpv window")?;
            }
        }
        for (name, value) in &args.mpv_options {
            mpv.set_option(name, value)?;
        }
        if let Some(sub_file) = &args.sub_file {
            mpv.set_option("sub-files", &sub_file.to_string_lossy())?;
        }
        mpv.initialize()?;

        let video = match args.way {
            Way::Subsurface => Video::Subsurface(Rc::new(RefCell::new(Some(
                VideoSubsurface::new(window, mpv.clone())?,
            )))),
            Way::Window => Video::Window,
            Way::Software => {
                let (frames, received) = futures::channel::mpsc::unbounded();
                let video = SoftwareVideo::new(mpv.clone(), frames)?;
                Self::show_frames(received, window, cx);
                Video::Software {
                    video,
                    image: Image::default(),
                }
            }
        };
        mpv.command(&["loadfile", &args.file.to_string_lossy()])?;

        cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                let going = this.update_in(cx, |this, window, cx| this.tick(window, cx));
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();

        Ok(Probe {
            args,
            mpv: Some(mpv),
            video: Some(video),
            stage: Stage::Warmup,
            start: None,
            stage_since: Instant::now(),
            handed: 0,
            painted: Rc::default(),
            inset: false,
            position: (0.0, 0.0),
        })
    }

    /// Way 3: each frame mpv rendered in memory becomes a GPUI image.
    fn show_frames(mut received: UnboundedReceiver<Frame>, window: &Window, cx: &Context<Probe>) {
        cx.spawn_in(window, async move |this, cx| {
            while let Some(mut frame) = received.next().await {
                // Only the newest frame is worth uploading when the UI fell behind.
                while let Ok(newer) = received.try_recv() {
                    frame = newer;
                }
                let Some(buffer) =
                    image::RgbaImage::from_raw(frame.width, frame.height, frame.bgra)
                else {
                    continue;
                };
                let image = Arc::new(RenderImage::new(SmallVec::from_elem(
                    image::Frame::new(buffer),
                    1,
                )));
                let handed = this.update_in(cx, |this, window, cx| {
                    if let Some(Video::Software { image: slot, .. }) = &this.video
                        && let Some(old) = slot.borrow_mut().replace(image)
                    {
                        // Otherwise every frame stays in GPUI's texture atlas.
                        let _ = window.drop_image(old);
                    }
                    this.handed += 1;
                    cx.notify();
                });
                if handed.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn tick(&mut self, window: &mut Window, cx: &mut Context<Probe>) -> bool {
        let elapsed = self.stage_since.elapsed().as_secs_f64();
        cx.notify();
        let Some(mpv) = self.mpv.clone() else {
            if elapsed >= 2.0 {
                cx.quit();
                return false;
            }
            return true;
        };
        mpv.drain_events();
        let number = |name: &str| mpv.get(name).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        self.position = (number("time-pos"), number("duration"));

        match self.stage {
            Stage::Warmup if elapsed >= self.args.warmup => {
                println!("PHASE steady");
                self.start = Some(self.snapshot(&mpv));
                self.next_stage(Stage::Measuring);
            }
            Stage::Measuring if elapsed >= self.args.measure => {
                let end = self.snapshot(&mpv);
                if let Some(start) = self.start.take() {
                    self.report(&mpv, &start, &end);
                }
                println!("PHASE layout");
                self.inset = true;
                self.next_stage(Stage::Layout);
            }
            Stage::Layout if elapsed >= 2.5 => {
                println!("PHASE resize");
                window.resize(size(px(1280.), px(720.)));
                self.next_stage(Stage::Resize);
            }
            Stage::Resize if elapsed >= 2.5 => {
                drop(mpv);
                self.shutdown();
                println!("PHASE closed");
                self.next_stage(Stage::Closed);
            }
            _ => {}
        }
        true
    }

    fn next_stage(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_since = Instant::now();
    }

    fn snapshot(&self, mpv: &Mpv) -> Snapshot {
        let cpu = std::fs::read_to_string("/proc/self/stat")
            .ok()
            .and_then(|stat| cpu_ticks(&stat))
            .unwrap_or(0);
        let drawn = match &self.video {
            Some(Video::Subsurface(video)) => video
                .borrow()
                .as_ref()
                .map_or(0, |video| video.counters.frames()),
            Some(Video::Software { video, .. }) => video.counters.frames(),
            _ => 0,
        };
        Snapshot {
            at: Instant::now(),
            cpu,
            dropped: mpv.get_i64("frame-drop-count"),
            delayed: mpv.get_i64("vo-delayed-frame-count"),
            drawn,
            handed: self.handed,
            painted: self.painted.get(),
        }
    }

    fn report(&self, mpv: &Mpv, start: &Snapshot, end: &Snapshot) {
        let seconds = (end.at - start.at).as_secs_f64();
        let get = |name: &str| mpv.get(name).unwrap_or_else(|| "-".into());
        let file = self
            .args
            .file
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let drawn = end.drawn - start.drawn;
        println!(
            "RESULT way={way} file={file} seconds={seconds:.1} \
             cpu_percent={cpu:.1} frame_drop={dropped} vo_delayed={delayed} \
             frames_drawn={drawn} fps_drawn={fps:.1} frames_handed_to_gpui={handed} gpui_paints={painted} \
             hwdec={hwdec} vo={vo} video={width}x{height}@{container_fps} \
             gamma={gamma} primaries={primaries} sig_peak={peak} \
             target_gamma={target_gamma} target_primaries={target_primaries} \
             sub_codec={sub_codec} sub_external={sub_external}",
            way = self.args.way.name(),
            cpu = cpu_percent(end.cpu - start.cpu, seconds, USER_HZ),
            dropped = end.dropped - start.dropped,
            delayed = end.delayed - start.delayed,
            fps = drawn as f64 / seconds,
            handed = end.handed - start.handed,
            painted = end.painted - start.painted,
            hwdec = get("hwdec-current"),
            vo = get("current-vo"),
            width = get("width"),
            height = get("height"),
            container_fps = get("container-fps"),
            gamma = get("video-params/gamma"),
            primaries = get("video-params/primaries"),
            peak = get("video-params/sig-peak"),
            target_gamma = get("video-target-params/gamma"),
            target_primaries = get("video-target-params/primaries"),
            sub_codec = get("current-tracks/sub/codec"),
            sub_external = get("current-tracks/sub/external"),
        );
    }

    /// Stops the render threads before mpv, and mpv before the window.
    fn shutdown(&mut self) {
        match self.video.take() {
            Some(Video::Subsurface(video)) => drop(video.borrow_mut().take()),
            Some(Video::Software { video, .. }) => drop(video),
            _ => {}
        }
        self.mpv.take();
    }

    fn video_area(&self) -> gpui::AnyElement {
        match &self.video {
            Some(Video::Subsurface(video)) => {
                let (video, painted) = (video.clone(), self.painted.clone());
                canvas(
                    move |bounds, window, _| {
                        let placement = placement(
                            (bounds.origin.x.into(), bounds.origin.y.into()),
                            (bounds.size.width.into(), bounds.size.height.into()),
                            window.scale_factor(),
                        );
                        if let Some(video) = video.borrow_mut().as_mut() {
                            video.place(placement);
                        }
                    },
                    move |_, _, _, _| painted.set(painted.get() + 1),
                )
                .size_full()
                .into_any_element()
            }
            Some(Video::Software { video, image }) => {
                let (target, image) = (video.target.clone(), image.clone());
                let painted = self.painted.clone();
                canvas(
                    move |bounds, window, _| {
                        let scale = window.scale_factor();
                        let placement = placement(
                            (bounds.origin.x.into(), bounds.origin.y.into()),
                            (bounds.size.width.into(), bounds.size.height.into()),
                            scale,
                        );
                        target.set_size(placement.buffer_width, placement.buffer_height);
                    },
                    move |bounds, _, window, _| {
                        painted.set(painted.get() + 1);
                        if let Some(image) = image.borrow().clone() {
                            let _ = window.paint_image(bounds, Corners::default(), image, 0, false);
                        }
                    },
                )
                .size_full()
                .into_any_element()
            }
            other => div()
                .size_full()
                .bg(rgb(0x000000))
                .flex()
                .items_center()
                .justify_center()
                .child(match other {
                    Some(_) => "The video plays in mpv's own window.",
                    None => "The player is closed.",
                })
                .into_any_element(),
        }
    }
}

impl Way {
    fn name(self) -> &'static str {
        match self {
            Way::Subsurface => "subsurface",
            Way::Window => "window",
            Way::Software => "software",
        }
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (position, duration) = self.position;
        let progress = if duration > 0.0 {
            (position / duration) as f32
        } else {
            0.0
        };
        let file = self.args.file.to_string_lossy().into_owned();
        // No background on the root nor on the video's box: that is the hole the subsurface shows
        // through. Everything else paints itself opaque.
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_color(rgb(0xffffff))
            .child(
                div()
                    .h(px(48.))
                    .px_4()
                    .flex()
                    .items_center()
                    .bg(rgb(0x1e1e2e))
                    .child(format!(
                        "Bobine video probe: way {}, {file}",
                        self.args.way.name()
                    )),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .h_full()
                            .w(px(if self.inset { 320. } else { 0. }))
                            .bg(rgb(0x2a2a3a)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .child(self.video_area())
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(96.))
                                    .left(px(0.))
                                    .right(px(0.))
                                    .flex()
                                    .justify_center()
                                    .child(
                                        div()
                                            .px_3()
                                            .py_1()
                                            .bg(rgba(0x000000a0))
                                            .text_color(rgb(0xffe080))
                                            .child("Caption drawn by GPUI over the video"),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .bottom(px(0.))
                                    .left(px(0.))
                                    .right(px(0.))
                                    .h(px(64.))
                                    .px_4()
                                    .gap_4()
                                    .flex()
                                    .items_center()
                                    .bg(rgba(0x000000b0))
                                    .child(format!("{position:5.1} / {duration:.1} s"))
                                    .child(div().flex_1().h(px(6.)).bg(rgba(0xffffff40)).child(
                                        div().h_full().w(relative(progress)).bg(rgb(0xe05050)),
                                    )),
                            ),
                    ),
            )
    }
}
