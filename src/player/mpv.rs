//! The player port over mpv. What it asks of mpv goes through `MpvClient`, which libmpv
//! implements (`super::libmpv`) and the tests fake: the mapping from the port to mpv's commands
//! and properties is tested without mpv.

use super::{Player, PlayerError, PlayerEvent, Seek, Stream, TrackKind, tracks};

/// The properties the player follows; mpv says when one changes.
pub const OBSERVED: [&str; 6] = ["time-pos", "duration", "pause", "track-list", "aid", "sid"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MpvEvent {
    /// One of `OBSERVED` changed: its value is read again.
    Changed(String),
    EndFile(EndReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    /// Stopped by a command or replaced by the next file: nothing to report.
    Stopped,
    /// mpv's error code in words.
    Error(String),
}

pub trait MpvClient {
    fn set(&self, name: &str, value: &str) -> Result<(), PlayerError>;
    fn command(&self, args: &[&str]) -> Result<(), PlayerError>;
    /// A property as text, `None` when it is unavailable.
    fn get(&self, name: &str) -> Option<String>;
    /// The next event, without waiting.
    fn next_event(&self) -> Option<MpvEvent>;
}

/// The real player shares its mpv with the render thread.
impl<C: MpvClient> MpvClient for std::sync::Arc<C> {
    fn set(&self, name: &str, value: &str) -> Result<(), PlayerError> {
        (**self).set(name, value)
    }
    fn command(&self, args: &[&str]) -> Result<(), PlayerError> {
        (**self).command(args)
    }
    fn get(&self, name: &str) -> Option<String> {
        (**self).get(name)
    }
    fn next_event(&self) -> Option<MpvEvent> {
        (**self).next_event()
    }
}

pub struct MpvPlayer<C: MpvClient> {
    client: C,
}

impl<C: MpvClient> MpvPlayer<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    #[cfg(test)]
    pub fn client(&self) -> &C {
        &self.client
    }

    fn number(&self, name: &str) -> Option<f64> {
        self.client.get(name)?.parse().ok()
    }
}

impl<C: MpvClient> Player for MpvPlayer<C> {
    fn load(&mut self, stream: Stream<'_>, start: f64) -> Result<(), PlayerError> {
        self.client
            .command(&["change-list", "http-header-fields", "clr", ""])?;
        for (name, value) in stream.headers {
            // Appended one by one: a header's own commas are not taken for separators.
            let header = format!("{name}: {value}");
            self.client
                .command(&["change-list", "http-header-fields", "append", &header])?;
        }
        self.client.set("start", &start.to_string())?;
        self.client.command(&["loadfile", stream.url, "replace"])
    }

    // A command refused here (a seek before the film is open, a track that just went) changes
    // nothing on screen: there is nothing to tell.
    fn set_paused(&mut self, paused: bool) {
        let _ = self.client.set("pause", yes_no(paused));
    }

    fn seek(&mut self, seek: Seek) {
        let (seconds, mode) = match seek {
            Seek::By(seconds) => (seconds, "relative"),
            Seek::To(seconds) => (seconds, "absolute"),
        };
        let _ = self.client.command(&["seek", &seconds.to_string(), mode]);
    }

    fn set_volume(&mut self, percent: f64) {
        let _ = self.client.set("volume", &percent.to_string());
    }

    fn set_muted(&mut self, muted: bool) {
        let _ = self.client.set("mute", yes_no(muted));
    }

    fn select_audio(&mut self, id: Option<i64>) {
        let _ = self.client.set("aid", &track_id(id));
    }

    fn select_subtitle(&mut self, id: Option<i64>) {
        let _ = self.client.set("sid", &track_id(id));
    }

    fn add_subtitle(
        &mut self,
        url: &str,
        title: &str,
        language: Option<&str>,
    ) -> Result<(), PlayerError> {
        self.client.command(&[
            "sub-add",
            url,
            "select",
            title,
            language.unwrap_or_default(),
        ])
    }

    fn events(&mut self) -> Vec<PlayerEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.client.next_event() {
            let event = match event {
                MpvEvent::Changed(name) => match name.as_str() {
                    "time-pos" => self.number("time-pos").map(PlayerEvent::Position),
                    "duration" => self.number("duration").map(PlayerEvent::Duration),
                    "pause" => self
                        .client
                        .get("pause")
                        .map(|paused| PlayerEvent::Paused(paused == "yes")),
                    _ => Some(PlayerEvent::Tracks(
                        tracks::from_mpv(|name| self.client.get(name))
                            .into_iter()
                            .filter(|track| track.kind != TrackKind::Video)
                            .collect(),
                    )),
                },
                MpvEvent::EndFile(EndReason::Eof) => Some(PlayerEvent::EndOfFile),
                MpvEvent::EndFile(EndReason::Error(reason)) => {
                    Some(PlayerEvent::Error(PlayerError(reason)))
                }
                MpvEvent::EndFile(EndReason::Stopped) => None,
            };
            events.extend(event);
        }
        events
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn track_id(id: Option<i64>) -> String {
    id.map_or_else(|| "no".to_string(), |id| id.to_string())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    use super::*;

    /// An mpv that plays nothing: it keeps what it is told, answers from a table and hands out
    /// queued events.
    #[derive(Default)]
    struct FakeMpv {
        log: RefCell<Vec<Vec<String>>>,
        properties: RefCell<HashMap<String, String>>,
        events: RefCell<VecDeque<MpvEvent>>,
    }

    impl FakeMpv {
        fn with(properties: &[(&str, &str)], events: &[MpvEvent]) -> Self {
            let fake = Self::default();
            for (name, value) in properties {
                fake.properties
                    .borrow_mut()
                    .insert(name.to_string(), value.to_string());
            }
            fake.events.borrow_mut().extend(events.iter().cloned());
            fake
        }
    }

    impl MpvClient for FakeMpv {
        fn set(&self, name: &str, value: &str) -> Result<(), PlayerError> {
            self.log
                .borrow_mut()
                .push(vec!["set".into(), name.into(), value.into()]);
            Ok(())
        }
        fn command(&self, args: &[&str]) -> Result<(), PlayerError> {
            self.log
                .borrow_mut()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            Ok(())
        }
        fn get(&self, name: &str) -> Option<String> {
            self.properties.borrow().get(name).cloned()
        }
        fn next_event(&self) -> Option<MpvEvent> {
            self.events.borrow_mut().pop_front()
        }
    }

    fn log(player: &MpvPlayer<FakeMpv>) -> Vec<Vec<String>> {
        player.client().log.borrow().clone()
    }

    fn line(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn changed(name: &str) -> MpvEvent {
        MpvEvent::Changed(name.into())
    }

    #[test]
    fn loading_a_stream_sends_its_headers_and_starts_at_the_position() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        let stream = Stream {
            url: "http://media.example/v.mkv",
            headers: &[(
                "Authorization",
                "MediaBrowser Client=\"Bobine\", Token=\"t\"",
            )],
        };
        player.load(stream, 754.5).unwrap();
        assert_eq!(
            log(&player),
            [
                line(&["change-list", "http-header-fields", "clr", ""]),
                // One header per item: its commas are not separators.
                line(&[
                    "change-list",
                    "http-header-fields",
                    "append",
                    "Authorization: MediaBrowser Client=\"Bobine\", Token=\"t\""
                ]),
                line(&["set", "start", "754.5"]),
                line(&["loadfile", "http://media.example/v.mkv", "replace"]),
            ]
        );
    }

    #[test]
    fn play_and_pause_set_the_pause_property() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.set_paused(true);
        player.set_paused(false);
        assert_eq!(
            log(&player),
            [
                line(&["set", "pause", "yes"]),
                line(&["set", "pause", "no"])
            ]
        );
    }

    #[test]
    fn seeking_is_relative_or_absolute() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.seek(Seek::By(-10.));
        player.seek(Seek::To(3600.));
        assert_eq!(
            log(&player),
            [
                line(&["seek", "-10", "relative"]),
                line(&["seek", "3600", "absolute"])
            ]
        );
    }

    #[test]
    fn volume_and_mute_set_their_properties() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.set_volume(80.);
        player.set_muted(true);
        assert_eq!(
            log(&player),
            [
                line(&["set", "volume", "80"]),
                line(&["set", "mute", "yes"])
            ]
        );
    }

    #[test]
    fn selecting_a_track_or_none_sets_aid_and_sid() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.select_audio(Some(2));
        player.select_subtitle(None);
        player.select_subtitle(Some(3));
        player.select_audio(None);
        assert_eq!(
            log(&player),
            [
                line(&["set", "aid", "2"]),
                line(&["set", "sid", "no"]),
                line(&["set", "sid", "3"]),
                line(&["set", "aid", "no"]),
            ]
        );
    }

    #[test]
    fn an_external_subtitle_is_added_by_url_and_shown() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player
            .add_subtitle(
                "http://media.example/s.srt?ApiKey=k",
                "Français",
                Some("fre"),
            )
            .unwrap();
        player
            .add_subtitle("http://media.example/t.srt", "Sous-titres", None)
            .unwrap();
        // Always a title: else mpv names the track after the end of its address.
        assert_eq!(
            log(&player),
            [
                line(&[
                    "sub-add",
                    "http://media.example/s.srt?ApiKey=k",
                    "select",
                    "Français",
                    "fre"
                ]),
                line(&[
                    "sub-add",
                    "http://media.example/t.srt",
                    "select",
                    "Sous-titres",
                    ""
                ]),
            ]
        );
    }

    #[test]
    fn position_duration_and_pause_are_reported_when_they_change() {
        let mpv = FakeMpv::with(
            &[("time-pos", "12.5"), ("duration", "5400"), ("pause", "yes")],
            &[changed("duration"), changed("time-pos"), changed("pause")],
        );
        let mut player = MpvPlayer::new(mpv);
        assert_eq!(
            player.events(),
            [
                PlayerEvent::Duration(5400.),
                PlayerEvent::Position(12.5),
                PlayerEvent::Paused(true),
            ]
        );
        assert_eq!(player.events(), []);
    }

    #[test]
    fn a_property_mpv_cannot_give_yet_is_not_reported() {
        let mpv = FakeMpv::with(&[], &[changed("time-pos"), changed("duration")]);
        let mut player = MpvPlayer::new(mpv);
        assert_eq!(player.events(), []);
    }

    #[test]
    fn the_tracks_are_reported_without_the_video_ones() {
        let mpv = FakeMpv::with(
            &[
                ("track-list/count", "2"),
                ("track-list/0/id", "1"),
                ("track-list/0/type", "video"),
                ("track-list/1/id", "1"),
                ("track-list/1/type", "audio"),
                ("track-list/1/selected", "yes"),
            ],
            &[changed("track-list"), changed("aid")],
        );
        let mut player = MpvPlayer::new(mpv);
        let events = player.events();
        assert_eq!(events.len(), 2);
        let PlayerEvent::Tracks(tracks) = &events[0] else {
            panic!("{events:?}");
        };
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].kind, TrackKind::Audio);
        assert!(tracks[0].selected);
    }

    #[test]
    fn the_end_of_the_film_is_reported() {
        let mpv = FakeMpv::with(&[], &[MpvEvent::EndFile(EndReason::Eof)]);
        assert_eq!(MpvPlayer::new(mpv).events(), [PlayerEvent::EndOfFile]);
    }

    #[test]
    fn a_film_that_fails_is_reported_with_the_reason() {
        let mpv = FakeMpv::with(
            &[],
            &[
                MpvEvent::EndFile(EndReason::Stopped),
                MpvEvent::EndFile(EndReason::Error("loading failed".into())),
            ],
        );
        assert_eq!(
            MpvPlayer::new(mpv).events(),
            [PlayerEvent::Error(PlayerError("loading failed".into()))]
        );
    }
}
