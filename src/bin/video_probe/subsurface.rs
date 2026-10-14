//! Way 1: mpv draws with OpenGL into a `wl_subsurface` placed under the GPUI window's surface. GPUI
//! leaves a transparent hole where the video goes, and the compositor blends the two: no frame
//! goes through the CPU, and the video moves with the window because it is part of it.
//!
//! GPUI 0.2 exposes its `wl_display` and `wl_surface` through raw-window-handle, so nothing in GPUI
//! is patched: the probe opens a second libwayland queue on GPUI's own connection, creates its
//! surface there and parents it to GPUI's.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use khronos_egl as egl;
use libmpv2_sys as sys;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::measure::Placement;
use crate::mpv::Mpv;
use crate::render::{Counters, RenderContext, Target, end, param};

const EGL_PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

pub struct VideoSubsurface {
    connection: Connection,
    queue: EventQueue<State>,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    placement: Option<Placement>,
    target: Arc<Target>,
    thread: Option<JoinHandle<()>>,
    pub counters: Arc<Counters>,
}

impl VideoSubsurface {
    /// Creates the subsurface under `window` and starts the thread that renders `mpv` into it.
    pub fn new(window: &gpui::Window, mpv: Arc<Mpv>) -> Result<VideoSubsurface, String> {
        let RawWindowHandle::Wayland(window_handle) = HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        else {
            return Err("the GPUI window is not a Wayland window".into());
        };
        let RawDisplayHandle::Wayland(display_handle) =
            window.display_handle().map_err(|e| e.to_string())?.as_raw()
        else {
            return Err("the GPUI display is not a Wayland display".into());
        };
        let display = display_handle.display.as_ptr();

        // A backend over a foreign display has its own event queue: GPUI never sees the events
        // of the objects made here, and this code never sees GPUI's.
        let connection =
            Connection::from_backend(unsafe { Backend::from_foreign_display(display.cast()) });
        let (globals, queue) = registry_queue_init::<State>(&connection)
            .map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals
            .bind(&qh, 4..=6, ())
            .map_err(|e| format!("wl_compositor: {e}"))?;
        let subcompositor: WlSubcompositor = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("wl_subcompositor: {e}"))?;
        let viewporter: WpViewporter = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("wp_viewporter: {e}"))?;

        let parent_id = unsafe {
            ObjectId::from_ptr(
                WlSurface::interface(),
                window_handle.surface.as_ptr().cast(),
            )
        }
        .map_err(|_| "GPUI's wl_surface is not a live proxy")?;
        let parent = WlSurface::from_id(&connection, parent_id)
            .map_err(|_| "GPUI's wl_surface is not a wl_surface")?;

        let surface = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&surface, &parent, &qh, ());
        let viewport = viewporter.get_viewport(&surface, &qh, ());
        subsurface.place_below(&parent);
        // Desynchronised: mpv's frames reach the screen without waiting for a GPUI commit.
        subsurface.set_desync();
        // The video takes no input: the pointer and the keys stay with the GPUI surface above.
        let region = compositor.create_region(&qh, ());
        surface.set_input_region(Some(&region));
        region.destroy();
        surface.commit();
        connection
            .flush()
            .map_err(|e| format!("Wayland flush: {e}"))?;

        let target = Arc::new(Target::default());
        let counters = Arc::new(Counters::default());
        let (ready, is_ready) = mpsc::channel();
        let thread = {
            let display = SendPtr(display);
            let surface = surface.id();
            let (target, counters) = (target.clone(), counters.clone());
            thread::Builder::new()
                .name("video-gl".into())
                .spawn(move || render_thread(display, surface, mpv, target, counters, ready))
                .map_err(|e| e.to_string())?
        };
        is_ready
            .recv()
            .map_err(|_| "the render thread died".to_string())??;

        Ok(VideoSubsurface {
            connection,
            queue,
            surface,
            subsurface,
            viewport,
            placement: None,
            target,
            thread: Some(thread),
            counters,
        })
    }

    /// Moves and sizes the video. Called while GPUI paints, so the position (which Wayland applies
    /// on the parent's next commit) lands in the same frame as the GPUI content around it.
    pub fn place(&mut self, placement: Placement) {
        if self.placement == Some(placement) {
            return;
        }
        self.placement = Some(placement);
        self.subsurface.set_position(placement.x, placement.y);
        self.viewport
            .set_destination(placement.width, placement.height);
        self.target
            .set_size(placement.buffer_width, placement.buffer_height);
        let _ = self.queue.dispatch_pending(&mut State);
        let _ = self.connection.flush();
    }
}

impl Drop for VideoSubsurface {
    fn drop(&mut self) {
        self.target.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        let _ = self.connection.flush();
    }
}

struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}

type Egl = egl::DynamicInstance<egl::EGL1_5>;

fn render_thread(
    display: SendPtr,
    surface: ObjectId,
    mpv: Arc<Mpv>,
    target: Arc<Target>,
    counters: Arc<Counters>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let gl = match Gl::new(display.0, surface, &mpv, target.clone()) {
        Ok(gl) => gl,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    if let Err(error) = gl.run(&target, &counters) {
        eprintln!("video-gl: {error}");
    }
}

/// The EGL context drawing into the subsurface, and the mpv render context that uses it.
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
        mpv: &Mpv,
        target: Arc<Target>,
    ) -> Result<Gl, String> {
        let egl =
            Box::new(unsafe { Egl::load_required() }.map_err(|e| format!("loading libEGL: {e}"))?);
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
            .ok_or("no EGL config for an OpenGL window")?;
        let context = egl
            .create_context(display, config, None, &[egl::NONE])
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        let window = wayland_egl::WlEglSurface::new(wl_surface, 1, 1)
            .map_err(|e| format!("wl_egl_window: {e}"))?;
        let surface = unsafe {
            egl.create_window_surface(display, config, window.ptr() as egl::NativeWindowType, None)
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
        egl.make_current(display, Some(surface), Some(surface), Some(context))
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;
        // Paced by the compositor's frame callbacks, like any Wayland client.
        egl.swap_interval(display, 1)
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
            // Lets mpv share decoded frames with GL through the display (VA-API on Wayland).
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

    fn run(&self, target: &Target, counters: &Counters) -> Result<(), String> {
        let render = self
            .render
            .as_ref()
            .expect("the render context lives until drop");
        let mut size = (0, 0);
        while !target.stopped() {
            target.wake.wait(Duration::from_millis(100));
            let wanted = target.size();
            let resized = wanted != size;
            if resized {
                self.window.resize(wanted.0, wanted.1, 0, 0);
                size = wanted;
            }
            if !render.has_new_frame() && !resized || size.0 == 0 {
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
            counters.count_frame();
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

unsafe extern "C" fn get_proc_address(egl: *mut c_void, name: *const c_char) -> *mut c_void {
    let egl = unsafe { &*(egl as *const Egl) };
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return ptr::null_mut();
    };
    egl.get_proc_address(name)
        .map_or(ptr::null_mut(), |f| f as *mut c_void)
}

struct State;

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut State,
        _: &WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlSubcompositor);
delegate_noop!(State: WlSubsurface);
delegate_noop!(State: WlRegion);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: ignore WlSurface);
