//! The black surfaces, on Wayland: one `zwlr_layer_surface_v1` per covered monitor, overlay
//! layer, anchored to all four edges, over exclusive zones, never taking the keyboard. Their
//! black is a 1×1 `wl_shm` pixel stretched by `wp_viewporter`: no GPU. They fade in and out by
//! changing that pixel's alpha, so the fade does not depend on the compositor.
//!
//! The surfaces live on a connection of their own, on their own thread, apart from GPUI's: it
//! follows the monitors plugged in or out at any time, and when Bobine quits or crashes the
//! connection closes and the compositor takes the surfaces away with it.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use uuid::Uuid;
use wayland_client::backend::WaylandError;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{
    Layer, ZwlrLayerShellV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

use super::{Blackout, Covers, Heartbeat, Wish, monitor_with_id};
use crate::motion::{self, Fade};

/// The steps of a fade: one pixel for each, from transparent to black.
const STEPS: usize = 16;
/// A frame, while something fades.
const FRAME_MS: i32 = 16;
/// The name the compositor sees the surfaces under (its layer rules can use it).
const NAMESPACE: &str = "bobine-blackout";

enum Command {
    /// The other monitors are wanted black or not, and GPUI's id of the window's monitor.
    Want(bool, Option<Uuid>),
    /// The UI thread is alive: said on every player tick while covering.
    Beat,
    Stop,
}

/// The thread that owns the black surfaces, and how to talk to it.
pub struct Screens {
    commands: Sender<Command>,
    wake: UnixStream,
    sent: Option<(bool, Option<Uuid>)>,
    thread: Option<JoinHandle<()>>,
}

impl Screens {
    /// Connects to the compositor. An error when it has no layer shell: the switch is not
    /// offered.
    pub fn connect() -> Result<Screens, String> {
        let connection = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
        let (globals, mut queue) = registry_queue_init::<State>(&connection)
            .map_err(|e| format!("Wayland registry: {e}"))?;
        let qh = queue.handle();
        let layer_shell: ZwlrLayerShellV1 = globals
            .bind(&qh, 1..=4, ())
            .map_err(|e| format!("zwlr_layer_shell_v1: {e}"))?;
        let compositor: WlCompositor = globals
            .bind(&qh, 4..=6, ())
            .map_err(|e| format!("wl_compositor: {e}"))?;
        let viewporter: WpViewporter = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("wp_viewporter: {e}"))?;
        let shm: WlShm = globals
            .bind(&qh, 1..=1, ())
            .map_err(|e| format!("wl_shm: {e}"))?;
        let pixels = black_pixels(&shm, &qh)?;
        let mut state = State {
            qh: qh.clone(),
            registry: globals.registry().clone(),
            compositor,
            layer_shell,
            viewporter,
            pixels,
            outputs: Vec::new(),
            outputs_changed: true,
            covers: HashMap::new(),
        };
        globals.contents().with_list(|list| {
            for global in list
                .iter()
                .filter(|g| g.interface == WlOutput::interface().name)
            {
                state.add_output(global.name, global.version);
            }
        });
        // The monitors' names, before the switch is offered.
        queue
            .roundtrip(&mut state)
            .map_err(|e| format!("Wayland: {e}"))?;

        let (commands, received) = mpsc::channel();
        let (wake, woken) = UnixStream::pair().map_err(|e| e.to_string())?;
        woken.set_nonblocking(true).map_err(|e| e.to_string())?;
        let thread = thread::Builder::new()
            .name("blackout".into())
            .spawn(move || run(connection, queue, state, received, woken))
            .map_err(|e| e.to_string())?;
        Ok(Screens {
            commands,
            wake,
            sent: None,
            thread: Some(thread),
        })
    }

    /// Whether the other monitors are wanted black, and GPUI's id of the window's monitor.
    /// Said on every tick: only a change reaches the thread.
    pub fn want(&mut self, wanted: bool, window_on: Option<Uuid>) {
        if self.sent == Some((wanted, window_on)) {
            return;
        }
        self.sent = Some((wanted, window_on));
        self.send(Command::Want(wanted, window_on));
    }

    /// Says the UI thread is alive. Without it for `SILENCE` the thread uncovers everything,
    /// so a frozen UI never leaves the monitors black. Only sent while covering.
    pub fn beat(&mut self) {
        if matches!(self.sent, Some((true, _))) {
            self.send(Command::Beat);
        }
    }

    fn send(&mut self, command: Command) {
        // A thread that ended (its connection lost) has nothing left on the screens.
        if self.commands.send(command).is_ok() {
            let _ = self.wake.write(&[1]);
        }
    }
}

impl Wish for Screens {
    fn want(&mut self, wanted: bool, window_on: Option<Uuid>) {
        Screens::want(self, wanted, window_on);
    }
}

