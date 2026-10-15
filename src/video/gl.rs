//! The render thread: an EGL context on the video subsurface, and mpv's OpenGL render API drawing
//! each new frame into it. It owns everything GL, so everything GL is freed on that thread, in
//! order: mpv's render context, then the EGL surface and context.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use khronos_egl as egl;
use libmpv2_sys as sys;
use wayland_backend::client::ObjectId;

use crate::player::libmpv::{Libmpv, check};

const EGL_PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

/// What the window asks of the render thread: the buffer size, and whether to stop. mpv's
/// update callback wakes it too.
#[derive(Default)]
pub struct Target {
    size: Mutex<(i32, i32)>,
    stop: AtomicBool,
    pending: Mutex<bool>,
    signal: Condvar,
}

impl Target {
    pub fn set_size(&self, width: i32, height: i32) {
        let mut size = self.size.lock().unwrap();
        if *size != (width, height) {
            *size = (width, height);
            drop(size);
            self.wake();
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake();
    }

    fn wake(&self) {
        *self.pending.lock().unwrap() = true;
        self.signal.notify_one();
    }

    fn wait(&self, timeout: Duration) {
        let pending = self.pending.lock().unwrap();
        let (mut pending, _) = self
            .signal
            .wait_timeout_while(pending, timeout, |pending| !*pending)
            .unwrap();
        *pending = false;
    }
}

/// A raw pointer handed to the render thread: GPUI's `wl_display`, which outlives the player.
pub struct SendPtr(pub *mut c_void);
// SAFETY: libwayland's display is thread-safe; the render thread only gives it to EGL and mpv.
unsafe impl Send for SendPtr {}

/// Runs on the render thread until `target` says stop. `ready` gets the outcome of the setup.
pub fn run(
    display: SendPtr,
    surface: ObjectId,
    mpv: Arc<Libmpv>,
    target: Arc<Target>,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let gl = match Gl::new(display.0, surface, &mpv, target.clone()) {
        Ok(gl) => gl,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    if let Err(error) = gl.run(&target) {
        eprintln!("bobine: the video stopped drawing: {error}");
    }
}

type Egl = egl::DynamicInstance<egl::EGL1_5>;

struct Gl {
    /// Freed first, while its GL context still exists.
    render: Option<RenderContext>,
    window: wayland_egl::WlEglSurface,
    egl: Box<Egl>,
    display: egl::Display,
    surface: egl::Surface,
    context: egl::Context,
}

impl Gl {
    fn new(
        wl_display: *mut c_void,
        wl_surface: ObjectId,
        mpv: &Libmpv,
        target: Arc<Target>,
    ) -> Result<Gl, String> {
        // SAFETY: loads libEGL; its symbols are only used through `egl`.
        let egl =
            Box::new(unsafe { Egl::load_required() }.map_err(|e| format!("loading libEGL: {e}"))?);
        // SAFETY: `wl_display` is GPUI's live display.
        let display = unsafe {
            egl.get_platform_display(EGL_PLATFORM_WAYLAND_KHR, wl_display, &[egl::ATTRIB_NONE])
        }
        .map_err(|e| format!("eglGetPlatformDisplay: {e}"))?;
        egl.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_API)
            .map_err(|e| format!("eglBindAPI: {e}"))?;
        let config = egl
            .choose_first_config(
                display,
                &[
                    egl::SURFACE_TYPE,
                    egl::WINDOW_BIT,
                    egl::RENDERABLE_TYPE,
                    egl::OPENGL_BIT,
                    egl::RED_SIZE,
                    8,
                    egl::GREEN_SIZE,
                    8,
                    egl::BLUE_SIZE,
                    8,
                    egl::NONE,
                ],
            )
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or("no EGL configuration for an OpenGL window")?;
        let context = egl
            .create_context(display, config, None, &[egl::NONE])
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        let window = wayland_egl::WlEglSurface::new(wl_surface, 1, 1)
            .map_err(|e| format!("wl_egl_window: {e}"))?;
        // SAFETY: `window` is a live wl_egl_window, kept in `Gl` past the EGL surface.
        let surface = unsafe {
            egl.create_window_surface(display, config, window.ptr() as egl::NativeWindowType, None)
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;
        // Never paced by the compositor's frame callbacks: a hidden window (another workspace)
        // gets none, and a swap waiting for one would never return, nor the thread, nor the app
        // that waits for it to leave the player. mpv paces the frames: its update callback says
        // when one is due, and rendering waits for the frame's time.
        egl.swap_interval(display, 0)
            .map_err(|e| format!("eglSwapInterval: {e}"))?;

        let init = sys::mpv_opengl_init_params {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: &*egl as *const Egl as *mut c_void,
        };
        let mut params = [
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                c"opengl".as_ptr().cast(),
            ),
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                (&init as *const sys::mpv_opengl_init_params).cast(),
            ),
            // Lets mpv share decoded frames with GL through the display.
            param(
                sys::mpv_render_param_type_MPV_RENDER_PARAM_WL_DISPLAY,
                wl_display,
            ),
            end(),
        ];
        let render = RenderContext::new(mpv, &mut params, target)?;
        Ok(Gl {
            render: Some(render),
            window,
            egl,
            display,
            surface,
            context,
        })
    }

