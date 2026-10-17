//! The MPRIS adapter: `org.mpris.MediaPlayer2.bobine` on the session bus, served with zbus.
//!
//! Everything D-Bus happens on a thread of its own (zbus runs its own async-io executor, the
//! one GPUI's dependencies already bring): the window's thread only sends what changed down a
//! channel and reads the commands back. The bus is reached at the first title that plays, so
//! nothing shows in the bar widget before. Without `DBUS_SESSION_BUS_ADDRESS` (the headless
//! checks) there is no bus, and the adapter stays silent.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use zbus::blocking::Connection;
use zbus::blocking::connection::Builder;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::interface;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use super::{Command, MediaControls, State, Status, file_url, track_id};

const NAME: &str = "org.mpris.MediaPlayer2.bobine";
const PATH: &str = "/org/mpris/MediaPlayer2";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

/// What the window's thread tells the bus thread.
enum Message {
    Show(State),
    /// Microseconds.
    Seeked(i64),
    Close,
}

struct Worker {
    to: Sender<Message>,
    thread: JoinHandle<()>,
}

pub struct Dbus {
    /// `None`: no bus to reach.
    address: Option<String>,
    worker: Option<Worker>,
    from: Option<Receiver<Command>>,
    /// Microseconds, read by the bus thread when the desktop asks.
    position: Arc<AtomicI64>,
}

impl Dbus {
    /// The session bus of `DBUS_SESSION_BUS_ADDRESS`, and only it: unset, there is no bus (zbus
    /// would otherwise guess one under `XDG_RUNTIME_DIR`).
    #[cfg(not(test))]
    pub fn session() -> Self {
        Self::at(std::env::var("DBUS_SESSION_BUS_ADDRESS").ok())
    }

    pub fn at(address: Option<String>) -> Self {
        Self {
            address,
            worker: None,
            from: None,
            position: Arc::new(AtomicI64::new(0)),
        }
    }

    fn send(&mut self, message: Message) {
        if let Some(worker) = &self.worker {
            // A bus thread that ended (no bus) hears nothing: nothing to say.
            let _ = worker.to.send(message);
        }
    }
}

impl MediaControls for Dbus {
    fn show(&mut self, state: &State) {
        if self.worker.is_none() {
            // Nothing played yet: nothing to show.
            if state.status == Status::Stopped {
                return;
            }
            let Some(address) = self.address.clone() else {
                return;
            };
            let (to, messages) = mpsc::channel();
            let (commands, from) = mpsc::channel();
            let position = self.position.clone();
            let thread = std::thread::Builder::new()
                .name("bobine-mpris".into())
                .spawn(move || {
                    if let Err(error) = serve(&address, messages, commands, position) {
                        eprintln!("bobine: the desktop's media controls are off: {error}");
                    }
                });
            let Ok(thread) = thread else {
                self.address = None;
                return;
            };
            self.worker = Some(Worker { to, thread });
            self.from = Some(from);
        }
        self.send(Message::Show(state.clone()));
    }

    fn position(&mut self, seconds: f64) {
        self.position.store(micros(seconds), Ordering::Relaxed);
    }

    fn seeked(&mut self, seconds: f64) {
        self.send(Message::Seeked(micros(seconds)));
    }

    fn commands(&mut self) -> Vec<Command> {
        self.from
            .as_ref()
            .map(|from| from.try_iter().collect())
            .unwrap_or_default()
    }

    fn close(&mut self) {
        self.send(Message::Close);
        if let Some(worker) = self.worker.take() {
            let _ = worker.thread.join();
        }
        self.from = None;
        self.address = None;
    }
}

impl Drop for Dbus {
    fn drop(&mut self) {
        self.close();
    }
}

fn micros(seconds: f64) -> i64 {
    (seconds * 1e6).round() as i64
}

