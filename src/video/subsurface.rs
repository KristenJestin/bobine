//! The video on Wayland: a `wl_subsurface` placed under GPUI's surface, desynchronised, with an
//! empty input region, sized with `wp_viewporter`, that mpv draws into with OpenGL on its own
//! thread (`super::gl`). GPUI leaves a transparent hole where the video is and draws its
//! controls above: no frame goes through the CPU, and the video moves with the window because
//! it is part of it.
//!
//! GPUI gives its `wl_display` and `wl_surface` through raw-window-handle, so GPUI is not
//! patched: a second libwayland queue on GPUI's own connection holds the objects made here, and
//! GPUI never sees their events. The same queue hides the cursor, which GPUI cannot do on
//! request.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use gpui_kit::Window;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{
    Shape, WpCursorShapeDeviceV1,
};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use super::gl::{self, SendPtr, Target};
use super::{Placement, VideoDisplay};
use crate::player::libmpv::Libmpv;

/// What the video needs of the compositor, bound once for the window and kept with it: Wayland
/// cannot release a registry or a compositor, so binding them for each film would leave them
/// behind, on a queue nobody reads any more.
pub struct Wayland {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    /// GPUI's `wl_display`, for EGL and mpv.
    display: *mut c_void,
    /// GPUI's surface, the video's parent.
    parent: WlSurface,
    compositor: WlCompositor,
    subcompositor: WlSubcompositor,
    viewporter: WpViewporter,
    seat: Option<WlSeat>,
    cursor_shapes: Option<WpCursorShapeManagerV1>,
}

impl Wayland {
    /// Binds, on a queue of GPUI's own connection, what the video of `window` needs.
    pub fn new(window: &Window) -> Result<Wayland, String> {
        let RawWindowHandle::Wayland(window_handle) = HasWindowHandle::window_handle(window)
            .map_err(|e| e.to_string())?
            .as_raw()
        else {
            return Err("the window is not a Wayland window".into());
        };
        let RawDisplayHandle::Wayland(display_handle) =
            window.display_handle().map_err(|e| e.to_string())?.as_raw()
        else {
            return Err("the display is not a Wayland display".into());
        };
        let display = display_handle.display.as_ptr();

        // SAFETY: GPUI's display stays connected as long as its window, which outlives this.
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
        // Without them the cursor simply stays. Version 3 at least: each film's pointer is
        // released when the film goes.
        let seat: Option<WlSeat> = globals.bind(&qh, 3..=5, ()).ok();
        let cursor_shapes: Option<WpCursorShapeManagerV1> = globals.bind(&qh, 1..=1, ()).ok();

        // SAFETY: GPUI's live wl_surface proxy, of that interface.
        let parent_id = unsafe {
            ObjectId::from_ptr(
                WlSurface::interface(),
                window_handle.surface.as_ptr().cast(),
            )
        }
        .map_err(|_| "the window's wl_surface is not a live proxy")?;
        let parent = WlSurface::from_id(&connection, parent_id)
            .map_err(|_| "the window's wl_surface is not a wl_surface")?;

        Ok(Wayland {
            connection,
            queue,
            state: State::default(),
            display,
            parent,
            compositor,
            subcompositor,
            viewporter,
            seat,
            cursor_shapes,
        })
    }

    /// Handles what the compositor said to this queue: the seat's pointer, its enter serial.
    fn dispatch(&mut self) {
        let _ = self.queue.dispatch_pending(&mut self.state);
        if self.state.pointer.is_none()
            && self.state.has_pointer
            && let Some(seat) = &self.seat
        {
            let qh = self.queue.handle();
            let pointer = seat.get_pointer(&qh, ());
            self.state.shape_device = self
                .cursor_shapes
                .as_ref()
                .map(|shapes| shapes.get_pointer(&pointer, &qh, ()));
            self.state.pointer = Some(pointer);
            let _ = self.connection.flush();
        }
    }

    /// The pointer made for a film goes with it: GPUI's own pointer keeps the cursor between
    /// films.
    fn release_pointer(&mut self) {
        if let Some(device) = self.state.shape_device.take() {
            device.destroy();
        }
        if let Some(pointer) = self.state.pointer.take() {
            pointer.release();
        }
        self.state.enter_serial = None;
        let _ = self.connection.flush();
    }
}

impl Drop for Wayland {
    /// What the protocol lets go goes; the registry and the compositor stay with the connection,
    /// which forgets them when it closes.
    fn drop(&mut self) {
        self.release_pointer();
        if let Some(shapes) = self.cursor_shapes.take() {
            shapes.destroy();
        }
        if let Some(seat) = self.seat.take()
            && seat.version() >= 5
        {
            seat.release();
        }
        self.viewporter.destroy();
        self.subcompositor.destroy();
        let _ = self.connection.flush();
    }
}

pub struct VideoSurface {
    wayland: Rc<RefCell<Wayland>>,
    surfaces: Surfaces,
    placement: Option<Placement>,
    /// The cursor as last set, and the pointer entry it was set for.
    cursor: Option<(bool, u32)>,
    target: Arc<Target>,
    thread: Option<JoinHandle<()>>,
}

