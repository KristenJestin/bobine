//! The notifications adapter: `org.freedesktop.Notifications` on the session bus, reached with
//! zbus.
//!
//! Everything D-Bus happens on threads of their own (zbus runs its own async-io executor, the
//! one GPUI's dependencies already bring): the window's thread only sends the notifications down
//! a channel, and the clicks come back through `Clicks`. The bus is reached at the first
//! notification. Without a bus address the adapter stays silent; the harness and the tests get
//! none: `Shell::without_desktop` turns the desktop off, whatever the environment.

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use zbus::MatchRule;
use zbus::blocking::connection::Builder;
use zbus::blocking::{Connection, MessageIterator};
use zbus::message::Type as MessageType;
use zbus::zvariant::Value;

use super::{Clicks, Notification, Notifier, Open};
use crate::mpris::file_url;

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// The action a click on the notification itself invokes.
const DEFAULT: &str = "default";

/// How many notifications shown are remembered for a click: a desktop that never says one was
/// closed would otherwise have them pile up while Bobine runs.
const REMEMBERED: usize = 64;

/// The notifications shown, by the desktop's id, with what a click on each opens; the oldest
/// forgotten beyond `REMEMBERED`.
#[derive(Default)]
struct Shown {
    open: HashMap<u32, Open>,
    order: VecDeque<u32>,
}

impl Shown {
    fn insert(&mut self, id: u32, open: Open) {
        if self.open.insert(id, open).is_some() {
            self.order.retain(|kept| *kept != id);
        }
        self.order.push_back(id);
        while self.order.len() > REMEMBERED {
            if let Some(oldest) = self.order.pop_front() {
                self.open.remove(&oldest);
            }
        }
    }

    fn remove(&mut self, id: u32) -> Option<Open> {
        self.order.retain(|kept| *kept != id);
        self.open.remove(&id)
    }
}

pub struct Dbus {
    /// `None`: no bus to reach.
    address: Option<String>,
    to: Option<Sender<Notification>>,
    clicks: Clicks,
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
            to: None,
            clicks: Clicks::default(),
        }
    }
}

impl Notifier for Dbus {
    fn notify(&mut self, notification: &Notification) {
        if self.to.is_none() {
            let Some(address) = self.address.clone() else {
                return;
            };
            let (to, notifications) = mpsc::channel();
            let clicks = self.clicks.clone();
            let thread = std::thread::Builder::new()
                .name("bobine-notify".into())
                .spawn(move || {
                    if let Err(error) = serve(&address, notifications, clicks) {
                        eprintln!("bobine: the new episodes are not told to the desktop: {error}");
                    }
                });
            if thread.is_err() {
                self.address = None;
                return;
            }
            self.to = Some(to);
        }
        if let Some(to) = &self.to {
            // A bus thread that ended (no bus) hears nothing: nothing to say.
            let _ = to.send(notification.clone());
        }
    }

    fn clicks(&self) -> Clicks {
        self.clicks.clone()
    }
}

/// The bus thread: the desktop's signals listened to, then each notification shown, until the
/// window's side is gone.
fn serve(address: &str, notifications: Receiver<Notification>, clicks: Clicks) -> zbus::Result<()> {
    let connection = Builder::address(address)?.build()?;
    // Listened to before the first notification goes, so that no click on it is missed.
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .interface(NAME)?
        .path(PATH)?
        .build();
    let signals = MessageIterator::for_match_rule(rule, &connection, None)?;
    let shown: Arc<Mutex<Shown>> = Arc::default();
    let heard = shown.clone();
    std::thread::Builder::new()
        .name("bobine-notify-signals".into())
        .spawn(move || listen(signals, &heard, &clicks))?;
    let mut told = false;
    for notification in notifications {
        match show(&connection, &notification) {
            Ok(id) => {
                shown.lock().unwrap().insert(id, notification.open);
            }
            Err(error) if !std::mem::replace(&mut told, true) => {
                eprintln!("bobine: a new episode was not told to the desktop: {error}");
            }
            Err(_) => {}
        }
    }
    Ok(())
}

