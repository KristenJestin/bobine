//! mpv's render API, shared by the subsurface (OpenGL) and software ways.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use libmpv2_sys as sys;

use crate::mpv::{Mpv, check};

/// Wakes the render thread: mpv has a new frame, or the target changed size, or it must stop.
#[derive(Default)]
pub struct Wake {
    pending: Mutex<bool>,
    signal: Condvar,
}

impl Wake {
    pub fn notify(&self) {
        *self.pending.lock().unwrap() = true;
        self.signal.notify_one();
    }

    /// Waits for a notification, at most `timeout`.
    pub fn wait(&self, timeout: Duration) {
        let pending = self.pending.lock().unwrap();
        let (mut pending, _) = self
            .signal
            .wait_timeout_while(pending, timeout, |pending| !*pending)
            .unwrap();
        *pending = false;
    }
}

/// What the UI asks of the render thread: the size to draw at, and whether to stop.
#[derive(Default)]
pub struct Target {
    size: Mutex<(i32, i32)>,
    stop: AtomicBool,
    pub wake: Wake,
}

impl Target {
    pub fn set_size(&self, width: i32, height: i32) {
        let mut size = self.size.lock().unwrap();
        if *size != (width, height) {
            *size = (width, height);
            drop(size);
            self.wake.notify();
        }
    }

    pub fn size(&self) -> (i32, i32) {
        *self.size.lock().unwrap()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.notify();
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
}

/// What the render thread tells the UI.
#[derive(Default)]
pub struct Counters {
    /// Frames drawn to the target.
    pub frames: AtomicU64,
}

impl Counters {
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }

    pub fn count_frame(&self) {
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct RenderContext {
    context: *mut sys::mpv_render_context,
    target: *const Target,
}

impl RenderContext {
    /// Creates the context; `params` must end with `MPV_RENDER_PARAM_INVALID`.
    pub fn new(
        mpv: &Mpv,
        params: &mut [sys::mpv_render_param],
        target: Arc<Target>,
    ) -> Result<RenderContext, String> {
        let mut context = ptr::null_mut();
        check(
            unsafe {
                sys::mpv_render_context_create(&mut context, mpv.handle(), params.as_mut_ptr())
            },
            "mpv_render_context_create",
        )?;
        // The callback runs on an mpv thread until the context is freed: it keeps one reference.
        let target = Arc::into_raw(target);
        unsafe {
            sys::mpv_render_context_set_update_callback(
                context,
                Some(on_update),
                target as *mut c_void,
            );
        }
        Ok(RenderContext { context, target })
    }

    /// Whether mpv has a new frame to draw.
    pub fn has_new_frame(&self) -> bool {
        let flags = unsafe { sys::mpv_render_context_update(self.context) };
        flags & sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64 != 0
    }

    /// Draws the current frame; `params` must end with `MPV_RENDER_PARAM_INVALID`.
    pub fn render(&self, params: &mut [sys::mpv_render_param]) -> Result<(), String> {
        check(
            unsafe { sys::mpv_render_context_render(self.context, params.as_mut_ptr()) },
            "mpv_render_context_render",
        )
    }

    pub fn report_swap(&self) {
        unsafe { sys::mpv_render_context_report_swap(self.context) };
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            sys::mpv_render_context_free(self.context);
            drop(Arc::from_raw(self.target));
        }
    }
}

unsafe extern "C" fn on_update(target: *mut c_void) {
    unsafe { &*(target as *const Target) }.wake.notify();
}

pub fn param(type_: sys::mpv_render_param_type, data: *const c_void) -> sys::mpv_render_param {
    sys::mpv_render_param {
        type_,
        data: data as *mut c_void,
    }
}

pub fn end() -> sys::mpv_render_param {
    param(
        sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
        ptr::null(),
    )
}