/// The bus thread: the name taken, then what the window says, until it closes.
fn serve(
    address: &str,
    messages: Receiver<Message>,
    commands: Sender<Command>,
    position: Arc<AtomicI64>,
) -> zbus::Result<()> {
    let player = Player {
        state: State::default(),
        position,
        commands,
    };
    let connection = Builder::address(address)?
        .serve_at(PATH, Root)?
        .serve_at(PATH, player)?
        .build()?;
    let name = take_name(&connection)?;
    let player = connection.object_server().interface::<_, Player>(PATH)?;
    serve_messages(
        messages,
        |message| match message {
            Message::Show(state) => {
                let mut shown = player.get_mut();
                let before = std::mem::replace(&mut shown.state, state);
                let now = &shown.state;
                let emitter = player.signal_emitter();
                zbus::block_on(async {
                    if before.track != now.track {
                        shown.metadata_changed(emitter).await?;
                        shown.can_play_changed(emitter).await?;
                        shown.can_pause_changed(emitter).await?;
                    }
                    if before.rate != now.rate {
                        shown.rate_changed(emitter).await?;
                    }
                    if before.can_go_next != now.can_go_next {
                        shown.can_go_next_changed(emitter).await?;
                    }
                    // Last: a widget that hears Playing already has the title that plays.
                    if before.status != now.status {
                        shown.playback_status_changed(emitter).await?;
                    }
                    zbus::Result::Ok(())
                })
            }
            Message::Seeked(micros) => {
                zbus::block_on(Player::seeked(player.signal_emitter(), micros))
            }
            Message::Close => Ok(()),
        },
        |error| eprintln!("bobine: the desktop's media controls missed a signal: {error}"),
    );
    connection.release_name(name.as_str())?;
    Ok(())
}

/// Every message until `Close`, or until the window's side is gone. A signal that fails to go
/// out is told once (`report`) and does not end the service: the next change may well go.
fn serve_messages(
    messages: Receiver<Message>,
    mut handle: impl FnMut(Message) -> zbus::Result<()>,
    mut report: impl FnMut(&zbus::Error),
) {
    let mut told = false;
    for message in messages {
        let closing = matches!(message, Message::Close);
        if let Err(error) = handle(message)
            && !std::mem::replace(&mut told, true)
        {
            report(&error);
        }
        if closing {
            break;
        }
    }
}

/// `org.mpris.MediaPlayer2.bobine`, or, when another Bobine holds it, the same with
/// `.instance<pid>`, as the specification says.
fn take_name(connection: &Connection) -> zbus::Result<String> {
    let flags = RequestNameFlags::DoNotQueue.into();
    // zbus says a name held by another as `NameTaken`.
    match connection.request_name_with_flags(NAME, flags) {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => Ok(NAME.into()),
        Err(error) if !matches!(error, zbus::Error::NameTaken) => Err(error),
        _ => {
            let name = format!("{NAME}.instance{}", std::process::id());
            connection.request_name_with_flags(name.as_str(), flags)?;
            Ok(name)
        }
    }
}

fn owned(value: Value<'_>) -> OwnedValue {
    value.try_to_owned().expect("no file descriptor in a value")
}

struct Root;

#[interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        "Bobine".into()
    }

    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "bobine".into()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

struct Player {
    state: State,
    position: Arc<AtomicI64>,
    commands: Sender<Command>,
}

impl Player {
    fn ask(&self, command: Command) {
        let _ = self.commands.send(command);
    }

    fn playing(&self) -> bool {
        self.state.track.is_some()
    }
}