/// `Notify`: the series as the summary, the poster as the image, a click opening it.
fn show(connection: &Connection, notification: &Notification) -> zbus::Result<u32> {
    let mut hints: HashMap<&str, Value<'_>> = HashMap::new();
    hints.insert("desktop-entry", Value::from("bobine"));
    let icon = notification.icon.as_deref().map(file_url);
    if let Some(icon) = &icon {
        hints.insert("image-path", Value::from(icon.as_str()));
    }
    let reply = connection.call_method(
        Some(NAME),
        PATH,
        Some(NAME),
        "Notify",
        &(
            "Bobine",
            0u32,
            "",
            notification.summary.as_str(),
            notification.body.as_str(),
            vec![DEFAULT, "Open"],
            hints,
            -1i32,
        ),
    )?;
    reply.body().deserialize()
}

/// The desktop's signals: a click on a notification shown is told to the window; a notification
/// closed is forgotten.
fn listen(signals: MessageIterator, shown: &Mutex<Shown>, clicks: &Clicks) {
    for message in signals.flatten() {
        let header = message.header();
        match header.member().map(|member| member.as_str()) {
            Some("ActionInvoked") => {
                let Ok((id, action)) = message.body().deserialize::<(u32, String)>() else {
                    continue;
                };
                if action != DEFAULT {
                    continue;
                }
                if let Some(open) = shown.lock().unwrap().remove(id) {
                    clicks.push(open);
                }
            }
            Some("NotificationClosed") => {
                if let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>() {
                    shown.lock().unwrap().remove(id);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    //! The real adapter on a private bus, with a notification server of the test's own on it:
    //! never the session bus of the machine, never a notification on the real desktop.

    use std::path::PathBuf;
    use std::time::Duration;

    use bobine::provider::ItemId;
    use zbus::interface;
    use zbus::object_server::SignalEmitter;
    use zbus::zvariant::OwnedValue;

    use super::*;
    use crate::mpris::dbus::tests::{Bus, private_bus, until};

    /// What the fake server was asked to show.
    #[derive(Clone, Debug)]
    struct Asked {
        app: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
    }

    struct Daemon {
        asked: Arc<Mutex<Vec<Asked>>>,
    }

    #[interface(name = "org.freedesktop.Notifications")]
    impl Daemon {
        #[allow(clippy::too_many_arguments)]
        fn notify(
            &self,
            app_name: String,
            _replaces_id: u32,
            _app_icon: String,
            summary: String,
            body: String,
            actions: Vec<String>,
            hints: HashMap<String, OwnedValue>,
            _expire_timeout: i32,
        ) -> u32 {
            let mut asked = self.asked.lock().unwrap();
            asked.push(Asked {
                app: app_name,
                summary,
                body,
                actions,
                hints,
            });
            asked.len() as u32
        }

        fn get_capabilities(&self) -> Vec<String> {
            vec!["actions".into(), "body".into()]
        }

        #[zbus(signal)]
        async fn action_invoked(
            emitter: &SignalEmitter<'_>,
            id: u32,
            action_key: &str,
        ) -> zbus::Result<()>;

        #[zbus(signal)]
        async fn notification_closed(
            emitter: &SignalEmitter<'_>,
            id: u32,
            reason: u32,
        ) -> zbus::Result<()>;
    }

    /// The test's notification server on `bus`.
    struct Desktop {
        connection: Connection,
        asked: Arc<Mutex<Vec<Asked>>>,
    }

    impl Desktop {
        fn on(bus: &Bus) -> Self {
            let asked = Arc::default();
            let daemon = Daemon {
                asked: Arc::clone(&asked),
            };
            let connection = Builder::address(bus.address.as_str())
                .unwrap()
                .serve_at(PATH, daemon)
                .unwrap()
                .name(NAME)
                .unwrap()
                .build()
                .unwrap();
            Self { connection, asked }
        }

        fn asked(&self) -> Vec<Asked> {
            self.asked.lock().unwrap().clone()
        }

        fn emitter(&self) -> SignalEmitter<'static> {
            SignalEmitter::new(self.connection.inner(), PATH).unwrap()
        }

        fn click(&self, id: u32, action: &str) {
            zbus::block_on(Daemon::action_invoked(&self.emitter(), id, action)).unwrap();
        }

        fn close(&self, id: u32) {
            zbus::block_on(Daemon::notification_closed(&self.emitter(), id, 2)).unwrap();
        }
    }

    fn notification(series: &str) -> Notification {
        Notification {
            summary: "Les Marées".into(),
            body: "S2 · E4 is available".into(),
            icon: Some(PathBuf::from("/tmp/cache/bobine/mpris/series 1.t1.jpg")),
            open: Open {
                account: "server-1/user-1".into(),
                series: ItemId(series.into()),
                season: ItemId(format!("{series}-s2")),
                episode: ItemId(format!("{series}-s2-e4")),
            },
        }
    }

    /// The clicks heard within a moment.
    fn heard(clicks: &Clicks) -> Vec<Open> {
        let (to, from) = mpsc::channel();
        let clicks = clicks.clone();
        std::thread::spawn(move || {
            loop {
                if to.send(zbus::block_on(clicks.next())).is_err() {
                    break;
                }
            }
        });
        let mut heard = Vec::new();
        while let Ok(open) = from.recv_timeout(Duration::from_millis(500)) {
            heard.push(open);
        }
        heard
    }

    #[test]
    fn a_notification_shows_the_series_what_is_new_and_the_poster() {
        let Some(bus) = private_bus() else { return };
        let desktop = Desktop::on(&bus);
        let mut dbus = Dbus::at(Some(bus.address.clone()));
        dbus.notify(&notification("series-1"));
        assert!(until(|| desktop.asked().len() == 1));
        let asked = desktop.asked().remove(0);
        assert_eq!(asked.app, "Bobine");
        assert_eq!(asked.summary, "Les Marées");
        assert_eq!(asked.body, "S2 · E4 is available");
        assert_eq!(asked.actions, [DEFAULT, "Open"]);
        let text = |key: &str| String::try_from(asked.hints[key].clone()).unwrap();
        assert_eq!(
            text("image-path"),
            "file:///tmp/cache/bobine/mpris/series%201.t1.jpg"
        );
        assert_eq!(text("desktop-entry"), "bobine");
    }

    #[test]
    fn a_click_on_a_notification_is_heard_with_what_it_opens() {
        let Some(bus) = private_bus() else { return };
        let desktop = Desktop::on(&bus);
        let mut dbus = Dbus::at(Some(bus.address.clone()));
        dbus.notify(&notification("series-1"));
        dbus.notify(&notification("series-4"));
        assert!(until(|| desktop.asked().len() == 2));
        // The second, then another action than the click itself, which opens nothing.
        desktop.click(2, DEFAULT);
        desktop.click(1, "snooze");
        assert_eq!(heard(&dbus.clicks()), [notification("series-4").open]);
    }

    #[test]
    fn a_closed_notification_opens_nothing() {
        let Some(bus) = private_bus() else { return };
        let desktop = Desktop::on(&bus);
        let mut dbus = Dbus::at(Some(bus.address.clone()));
        dbus.notify(&notification("series-1"));
        assert!(until(|| desktop.asked().len() == 1));
        desktop.close(1);
        std::thread::sleep(Duration::from_millis(100));
        desktop.click(1, DEFAULT);
        assert_eq!(heard(&dbus.clicks()), []);
    }

    #[test]
    fn a_notification_never_holds_an_address() {
        let Some(bus) = private_bus() else { return };
        let desktop = Desktop::on(&bus);
        let mut dbus = Dbus::at(Some(bus.address.clone()));
        dbus.notify(&notification("series-1"));
        assert!(until(|| desktop.asked().len() == 1));
        let printed = format!("{:?}", desktop.asked());
        assert!(!printed.contains("http"), "{printed}");
    }

    #[test]
    fn only_the_latest_notifications_shown_are_remembered() {
        let mut shown = Shown::default();
        for id in 1..=REMEMBERED as u32 + 1 {
            shown.insert(id, notification(&format!("series-{id}")).open);
        }
        assert_eq!(shown.open.len(), REMEMBERED);
        assert_eq!(shown.remove(1), None, "the oldest forgotten");
        assert_eq!(shown.remove(2), Some(notification("series-2").open));
        assert_eq!(shown.order.len(), REMEMBERED - 1);
    }

    #[test]
    fn without_a_bus_address_the_notifier_stays_silent() {
        let mut dbus = Dbus::at(None);
        dbus.notify(&notification("series-1"));
        assert!(dbus.to.is_none());
    }
}
