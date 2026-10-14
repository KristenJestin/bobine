//! Way 3: mpv's software render API draws each frame into memory, and the probe hands it to GPUI
//! as an image. Measured only to know what the last resort costs.

use std::ffi::c_int;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use libmpv2_sys as sys;

use crate::measure::opaque_bgra;
use crate::mpv::Mpv;
use crate::render::{Counters, RenderContext, Target, end, param};

/// A frame ready for GPUI: tightly packed, opaque BGRA.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct SoftwareVideo {
    pub target: Arc<Target>,
    thread: Option<JoinHandle<()>>,
    pub counters: Arc<Counters>,
}

impl SoftwareVideo {
    pub fn new(mpv: Arc<Mpv>, frames: UnboundedSender<Frame>) -> Result<SoftwareVideo, String> {
        let target = Arc::new(Target::default());
        let counters = Arc::new(Counters::default());
        let (ready, is_ready) = mpsc::channel();
        let thread = {
            let (target, counters) = (target.clone(), counters.clone());
            thread::Builder::new()
                .name("video-sw".into())
                .spawn(move || render_thread(mpv, target, counters, frames, ready))
                .map_err(|e| e.to_string())?
        };
        is_ready
            .recv()
            .map_err(|_| "the render thread died".to_string())??;
        Ok(SoftwareVideo {
            target,
            thread: Some(thread),
            counters,
        })
    }
}

impl Drop for SoftwareVideo {
    fn drop(&mut self) {
        self.target.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// 64 bytes, the alignment mpv asks of the target for its SIMD paths.
#[derive(Clone, Copy)]
#[repr(C, align(64))]
struct Line([u8; 64]);

fn render_thread(
    mpv: Arc<Mpv>,
    target: Arc<Target>,
    counters: Arc<Counters>,
    frames: UnboundedSender<Frame>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let mut params = [
        param(
            sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
            c"sw".as_ptr().cast(),
        ),
        end(),
    ];
    let render = match RenderContext::new(&mpv, &mut params, target.clone()) {
        Ok(render) => render,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));

    let mut size = (0, 0);
    let mut buffer: Vec<Line> = Vec::new();
    while !target.stopped() {
        target.wake.wait(Duration::from_millis(100));
        let wanted = target.size();
        let resized = wanted != size;
        size = wanted;
        if !render.has_new_frame() && !resized || size.0 == 0 {
            continue;
        }
        let (width, height) = (size.0 as usize, size.1 as usize);
        let stride = (width * 4).next_multiple_of(64);
        buffer.resize(stride * height / 64, Line([0; 64]));
        let sw_size: [c_int; 2] = [size.0, size.1];
        let result = render.render(&mut [
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                sw_size.as_ptr().cast(),
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                c"bgr0".as_ptr().cast(),
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                (&stride as *const usize).cast(),
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                buffer.as_mut_ptr().cast(),
            ),
            end(),
        ]);
        if let Err(error) = result {
            eprintln!("video-sw: {error}");
            return;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), buffer.len() * 64) };
        let frame = Frame {
            width: width as u32,
            height: height as u32,
            bgra: opaque_bgra(bytes, stride, width, height),
        };
        if frames.unbounded_send(frame).is_err() {
            return;
        }
        render.report_swap();
        counters.count_frame();
    }
}