#[interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn next(&self) {
        if self.state.can_go_next {
            self.ask(Command::Next);
        }
    }

    fn previous(&self) {}

    fn pause(&self) {
        if self.playing() {
            self.ask(Command::Pause);
        }
    }

    fn play_pause(&self) {
        if self.playing() {
            self.ask(Command::PlayPause);
        }
    }

    fn stop(&self) {
        if self.playing() {
            self.ask(Command::Stop);
        }
    }

    fn play(&self) {
        if self.playing() {
            self.ask(Command::Play);
        }
    }

    /// Microseconds from where the film is.
    fn seek(&self, offset: i64) {
        if self.playing() {
            self.ask(Command::SeekBy(offset as f64 / 1e6));
        }
    }

    /// Dropped when the track is not the one shown, or the position outside it.
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        let Some(track) = &self.state.track else {
            return;
        };
        let length = micros(track.length);
        if track_id.as_str() != super::track_id(&track.item)
            || position < 0
            || (length > 0 && position > length)
        {
            return;
        }
        self.ask(Command::SetPosition(position as f64 / 1e6));
    }

    fn open_uri(&self, _uri: String) {}

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        match self.state.status {
            Status::Playing => "Playing",
            Status::Paused => "Paused",
            Status::Stopped => "Stopped",
        }
        .into()
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.state.rate
    }

    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let mut metadata = HashMap::new();
        let Some(track) = &self.state.track else {
            let path = ObjectPath::from_static_str_unchecked(NO_TRACK);
            metadata.insert("mpris:trackid".into(), owned(Value::from(path)));
            return metadata;
        };
        let id = track_id(&track.item);
        let path = ObjectPath::try_from(id.as_str()).expect("a track id is an object path");
        metadata.insert("mpris:trackid".into(), owned(Value::from(path)));
        metadata.insert(
            "xesam:title".into(),
            owned(Value::from(track.title.as_str())),
        );
        if let Some(series) = &track.series {
            metadata.insert("xesam:album".into(), owned(Value::from(series.as_str())));
            metadata.insert(
                "xesam:artist".into(),
                owned(Value::from(vec![series.as_str()])),
            );
        }
        if track.length > 0. {
            metadata.insert(
                "mpris:length".into(),
                owned(Value::from(micros(track.length))),
            );
        }
        if let Some(art) = &track.art {
            metadata.insert("mpris:artUrl".into(), owned(Value::from(file_url(art))));
        }
        metadata
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.position.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        crate::player::settings::SPEEDS[0]
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        crate::player::settings::SPEEDS[crate::player::settings::SPEEDS.len() - 1]
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.state.can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.playing()
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.playing()
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    //! The real adapter on a private bus: a `dbus-daemon` started by the test, reached by its
    //! address, killed at the end. Never the session bus of the machine.

    use std::io::{BufRead, BufReader};
    use std::path::PathBuf;
    use std::process::{Child, Stdio};
    use std::time::{Duration, Instant};

    use zbus::MatchRule;
    use zbus::blocking::{MessageIterator, Proxy};
    use zbus::message::Type as MessageType;

    use super::super::Track;
    use super::*;

    const WAIT: Duration = Duration::from_secs(5);

    struct Bus {
        daemon: Child,
        address: String,
    }

    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    /// A bus of the test's own; `None`, said aloud, when `dbus-daemon` is not installed.
    fn private_bus() -> Option<Bus> {
        let spawned = std::process::Command::new("dbus-daemon")
            .args(["--session", "--print-address", "--nofork"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let mut daemon = match spawned {
            Ok(daemon) => daemon,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("SKIPPED: dbus-daemon is not installed, the MPRIS adapter is not tried");
                return None;
            }
            Err(error) => panic!("dbus-daemon does not start: {error}"),
        };
        let mut line = String::new();
        BufReader::new(daemon.stdout.take().expect("piped"))
            .read_line(&mut line)
            .expect("dbus-daemon prints its address");
        Some(Bus {
            daemon,
            address: line.trim().to_string(),
        })
    }

    fn client(bus: &Bus) -> Connection {
        Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .unwrap()
    }

    fn proxy<'a>(connection: &'a Connection, name: &'a str, interface: &'a str) -> Proxy<'a> {
        zbus::blocking::proxy::Builder::new(connection)
            .destination(name)
            .unwrap()
            .path(PATH)
            .unwrap()
            .interface(interface)
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .unwrap()
    }

    fn player<'a>(connection: &'a Connection) -> Proxy<'a> {
        proxy(connection, NAME, "org.mpris.MediaPlayer2.Player")
    }

    fn owned_by_someone(connection: &Connection, name: &str) -> bool {
        let dbus = zbus::blocking::fdo::DBusProxy::new(connection).unwrap();
        dbus.name_has_owner(name.try_into().unwrap()).unwrap()
    }

    /// Waits for `done`, five seconds at most.
    fn until(mut done: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < WAIT {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn episode() -> Track {
        Track {
            item: "episode-3".into(),
            title: "La Digue".into(),
            series: Some("Les Marées".into()),
            art: Some(PathBuf::from("/tmp/cache/bobine/mpris/episode-3.jpg")),
            length: 2520.,
        }
    }

    fn playing() -> State {
        State {
            status: Status::Playing,
            track: Some(episode()),
            can_go_next: true,
            rate: 1.,
        }
    }

    /// The adapter on `bus`, showing `state`, once its name is on the bus.
    fn shown(bus: &Bus, client: &Connection, state: &State) -> Dbus {
        let mut dbus = Dbus::at(Some(bus.address.clone()));
        dbus.show(state);
        assert!(
            until(|| owned_by_someone(client, NAME)),
            "the name is taken"
        );
        dbus
    }

    fn status(player: &Proxy<'_>) -> String {
        player.get_property("PlaybackStatus").unwrap()
    }

    fn commands_until(dbus: &mut Dbus, count: usize) -> Vec<Command> {
        let mut heard = Vec::new();
        until(|| {
            heard.extend(dbus.commands());
            heard.len() >= count
        });
        heard
    }

    #[test]
    fn a_playback_publishes_its_metadata_and_its_status() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        dbus.position(61.5);
        let root = proxy(&client, NAME, "org.mpris.MediaPlayer2");
        let identity: String = root.get_property("Identity").unwrap();
        let entry: String = root.get_property("DesktopEntry").unwrap();
        assert_eq!((identity.as_str(), entry.as_str()), ("Bobine", "bobine"));
        let player = player(&client);
        assert!(until(|| status(&player) == "Playing"));
        let metadata: HashMap<String, OwnedValue> = player.get_property("Metadata").unwrap();
        let text = |key: &str| String::try_from(metadata[key].clone()).unwrap();
        assert_eq!(text("xesam:title"), "La Digue");
        assert_eq!(text("xesam:album"), "Les Marées");
        let artist = Vec::<String>::try_from(metadata["xesam:artist"].clone()).unwrap();
        assert_eq!(artist, ["Les Marées"]);
        assert_eq!(
            i64::try_from(metadata["mpris:length"].clone()).unwrap(),
            2_520_000_000
        );
        assert_eq!(
            text("mpris:artUrl"),
            "file:///tmp/cache/bobine/mpris/episode-3.jpg"
        );
        let id = ObjectPath::try_from(metadata["mpris:trackid"].clone()).unwrap();
        assert_eq!(id.as_str(), track_id("episode-3"));
        assert_eq!(player.get_property::<i64>("Position").unwrap(), 61_500_000);
        assert!(player.get_property::<bool>("CanGoNext").unwrap());
        assert!(!player.get_property::<bool>("CanGoPrevious").unwrap());
        assert!(player.get_property::<bool>("CanSeek").unwrap());
    }

    #[test]
    fn pause_from_mpris_pauses_the_player_and_the_status_follows() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        let player = player(&client);
        assert!(until(|| status(&player) == "Playing"));
        player.call_method("Pause", &()).unwrap();
        player.call_method("PlayPause", &()).unwrap();
        player.call_method("Play", &()).unwrap();
        assert_eq!(
            commands_until(&mut dbus, 3),
            [Command::Pause, Command::PlayPause, Command::Play]
        );
        dbus.show(&State {
            status: Status::Paused,
            ..playing()
        });
        assert!(until(|| status(&player) == "Paused"));
    }

    #[test]
    fn seek_and_set_position_move_the_player() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        let player = player(&client);
        assert!(until(|| status(&player) == "Playing"));
        let current = ObjectPath::try_from(track_id("episode-3")).unwrap();
        let stale = ObjectPath::try_from(track_id("episode-2")).unwrap();
        player.call_method("Seek", &(-10_000_000i64)).unwrap();
        // A stale track, a position before the start or past the end: dropped.
        player
            .call_method("SetPosition", &(stale, 60_000_000i64))
            .unwrap();
        player
            .call_method("SetPosition", &(current.clone(), -1i64))
            .unwrap();
        player
            .call_method("SetPosition", &(current.clone(), 9_999_000_000i64))
            .unwrap();
        player
            .call_method("SetPosition", &(current, 60_000_000i64))
            .unwrap();
        let heard = commands_until(&mut dbus, 2);
        std::thread::sleep(Duration::from_millis(100));
        let heard: Vec<Command> = heard.into_iter().chain(dbus.commands()).collect();
        assert_eq!(heard, [Command::SeekBy(-10.), Command::SetPosition(60.)]);
    }

    #[test]
    fn next_is_heard_only_when_an_episode_follows() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let last = State {
            can_go_next: false,
            ..playing()
        };
        let mut dbus = shown(&bus, &client, &last);
        let player = player(&client);
        assert!(until(|| status(&player) == "Playing"));
        player.call_method("Next", &()).unwrap();
        player.call_method("Previous", &()).unwrap();
        dbus.show(&playing());
        assert!(until(|| player.get_property::<bool>("CanGoNext").unwrap()));
        player.call_method("Next", &()).unwrap();
        assert_eq!(commands_until(&mut dbus, 1), [Command::Next]);
    }

    /// Every signal of the player's path, as they come.
    fn signals(bus: &Bus) -> Receiver<zbus::Message> {
        let connection = client(bus);
        let rule = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .path(PATH)
            .unwrap()
            .build();
        let messages = MessageIterator::for_match_rule(rule, &connection, None).unwrap();
        let (to, from) = mpsc::channel();
        std::thread::spawn(move || {
            let _connection = connection;
            for message in messages.flatten() {
                if to.send(message).is_err() {
                    break;
                }
            }
        });
        from
    }

    /// The members of the signals heard within a moment, with the properties each change names.
    fn heard(signals: &Receiver<zbus::Message>) -> Vec<(String, Vec<String>)> {
        let mut heard = Vec::new();
        while let Ok(message) = signals.recv_timeout(Duration::from_millis(500)) {
            let header = message.header();
            let member = header.member().unwrap().to_string();
            let changed = if member == "PropertiesChanged" {
                let (_, changed, _): (String, HashMap<String, OwnedValue>, Vec<String>) =
                    message.body().deserialize().unwrap();
                let mut names: Vec<String> = changed.into_keys().collect();
                names.sort();
                names
            } else {
                Vec::new()
            };
            heard.push((member, changed));
        }
        heard
    }

    #[test]
    fn a_change_of_status_metadata_or_rate_is_announced_and_a_jump_is_a_seeked_signal() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let signals = signals(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        heard(&signals);
        dbus.show(&State {
            status: Status::Paused,
            ..playing()
        });
        assert_eq!(
            heard(&signals),
            [("PropertiesChanged".into(), vec!["PlaybackStatus".into()])]
        );
        dbus.show(&State {
            status: Status::Paused,
            rate: 1.5,
            ..playing()
        });
        assert_eq!(
            heard(&signals),
            [("PropertiesChanged".into(), vec!["Rate".into()])]
        );
        let mut longer = playing();
        longer.status = Status::Paused;
        longer.rate = 1.5;
        longer.track.as_mut().unwrap().length = 2600.;
        dbus.show(&longer);
        let changes = heard(&signals);
        assert!(
            changes.contains(&("PropertiesChanged".into(), vec!["Metadata".into()])),
            "{changes:?}"
        );
        dbus.position(600.);
        assert_eq!(heard(&signals), [], "the position is not announced");
        dbus.seeked(600.);
        assert_eq!(heard(&signals), [("Seeked".into(), vec![])]);
    }

    #[test]
    fn a_new_title_is_announced_before_the_status_that_plays_it() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let signals = signals(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        dbus.show(&State::default());
        heard(&signals);
        let mut next = playing();
        next.track.as_mut().unwrap().item = "episode-4".into();
        dbus.show(&next);
        let changes = heard(&signals);
        let at = |name: &str| {
            changes
                .iter()
                .position(|(_, names)| names.iter().any(|n| n == name))
                .unwrap_or_else(|| panic!("{name} not announced: {changes:?}"))
        };
        assert!(
            at("Metadata") <= at("PlaybackStatus"),
            "a widget must not see Playing with no track: {changes:?}"
        );
    }

    #[test]
    fn a_failed_send_is_told_once_and_the_service_goes_on() {
        let (to, messages) = mpsc::channel();
        for _ in 0..3 {
            to.send(Message::Seeked(1)).unwrap();
        }
        to.send(Message::Close).unwrap();
        to.send(Message::Seeked(9)).unwrap();
        let mut handled = Vec::new();
        let mut told = Vec::new();
        serve_messages(
            messages,
            |message| {
                handled.push(matches!(message, Message::Close));
                Err(zbus::Error::Failure("the bus hung up".into()))
            },
            |error| told.push(error.to_string()),
        );
        assert_eq!(
            handled,
            [false, false, false, true],
            "all of them, up to Close"
        );
        assert_eq!(told.len(), 1, "told once: {told:?}");
    }

    #[test]
    fn stopping_the_film_sets_it_to_stopped() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        let player = player(&client);
        assert!(until(|| status(&player) == "Playing"));
        dbus.show(&State::default());
        assert!(until(|| status(&player) == "Stopped"));
        let metadata: HashMap<String, OwnedValue> = player.get_property("Metadata").unwrap();
        let id = ObjectPath::try_from(metadata["mpris:trackid"].clone()).unwrap();
        assert_eq!(id.as_str(), NO_TRACK);
        assert!(!metadata.contains_key("xesam:title"));
    }

    #[test]
    fn the_name_is_released_when_the_window_closes() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let mut dbus = shown(&bus, &client, &playing());
        dbus.close();
        assert!(!owned_by_someone(&client, NAME));
    }

    #[test]
    fn a_second_bobine_takes_an_instance_name() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let _first = shown(&bus, &client, &playing());
        let mut second = Dbus::at(Some(bus.address.clone()));
        second.show(&playing());
        let instance = format!("{NAME}.instance{}", std::process::id());
        assert!(until(|| owned_by_someone(&client, &instance)));
    }

    #[test]
    fn nothing_reaches_the_bus_before_a_title_plays() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let mut dbus = Dbus::at(Some(bus.address.clone()));
        dbus.show(&State::default());
        std::thread::sleep(Duration::from_millis(200));
        assert!(!owned_by_someone(&client, NAME));
        assert!(dbus.worker.is_none());
    }

    #[test]
    fn without_a_bus_address_the_controls_stay_silent() {
        let mut dbus = Dbus::at(None);
        dbus.show(&playing());
        dbus.position(10.);
        dbus.seeked(10.);
        assert!(dbus.worker.is_none());
        assert_eq!(dbus.commands(), []);
        dbus.close();
    }

    #[test]
    fn a_published_property_never_holds_an_address() {
        let Some(bus) = private_bus() else { return };
        let client = client(&bus);
        let _dbus = shown(&bus, &client, &playing());
        let properties = zbus::blocking::fdo::PropertiesProxy::builder(&client)
            .destination(NAME)
            .unwrap()
            .path(PATH)
            .unwrap()
            .build()
            .unwrap();
        let player = player(&client);
        assert!(until(|| status(&player) == "Playing"));
        for interface in ["org.mpris.MediaPlayer2", "org.mpris.MediaPlayer2.Player"] {
            let all = properties.get_all(interface.try_into().unwrap()).unwrap();
            let printed = format!("{all:?}");
            assert!(!printed.contains("http"), "{printed}");
        }
    }
}