impl VideoSurface {
    /// Creates the subsurface under the window of `wayland` and starts the thread that draws
    /// `mpv` into it.
    pub fn new(wayland: Rc<RefCell<Wayland>>, mpv: Arc<Libmpv>) -> Result<VideoSurface, String> {
        let surfaces = {
            let wayland = wayland.borrow();
            let qh = wayland.queue.handle();
            let surface = wayland.compositor.create_surface(&qh, ());
            let subsurface =
                wayland
                    .subcompositor
                    .get_subsurface(&surface, &wayland.parent, &qh, ());
            let viewport = wayland.viewporter.get_viewport(&surface, &qh, ());
            let surfaces = Surfaces {
                connection: wayland.connection.clone(),
                surface,
                subsurface,
                viewport,
            };
            surfaces.subsurface.place_below(&wayland.parent);
            // Desynchronised: mpv's frames reach the screen without waiting for a GPUI commit.
            surfaces.subsurface.set_desync();
            // The video takes no input: the pointer and the keys stay with the window above.
            let region = wayland.compositor.create_region(&qh, ());
            surfaces.surface.set_input_region(Some(&region));
            region.destroy();
            surfaces.surface.commit();
            wayland
                .connection
                .flush()
                .map_err(|e| format!("Wayland flush: {e}"))?;
            surfaces
        };

        let target = Arc::new(Target::default());
        let (ready, is_ready) = mpsc::channel();
        let thread = {
            let display = SendPtr(wayland.borrow().display);
            let surface = surfaces.surface.id();
            let target = target.clone();
            thread::Builder::new()
                .name("video-gl".into())
                .spawn(move || gl::run(display, surface, mpv, target, ready))
                .map_err(|e| e.to_string())?
        };
        let outcome = is_ready
            .recv()
            .map_err(|_| "the render thread died".to_string())
            .flatten();
        if let Err(error) = outcome {
            // Its EGL objects go before the surface they were made on.
            let _ = thread.join();
            return Err(error);
        }

        Ok(VideoSurface {
            wayland,
            surfaces,
            placement: None,
            cursor: None,
            target,
            thread: Some(thread),
        })
    }
}

impl VideoDisplay for VideoSurface {
    /// Called while GPUI paints, so the position (applied on the window's next commit) lands in
    /// the same frame as the controls around it.
    fn place(&mut self, placement: Placement) {
        let mut wayland = self.wayland.borrow_mut();
        wayland.dispatch();
        if self.placement == Some(placement) {
            return;
        }
        self.placement = Some(placement);
        self.surfaces
            .subsurface
            .set_position(placement.x, placement.y);
        self.surfaces
            .viewport
            .set_destination(placement.width, placement.height);
        self.target
            .set_size(placement.buffer_width, placement.buffer_height);
        let _ = wayland.connection.flush();
    }

    fn picture_shown(&self) -> bool {
        self.target.shown()
    }

    fn set_cursor_visible(&mut self, visible: bool) {
        let mut wayland = self.wayland.borrow_mut();
        wayland.dispatch();
        let (Some(pointer), Some(device), Some(serial)) = (
            &wayland.state.pointer,
            &wayland.state.shape_device,
            wayland.state.enter_serial,
        ) else {
            // Hidden without a way to bring it back would be worse than shown.
            return;
        };
        // Each entry into the window lets GPUI set its own cursor again.
        if self.cursor == Some((visible, serial)) {
            return;
        }
        self.cursor = Some((visible, serial));
        if visible {
            device.set_shape(serial, Shape::Default);
        } else {
            pointer.set_cursor(serial, None, 0, 0);
        }
        let _ = wayland.connection.flush();
    }
}

impl Drop for VideoSurface {
    /// The render thread frees mpv's render context and the EGL objects before the surface they
    /// draw into goes, and all of it before the window does. What was bound for the window
    /// stays for the next film.
    fn drop(&mut self) {
        self.target.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.wayland.borrow_mut().release_pointer();
    }
}

/// The video's own surface, its place under the window and its size: destroyed when dropped,
/// so a setup that fails halfway leaves no empty subsurface under the window.
struct Surfaces {
    connection: Connection,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
}

impl Drop for Surfaces {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        let _ = self.connection.flush();
    }
}

#[derive(Default)]
struct State {
    has_pointer: bool,
    pointer: Option<WlPointer>,
    shape_device: Option<WpCursorShapeDeviceV1>,
    /// The serial of the pointer's last entry into one of the window's surfaces: the cursor can
    /// only be changed with it.
    enter_serial: Option<u32>,
}

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

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut State,
        _: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            state.has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut State,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial, .. } => state.enter_serial = Some(serial),
            wl_pointer::Event::Leave { .. } => state.enter_serial = None,
            _ => {}
        }
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlSubcompositor);
delegate_noop!(State: WlSubsurface);
delegate_noop!(State: WlRegion);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: WpCursorShapeManagerV1);
delegate_noop!(State: WpCursorShapeDeviceV1);
delegate_noop!(State: ignore WlSurface);