    fn run(&self, target: &Target) -> Result<(), String> {
        let render = self
            .render
            .as_ref()
            .expect("the render context lives until drop");
        let mut size = (0, 0);
        while !target.stop.load(Ordering::Relaxed) {
            target.wait(Duration::from_millis(100));
            let wanted = *target.size.lock().unwrap();
            let resized = wanted != size;
            if resized {
                self.window.resize(wanted.0, wanted.1, 0, 0);
                size = wanted;
            }
            // A new size is drawn at once, even on a paused film: else the old buffer would be
            // stretched.
            if (!render.has_new_frame() && !resized) || size.0 == 0 {
                continue;
            }
            let fbo = sys::mpv_opengl_fbo {
                fbo: 0,
                w: size.0,
                h: size.1,
                internal_format: 0,
            };
            let flip_y: c_int = 1;
            render.render(&mut [
                param(
                    sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                    (&fbo as *const sys::mpv_opengl_fbo).cast(),
                ),
                param(
                    sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                    (&flip_y as *const c_int).cast(),
                ),
                end(),
            ])?;
            self.egl
                .swap_buffers(self.display, self.surface)
                .map_err(|e| format!("eglSwapBuffers: {e}"))?;
            render.report_swap();
        }
        Ok(())
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        drop(self.render.take());
        let _ = self.egl.make_current(self.display, None, None, None);
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
    }
}

struct RenderContext {
    context: *mut sys::mpv_render_context,
    target: *const Target,
}

impl RenderContext {
    /// `params` ends with `MPV_RENDER_PARAM_INVALID`.
    fn new(
        mpv: &Libmpv,
        params: &mut [sys::mpv_render_param],
        target: Arc<Target>,
    ) -> Result<RenderContext, String> {
        let mut context = ptr::null_mut();
        // SAFETY: a live handle and a terminated parameter list.
        check(unsafe {
            sys::mpv_render_context_create(&mut context, mpv.handle(), params.as_mut_ptr())
        })
        .map_err(|e| format!("mpv_render_context_create: {e}"))?;
        // The callback runs on an mpv thread until the context is freed: it keeps one reference.
        let target = Arc::into_raw(target);
        // SAFETY: `target` stays alive until `drop` frees the context, then the reference.
        unsafe {
            sys::mpv_render_context_set_update_callback(
                context,
                Some(on_update),
                target as *mut c_void,
            );
        }
        Ok(RenderContext { context, target })
    }

    fn has_new_frame(&self) -> bool {
        // SAFETY: a live context, on its render thread.
        let flags = unsafe { sys::mpv_render_context_update(self.context) };
        flags & sys::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME as u64 != 0
    }

    fn render(&self, params: &mut [sys::mpv_render_param]) -> Result<(), String> {
        // SAFETY: a live context, its GL context current, a terminated parameter list.
        check(unsafe { sys::mpv_render_context_render(self.context, params.as_mut_ptr()) })
            .map_err(|e| format!("mpv_render_context_render: {e}"))
    }

    fn report_swap(&self) {
        // SAFETY: a live context.
        unsafe { sys::mpv_render_context_report_swap(self.context) };
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        // SAFETY: freed once, with its GL context current; no callback runs after the free.
        unsafe {
            sys::mpv_render_context_free(self.context);
            drop(Arc::from_raw(self.target));
        }
    }
}

unsafe extern "C" fn on_update(target: *mut c_void) {
    // SAFETY: the pointer given in `RenderContext::new`, alive until the context is freed.
    unsafe { &*(target as *const Target) }.wake();
}

unsafe extern "C" fn get_proc_address(egl: *mut c_void, name: *const c_char) -> *mut c_void {
    // SAFETY: the `Egl` given in `Gl::new`, alive as long as the render context.
    let egl = unsafe { &*(egl as *const Egl) };
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return ptr::null_mut();
    };
    egl.get_proc_address(name)
        .map_or(ptr::null_mut(), |f| f as *mut c_void)
}

fn param(type_: sys::mpv_render_param_type, data: *const c_void) -> sys::mpv_render_param {
    sys::mpv_render_param {
        type_,
        data: data as *mut c_void,
    }
}

fn end() -> sys::mpv_render_param {
    param(
        sys::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
        ptr::null(),
    )
}