impl Drop for Screens {
    /// The connection closes with the thread: whatever still covers a monitor goes at once.
    fn drop(&mut self) {
        self.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One premultiplied ARGB pixel per step of a fade, black from alpha 0 to 255, in one pool.
fn black_pixels(shm: &WlShm, qh: &QueueHandle<State>) -> Result<Vec<WlBuffer>, String> {
    let bytes: Vec<u8> = (0..=STEPS)
        .flat_map(|step| [0, 0, 0, (step * 255 / STEPS) as u8])
        .collect();
    // SAFETY: a plain call with a static name; the fd it returns is owned at once.
    let fd = unsafe { libc::memfd_create(c"bobine-blackout".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!("memfd: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: a fresh fd nothing else owns.
    let mut file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    file.write_all(&bytes).map_err(|e| format!("memfd: {e}"))?;
    let pool = shm.create_pool(file.as_fd(), bytes.len() as i32, qh, ());
    let pixels = (0..=STEPS)
        .map(|step| {
            let offset = (step * 4) as i32;
            pool.create_buffer(offset, 1, 1, 4, wl_shm::Format::Argb8888, qh, ())
        })
        .collect();
    // The buffers keep the memory: the pool and the file can go.
    pool.destroy();
    Ok(pixels)
}

fn run(
    connection: Connection,
    mut queue: EventQueue<State>,
    mut state: State,
    commands: Receiver<Command>,
    mut woken: UnixStream,
) {
    let mut blackout = Blackout::default();
    let mut wish: (bool, Option<Uuid>) = (false, None);
    let mut heartbeat = Heartbeat::new(Instant::now());
    // The UI went silent while covering: the wish is kept, but nothing is covered.
    let mut silent = false;
    loop {
        let _ = woken.read(&mut [0; 64]);
        let mut wish_changed = false;
        loop {
            match commands.try_recv() {
                Ok(Command::Want(wanted, window_on)) => {
                    heartbeat.beat(Instant::now());
                    wish = (wanted, window_on);
                    wish_changed = true;
                }
                Ok(Command::Beat) => heartbeat.beat(Instant::now()),
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        if queue.dispatch_pending(&mut state).is_err() {
            return;
        }
        let now = Instant::now();
        let was_silent = silent;
        silent = wish.0 && heartbeat.lapsed(now);
        if wish_changed || silent != was_silent || state.outputs_changed {
            state.outputs_changed = false;
            let outputs = state.output_names();
            let window_on = wish.1.and_then(|id| monitor_with_id(&outputs, id));
            blackout.follow(
                &outputs,
                wish.0 && !silent,
                window_on.as_deref(),
                &mut state,
            );
        }
        let fading = state.step(now);
        let blocked = match flushed(connection.flush()) {
            Flushed::Done => false,
            Flushed::Blocked => true,
            Flushed::Failed => return,
        };
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [
            libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                // A full socket is waited on until it drains: the rest is sent then.
                events: libc::POLLIN | if blocked { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: woken.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // While covering, wake up when the UI's silence would lapse.
        let watch = (wish.0 && !silent).then(|| {
            let wait = heartbeat.until_lapse(Instant::now());
            wait.as_millis() as i32 + 1
        });
        let timeout = match (fading, watch) {
            (true, Some(watch)) => FRAME_MS.min(watch),
            (true, None) => FRAME_MS,
            (false, Some(watch)) => watch,
            (false, None) => -1,
        };
        // SAFETY: two valid pollfds, alive for the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if ready > 0 && fds[0].revents & !libc::POLLOUT != 0 {
            if guard.read().is_err() {
                return;
            }
        } else {
            drop(guard);
        }
    }
}

/// How sending to the compositor went.
#[derive(Debug, PartialEq)]
enum Flushed {
    Done,
    /// The socket is full: try again when it can be written.
    Blocked,
    Failed,
}

fn flushed(result: Result<(), WaylandError>) -> Flushed {
    match result {
        Ok(()) => Flushed::Done,
        Err(WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => Flushed::Blocked,
        Err(_) => Flushed::Failed,
    }
}

struct Output {
    /// The registry's name of the global, to know it when it goes.
    global: u32,
    output: WlOutput,
    name: Option<String>,
    done: bool,
}

/// A black surface on one monitor.
struct Cover {
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    viewport: WpViewport,
    fade: Fade,
    /// The size the compositor gave it; nothing is drawn before.
    size: Option<(i32, i32)>,
    /// The step drawn last.
    drawn: Option<usize>,
}

impl Cover {
    fn destroy(self) {
        self.layer.destroy();
        self.viewport.destroy();
        self.surface.destroy();
    }
}

struct State {
    qh: QueueHandle<State>,
    registry: WlRegistry,
    compositor: WlCompositor,
    layer_shell: ZwlrLayerShellV1,
    viewporter: WpViewporter,
    pixels: Vec<WlBuffer>,
    outputs: Vec<Output>,
    outputs_changed: bool,
    covers: HashMap<String, Cover>,
}

impl State {
    fn add_output(&mut self, global: u32, version: u32) {
        // Version 4 tells the monitor's name, which is how GPUI and the decisions know it.
        if version < 4 {
            return;
        }
        let output = self
            .registry
            .bind::<WlOutput, _, _>(global, version.min(4), &self.qh, global);
        self.outputs.push(Output {
            global,
            output,
            name: None,
            done: false,
        });
    }

    fn output_names(&self) -> Vec<String> {
        let ready = self.outputs.iter().filter(|output| output.done);
        ready.filter_map(|output| output.name.clone()).collect()
    }

    /// Draws each cover's step of its fade; a faded-out cover goes. Whether one still fades.
    fn step(&mut self, now: Instant) -> bool {
        let gone: Vec<String> = self
            .covers
            .iter()
            .filter(|(_, cover)| !cover.fade.visible(now))
            .map(|(name, _)| name.clone())
            .collect();
        for name in gone {
            if let Some(cover) = self.covers.remove(&name) {
                cover.destroy();
            }
        }
        let mut fading = false;
        for cover in self.covers.values_mut() {
            fading |= cover.fade.moving(now);
            if cover.size.is_none() {
                continue;
            }
            let step = (cover.fade.level(now).clamp(0., 1.) * STEPS as f32).round() as usize;
            if cover.drawn == Some(step) {
                continue;
            }
            cover.drawn = Some(step);
            cover.surface.attach(Some(&self.pixels[step]), 0, 0);
            cover.surface.damage_buffer(0, 0, 1, 1);
            cover.surface.commit();
        }
        fading
    }
}

impl Covers for State {
    fn cover(&mut self, name: &str) {
        let now = Instant::now();
        if let Some(cover) = self.covers.get_mut(name) {
            // Still fading out: it fades back in from where it is.
            cover.fade.set(true, now);
            return;
        }
        let Some(output) = self
            .outputs
            .iter()
            .find(|o| o.name.as_deref() == Some(name))
        else {
            return;
        };
        let surface = self.compositor.create_surface(&self.qh, ());
        let viewport = self.viewporter.get_viewport(&surface, &self.qh, ());
        let layer = self.layer_shell.get_layer_surface(
            &surface,
            Some(&output.output),
            Layer::Overlay,
            NAMESPACE.into(),
            &self.qh,
            name.to_string(),
        );
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        // The first commit has no buffer: the compositor answers with the size.
        surface.commit();
        let mut fade = Fade::new(motion::page());
        fade.set(true, now);
        self.covers.insert(
            name.to_string(),
            Cover {
                surface,
                layer,
                viewport,
                fade,
                size: None,
                drawn: None,
            },
        );
    }

    fn uncover(&mut self, name: &str) {
        let unplugged = !self.outputs.iter().any(|o| o.name.as_deref() == Some(name));
        if unplugged {
            // Nothing left to fade on.
            if let Some(cover) = self.covers.remove(name) {
                cover.destroy();
            }
        } else if let Some(cover) = self.covers.get_mut(name) {
            cover.fade.set(false, Instant::now());
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut State,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == WlOutput::interface().name => state.add_output(name, version),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(index) = state.outputs.iter().position(|o| o.global == name) {
                    let output = state.outputs.remove(index);
                    output.output.release();
                    state.outputs_changed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(
        state: &mut State,
        _: &WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        let Some(output) = state.outputs.iter_mut().find(|o| o.global == *global) else {
            return;
        };
        match event {
            wl_output::Event::Name { name } => output.name = Some(name),
            wl_output::Event::Done => {
                output.done = true;
                state.outputs_changed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, String> for State {
    fn event(
        state: &mut State,
        layer: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        name: &String,
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                let Some(cover) = state.covers.get_mut(name) else {
                    return;
                };
                let size = (width.max(1) as i32, height.max(1) as i32);
                if cover.size != Some(size) {
                    cover.size = Some(size);
                    cover.viewport.set_destination(size.0, size.1);
                    // Drawn again at the next step, at its new size.
                    cover.drawn = None;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(cover) = state.covers.remove(name) {
                    cover.destroy();
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: ZwlrLayerShellV1);

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    use super::*;

    #[test]
    fn a_full_socket_is_waited_on_not_given_up() {
        let full = WaylandError::Io(Error::from(ErrorKind::WouldBlock));
        assert_eq!(flushed(Err(full)), Flushed::Blocked);
    }

    #[test]
    fn a_lost_connection_ends_it() {
        let lost = WaylandError::Io(Error::from(ErrorKind::BrokenPipe));
        assert_eq!(flushed(Err(lost)), Flushed::Failed);
        assert_eq!(flushed(Ok(())), Flushed::Done);
    }
}
