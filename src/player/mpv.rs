//! The player port over mpv. What it asks of mpv goes through `MpvClient`, which libmpv
//! implements (`super::libmpv`) and the tests fake: the mapping from the port to mpv's commands
//! and properties is tested without mpv.

use super::{
    Aspect, FirstSubtitle, Media, PlaybackInfo, Player, PlayerError, PlayerEvent, Seek, TrackKind,
    tracks,
};

/// The properties the player follows; mpv says when one changes.
/// `vo-configured` turns on with the first picture the video output gets, `paused-for-cache`
/// while the stream waits for data.
pub const OBSERVED: [&str; 8] = [
    "time-pos",
    "duration",
    "pause",
    "track-list",
    "aid",
    "sid",
    "vo-configured",
    "paused-for-cache",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MpvEvent {
    /// One of `OBSERVED` changed: its value is read again.
    Changed(String),
    /// mpv begins to open a file: one per `loadfile`, before its `FileLoaded`.
    StartFile,
    /// The file is open: its tracks exist.
    FileLoaded,
    EndFile(EndReason),
    /// An asynchronous command finished; `error` is mpv's error code in words when it failed.
    CommandReply {
        id: u64,
        error: Option<String>,
    },
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
    /// Queues a command that may do I/O (`sub-add` downloads its file) and returns at once; how
    /// it went comes back as `MpvEvent::CommandReply` with the same `id`.
    fn command_async(&self, id: u64, args: &[&str]) -> Result<(), PlayerError>;
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
    fn command_async(&self, id: u64, args: &[&str]) -> Result<(), PlayerError> {
        (**self).command_async(id, args)
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
    /// What `load` asked that mpv only takes once the file is open.
    on_loaded: Option<OnLoaded>,
    /// How many films `load` asked mpv for, how many it began to open, how many ended: mpv's
    /// events do not name their file, but they come in the order of the `loadfile`s. An event
    /// belongs to the latest film when its count has caught up with `loads`.
    loads: u64,
    started: u64,
    ended: u64,
    /// The `sub-add`s of the film being opened that mpv has not answered, by reply id.
    adding: Vec<u64>,
    /// The subtitle choice that waits for them: a late `auto` add must not undo it.
    then: Option<FirstSubtitle>,
    next_reply: u64,
}

/// Owned, as it waits for mpv: the addresses may carry a token, so nothing here is printed.
struct OnLoaded {
    subtitles: Vec<(String, String, Option<String>)>,
    audio: Option<i64>,
    subtitle: FirstSubtitle,
}

impl<C: MpvClient> MpvPlayer<C> {
    pub fn new(client: C) -> Self {
        Self {
            client,
            on_loaded: None,
            loads: 0,
            started: 0,
            ended: 0,
            adding: Vec::new(),
            then: None,
            next_reply: 0,
        }
    }

    /// The film is open: its external subtitles are queued (mpv downloads them while the window
    /// goes on), then the chosen tracks are set, the subtitle once the downloads are answered.
    /// Returns what mpv refused to queue.
    fn apply_on_loaded(&mut self) -> Vec<PlayerEvent> {
        let Some(on_loaded) = self.on_loaded.take() else {
            return Vec::new();
        };
        let mut failed = Vec::new();
        for (index, (url, title, language)) in on_loaded.subtitles.iter().enumerate() {
            let flag = if on_loaded.subtitle == FirstSubtitle::External(index) {
                "select"
            } else {
                "auto"
            };
            let language = language.as_deref().unwrap_or_default();
            self.next_reply += 1;
            let id = self.next_reply;
            match self
                .client
                .command_async(id, &["sub-add", url, flag, title, language])
            {
                Ok(()) => self.adding.push(id),
                Err(error) => failed.push(PlayerEvent::SubtitleFailed(error)),
            }
        }
        if let Some(id) = on_loaded.audio {
            self.select_audio(Some(id));
        }
        self.then = Some(on_loaded.subtitle);
        self.choose_subtitle_when_added();
        failed
    }

    /// Sets the subtitle `load` chose once no `sub-add` is left to answer. A chosen external
    /// one was selected by its own `sub-add`.
    fn choose_subtitle_when_added(&mut self) {
        if !self.adding.is_empty() {
            return;
        }
        match self.then.take() {
            Some(FirstSubtitle::Off) => self.select_subtitle(None),
            Some(FirstSubtitle::Track(id)) => self.select_subtitle(Some(id)),
            Some(FirstSubtitle::Default | FirstSubtitle::External(_)) | None => {}
        }
    }

    /// mpv answered a `sub-add`. Returns the refusal; an answer for a film that is gone is not
    /// one.
    fn subtitle_added(&mut self, id: u64, error: Option<String>) -> Option<PlayerEvent> {
        let position = self.adding.iter().position(|&adding| adding == id)?;
        self.adding.remove(position);
        self.choose_subtitle_when_added();
        error.map(|error| PlayerEvent::SubtitleFailed(PlayerError(error)))
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
    /// Its core answers each property asked once it is free, and opening a file keeps it busy.
    fn answers_slowly(&self) -> bool {
        true
    }

    fn load(&mut self, media: Media<'_>) -> Result<(), PlayerError> {
        let Media {
            stream,
            start,
            audio,
            subtitle,
            external_subtitles,
        } = media;
        let on_loaded = OnLoaded {
            subtitles: external_subtitles
                .iter()
                .map(|sub| {
                    (
                        sub.url.to_string(),
                        sub.title.to_string(),
                        sub.language.map(str::to_string),
                    )
                })
                .collect(),
            audio,
            subtitle,
        };
        self.client
            .command(&["change-list", "http-header-fields", "clr", ""])?;
        for (name, value) in stream.headers {
            // Appended one by one: a header's own commas are not taken for separators.
            let header = format!("{name}: {value}");
            self.client
                .command(&["change-list", "http-header-fields", "append", &header])?;
        }
        self.client.set("start", &start.to_string())?;
        self.client.command(&["loadfile", stream.url, "replace"])?;
        // Only a film mpv took replaces the one being opened.
        self.loads += 1;
        self.on_loaded = Some(on_loaded);
        self.adding.clear();
        self.then = None;
        Ok(())
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
            Seek::Rough(seconds) => (seconds, "absolute+keyframes"),
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

    fn set_subtitle_position(&mut self, percent: f64) {
        let _ = self.client.set("sub-pos", &percent.to_string());
    }

    fn set_speed(&mut self, speed: f64) {
        let _ = self.client.set("speed", &speed.to_string());
    }

    fn set_subtitle_delay(&mut self, seconds: f64) {
        let _ = self.client.set("sub-delay", &seconds.to_string());
    }

    fn set_aspect(&mut self, aspect: Aspect) {
        // Fill keeps the film's shape and crops it to the window: mpv's pan and scan.
        let (shape, crop) = match aspect {
            Aspect::Auto => ("-1", "0"),
            Aspect::Fill => ("-1", "1"),
            Aspect::Wide => ("16:9", "0"),
            Aspect::Standard => ("4:3", "0"),
            Aspect::Scope => ("2.35:1", "0"),
        };
        let _ = self.client.set("video-aspect-override", shape);
        let _ = self.client.set("panscan", crop);
    }

    fn info(&self) -> PlaybackInfo {
        let text = |name: &str| self.client.get(name).filter(|value| !value.is_empty());
        let whole = |name: &str| text(name)?.parse::<u64>().ok();
        let dropped = match (whole("frame-drop-count"), whole("decoder-frame-drop-count")) {
            (None, None) => None,
            (output, decoder) => Some(output.unwrap_or(0) + decoder.unwrap_or(0)),
        };
        PlaybackInfo {
            container: text("file-format"),
            video_codec: text("video-format"),
            audio_codec: text("audio-codec-name"),
            size: whole("width")
                .zip(whole("height"))
                .map(|(width, height)| (width as u32, height as u32)),
            video_bitrate: self.number("video-bitrate"),
            dropped_frames: dropped,
            hardware_decoder: text("hwdec-current").filter(|decoder| decoder != "no"),
        }
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
                    "vo-configured" => (self.client.get("vo-configured").as_deref() == Some("yes"))
                        .then_some(PlayerEvent::FirstFrame),
                    "paused-for-cache" => self
                        .client
                        .get("paused-for-cache")
                        .map(|waiting| PlayerEvent::Buffering(waiting == "yes")),
                    _ => Some(PlayerEvent::Tracks(
                        tracks::from_mpv(|name| self.client.get(name))
                            .into_iter()
                            .filter(|track| track.kind != TrackKind::Video)
                            .collect(),
                    )),
                },
                MpvEvent::StartFile => {
                    self.started += 1;
                    None
                }
                MpvEvent::FileLoaded if self.started == self.loads => {
                    events.extend(self.apply_on_loaded());
                    Some(PlayerEvent::Loaded)
                }
                MpvEvent::FileLoaded => None,
                MpvEvent::CommandReply { id, error } => self.subtitle_added(id, error),
                MpvEvent::EndFile(reason) => {
                    self.ended += 1;
                    match reason {
                        _ if self.ended != self.loads => None,
                        EndReason::Eof => Some(PlayerEvent::EndOfFile),
                        EndReason::Error(reason) => Some(PlayerEvent::Error(PlayerError(reason))),
                        EndReason::Stopped => None,
                    }
                }
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
    use crate::player::{ExternalSubtitle, Stream};

    /// An mpv that plays nothing: it keeps what it is told, answers from a table and hands out
    /// queued events.
    #[derive(Default)]
    struct FakeMpv {
        log: RefCell<Vec<Vec<String>>>,
        properties: RefCell<HashMap<String, String>>,
        events: RefCell<VecDeque<MpvEvent>>,
        /// Commands it refuses, by name.
        refused: RefCell<Vec<String>>,
        /// The commands the caller waited for, by name: the others were queued.
        synchronous: RefCell<Vec<String>>,
        /// The reply ids of the commands queued.
        queued: RefCell<Vec<u64>>,
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
            self.synchronous.borrow_mut().push(args[0].to_string());
            self.log
                .borrow_mut()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            if self.refused.borrow().iter().any(|name| name == args[0]) {
                return Err(PlayerError("error running command".into()));
            }
            Ok(())
        }
        fn command_async(&self, id: u64, args: &[&str]) -> Result<(), PlayerError> {
            self.log
                .borrow_mut()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            if self.refused.borrow().iter().any(|name| name == args[0]) {
                return Err(PlayerError("error running command".into()));
            }
            self.queued.borrow_mut().push(id);
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

    fn media<'a>(stream: Stream<'a>, start: f64) -> Media<'a> {
        Media {
            stream,
            start,
            audio: None,
            subtitle: FirstSubtitle::Default,
            external_subtitles: &[],
        }
    }

    const FILM: Stream<'static> = Stream {
        url: "http://media.example/v.mkv",
        headers: &[],
    };

    const SUBTITLES: [ExternalSubtitle<'static>; 2] = [
        ExternalSubtitle {
            url: "http://media.example/fr.srt?ApiKey=k",
            title: "Français",
            language: Some("fre"),
        },
        ExternalSubtitle {
            url: "http://media.example/en.srt?ApiKey=k",
            title: "English",
            language: Some("eng"),
        },
    ];

    #[test]
    fn the_loaded_file_is_reported() {
        let mpv = FakeMpv::with(&[], &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        let mut player = MpvPlayer::new(mpv);
        player.load(media(FILM, 0.)).unwrap();
        assert_eq!(player.events(), [PlayerEvent::Loaded]);
    }

    #[test]
    fn the_external_subtitles_and_the_chosen_tracks_wait_for_the_file_to_be_loaded() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player
            .load(Media {
                audio: Some(2),
                subtitle: FirstSubtitle::External(1),
                external_subtitles: &SUBTITLES,
                ..media(FILM, 0.)
            })
            .unwrap();
        let loaded = log(&player).len();
        player
            .client()
            .events
            .borrow_mut()
            .push_back(MpvEvent::StartFile);
        // mpv refuses `sub-add` before the file is open.
        assert_eq!(player.events(), []);
        assert_eq!(log(&player).len(), loaded);

        player
            .client()
            .events
            .borrow_mut()
            .push_back(MpvEvent::FileLoaded);
        assert_eq!(player.events(), [PlayerEvent::Loaded]);
        assert_eq!(
            log(&player)[loaded..],
            [
                line(&[
                    "sub-add",
                    "http://media.example/fr.srt?ApiKey=k",
                    "auto",
                    "Français",
                    "fre"
                ]),
                line(&[
                    "sub-add",
                    "http://media.example/en.srt?ApiKey=k",
                    "select",
                    "English",
                    "eng"
                ]),
                line(&["set", "aid", "2"]),
            ]
        );
    }

    #[test]
    fn a_chosen_embedded_subtitle_or_none_is_set_once_the_file_is_loaded() {
        for (subtitle, sid) in [(FirstSubtitle::Track(3), "3"), (FirstSubtitle::Off, "no")] {
            let mpv = FakeMpv::with(&[], &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
            let mut player = MpvPlayer::new(mpv);
            player
                .load(Media {
                    subtitle,
                    external_subtitles: &SUBTITLES[..1],
                    ..media(FILM, 0.)
                })
                .unwrap();
            player.events();
            // The added subtitles come first: a late `auto` add must not undo the choice.
            assert!(!log(&player).iter().any(|line| line[..2] == ["set", "sid"]));

            let id = player.client().queued.borrow()[0];
            push(&player, &[MpvEvent::CommandReply { id, error: None }]);
            assert_eq!(player.events(), []);
            assert_eq!(log(&player).last(), Some(&line(&["set", "sid", sid])));
        }
    }

    #[test]
    fn adding_external_subtitles_never_waits_for_their_download() {
        let mpv = FakeMpv::with(&[], &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        let mut player = MpvPlayer::new(mpv);
        player
            .load(Media {
                external_subtitles: &SUBTITLES,
                ..media(FILM, 0.)
            })
            .unwrap();
        assert_eq!(player.events(), [PlayerEvent::Loaded]);
        let client = player.client();
        assert!(
            !client
                .synchronous
                .borrow()
                .iter()
                .any(|name| name == "sub-add")
        );
        assert_eq!(client.queued.borrow().len(), 2);
    }

    #[test]
    fn a_refused_subtitle_is_reported_when_mpv_answers() {
        let mpv = FakeMpv::with(&[], &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        let mut player = MpvPlayer::new(mpv);
        player
            .load(Media {
                external_subtitles: &SUBTITLES,
                ..media(FILM, 0.)
            })
            .unwrap();
        assert_eq!(player.events(), [PlayerEvent::Loaded]);

        let ids = player.client().queued.borrow().clone();
        push(
            &player,
            &[
                MpvEvent::CommandReply {
                    id: ids[0],
                    error: Some("unrecognized file format".into()),
                },
                MpvEvent::CommandReply {
                    id: ids[1],
                    error: None,
                },
            ],
        );
        assert_eq!(
            player.events(),
            [PlayerEvent::SubtitleFailed(PlayerError(
                "unrecognized file format".into()
            ))]
        );
    }

    #[test]
    fn the_answer_for_a_replaced_film_changes_nothing() {
        let mpv = FakeMpv::with(&[], &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        let mut player = MpvPlayer::new(mpv);
        player
            .load(Media {
                subtitle: FirstSubtitle::Off,
                external_subtitles: &SUBTITLES[..1],
                ..media(FILM, 0.)
            })
            .unwrap();
        player.events();
        let id = player.client().queued.borrow()[0];

        player.load(media(FILM, 0.)).unwrap();
        push(
            &player,
            &[MpvEvent::CommandReply {
                id,
                error: Some("unrecognized file format".into()),
            }],
        );
        assert_eq!(player.events(), []);
        assert!(!log(&player).iter().any(|line| line[..2] == ["set", "sid"]));
    }

    #[test]
    fn what_is_applied_once_loaded_is_not_applied_again_on_the_next_file() {
        let mpv = FakeMpv::with(
            &[],
            &[
                MpvEvent::StartFile,
                MpvEvent::FileLoaded,
                MpvEvent::FileLoaded,
            ],
        );
        let mut player = MpvPlayer::new(mpv);
        player
            .load(Media {
                external_subtitles: &SUBTITLES[..1],
                ..media(FILM, 0.)
            })
            .unwrap();
        player.events();
        let added = log(&player)
            .iter()
            .filter(|line| line[0] == "sub-add")
            .count();
        assert_eq!(added, 1);
    }

    #[test]
    fn a_refused_external_subtitle_is_reported_and_the_film_goes_on() {
        let mpv = FakeMpv::with(&[], &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        mpv.refused.borrow_mut().push("sub-add".into());
        let mut player = MpvPlayer::new(mpv);
        player
            .load(Media {
                external_subtitles: &SUBTITLES[..1],
                ..media(FILM, 0.)
            })
            .unwrap();
        assert_eq!(
            player.events(),
            [
                PlayerEvent::SubtitleFailed(PlayerError("error running command".into())),
                PlayerEvent::Loaded,
            ]
        );
    }

    #[test]
    fn adding_a_subtitle_says_when_mpv_refuses_it() {
        let mpv = FakeMpv::default();
        mpv.refused.borrow_mut().push("sub-add".into());
        let mut player = MpvPlayer::new(mpv);
        assert!(
            player
                .add_subtitle("http://media.example/s.srt", "Français", None)
                .is_err()
        );
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
        player.load(media(stream, 754.5)).unwrap();
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
    fn a_rough_seek_lands_on_a_keyframe() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.seek(Seek::Rough(3600.));
        assert_eq!(
            log(&player),
            [line(&["seek", "3600", "absolute+keyframes"])]
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
    fn the_subtitle_position_sets_sub_pos() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.set_subtitle_position(85.);
        assert_eq!(log(&player), [line(&["set", "sub-pos", "85"])]);
    }

    #[test]
    fn the_speed_sets_the_speed_property() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.set_speed(1.25);
        assert_eq!(log(&player), [line(&["set", "speed", "1.25"])]);
    }

    #[test]
    fn the_subtitle_offset_sets_sub_delay() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.set_subtitle_delay(-0.3);
        assert_eq!(log(&player), [line(&["set", "sub-delay", "-0.3"])]);
    }

    #[test]
    fn each_aspect_sets_the_aspect_override_and_the_crop() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        for aspect in Aspect::ALL {
            player.set_aspect(aspect);
        }
        let set = |name: &str, value: &str| line(&["set", name, value]);
        assert_eq!(
            log(&player),
            [
                set("video-aspect-override", "-1"),
                set("panscan", "0"),
                set("video-aspect-override", "-1"),
                set("panscan", "1"),
                set("video-aspect-override", "16:9"),
                set("panscan", "0"),
                set("video-aspect-override", "4:3"),
                set("panscan", "0"),
                set("video-aspect-override", "2.35:1"),
                set("panscan", "0"),
            ]
        );
    }

    #[test]
    fn the_playback_info_is_read_from_mpv() {
        let mpv = FakeMpv::with(
            &[
                ("file-format", "matroska,webm"),
                ("video-format", "hevc"),
                ("audio-codec-name", "eac3"),
                ("width", "3840"),
                ("height", "2160"),
                ("video-bitrate", "18500000"),
                ("frame-drop-count", "2"),
                ("decoder-frame-drop-count", "1"),
                ("hwdec-current", "vaapi"),
            ],
            &[],
        );
        let player = MpvPlayer::new(mpv);
        assert_eq!(
            player.info(),
            PlaybackInfo {
                container: Some("matroska,webm".into()),
                video_codec: Some("hevc".into()),
                audio_codec: Some("eac3".into()),
                size: Some((3840, 2160)),
                video_bitrate: Some(18_500_000.),
                dropped_frames: Some(3),
                hardware_decoder: Some("vaapi".into()),
            }
        );
    }

    #[test]
    fn a_film_the_processor_decodes_has_no_hardware_decoder() {
        for said in ["no", ""] {
            let player = MpvPlayer::new(FakeMpv::with(&[("hwdec-current", said)], &[]));
            assert_eq!(player.info().hardware_decoder, None, "{said:?}");
        }
        let nothing = MpvPlayer::new(FakeMpv::default()).info();
        assert_eq!(nothing, PlaybackInfo::default());
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
    fn the_first_picture_is_told_once_the_video_output_is_configured() {
        let mpv = FakeMpv::with(&[("vo-configured", "no")], &[changed("vo-configured")]);
        let mut player = MpvPlayer::new(mpv);
        assert_eq!(player.events(), []);
        player
            .client()
            .properties
            .borrow_mut()
            .insert("vo-configured".into(), "yes".into());
        push(&player, &[changed("vo-configured")]);
        assert_eq!(player.events(), [PlayerEvent::FirstFrame]);
    }

    #[test]
    fn a_cache_stall_is_told_by_paused_for_cache() {
        let mpv = FakeMpv::with(
            &[("paused-for-cache", "yes")],
            &[changed("paused-for-cache")],
        );
        let mut player = MpvPlayer::new(mpv);
        assert_eq!(player.events(), [PlayerEvent::Buffering(true)]);
        player
            .client()
            .properties
            .borrow_mut()
            .insert("paused-for-cache".into(), "no".into());
        push(&player, &[changed("paused-for-cache")]);
        assert_eq!(player.events(), [PlayerEvent::Buffering(false)]);
    }

    fn push(player: &MpvPlayer<FakeMpv>, events: &[MpvEvent]) {
        player
            .client()
            .events
            .borrow_mut()
            .extend(events.iter().cloned());
    }

    #[test]
    fn the_file_loaded_of_a_replaced_film_applies_nothing_to_the_next_one() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player
            .load(Media {
                audio: Some(1),
                external_subtitles: &SUBTITLES[..1],
                ..media(FILM, 0.)
            })
            .unwrap();
        push(&player, &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        // A's file is open and its event is waiting when the next film is asked for.
        player
            .load(Media {
                audio: Some(2),
                subtitle: FirstSubtitle::Track(5),
                external_subtitles: &SUBTITLES[1..],
                ..media(FILM, 0.)
            })
            .unwrap();
        let before = log(&player).len();
        push(
            &player,
            &[MpvEvent::EndFile(EndReason::Stopped), MpvEvent::StartFile],
        );
        assert_eq!(player.events(), []);
        assert_eq!(log(&player).len(), before, "nothing is applied to A");

        push(&player, &[MpvEvent::FileLoaded]);
        assert_eq!(player.events(), [PlayerEvent::Loaded]);
        let applied = &log(&player)[before..];
        assert!(
            applied
                .iter()
                .any(|line| line[0] == "sub-add" && line[1].contains("en.srt"))
        );
        assert!(
            !applied
                .iter()
                .any(|line| line.iter().any(|part| part.contains("fr.srt")))
        );
        assert!(applied.contains(&line(&["set", "aid", "2"])));
        assert!(!applied.contains(&line(&["set", "aid", "1"])));
        let id = player.client().queued.borrow()[0];
        push(&player, &[MpvEvent::CommandReply { id, error: None }]);
        assert_eq!(player.events(), []);
        assert_eq!(log(&player).last(), Some(&line(&["set", "sid", "5"])));
    }

    #[test]
    fn the_end_of_a_replaced_film_is_not_the_end_of_the_next_one() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player.load(media(FILM, 0.)).unwrap();
        player.load(media(FILM, 0.)).unwrap();
        push(
            &player,
            &[
                MpvEvent::StartFile,
                MpvEvent::EndFile(EndReason::Error("A failed".into())),
                MpvEvent::StartFile,
            ],
        );
        assert_eq!(player.events(), []);

        push(&player, &[MpvEvent::EndFile(EndReason::Eof)]);
        assert_eq!(player.events(), [PlayerEvent::EndOfFile]);
    }

    #[test]
    fn a_load_mpv_refuses_leaves_the_film_being_opened_its_file_loaded() {
        let mut player = MpvPlayer::new(FakeMpv::default());
        player
            .load(Media {
                external_subtitles: &SUBTITLES[..1],
                ..media(FILM, 0.)
            })
            .unwrap();
        player.client().refused.borrow_mut().push("loadfile".into());
        assert!(player.load(media(FILM, 0.)).is_err());
        push(&player, &[MpvEvent::StartFile, MpvEvent::FileLoaded]);
        assert_eq!(player.events(), [PlayerEvent::Loaded]);
        assert!(log(&player).iter().any(|line| line[0] == "sub-add"));
    }

    #[test]
    fn the_end_of_the_film_is_reported() {
        let mpv = FakeMpv::with(
            &[],
            &[MpvEvent::StartFile, MpvEvent::EndFile(EndReason::Eof)],
        );
        let mut player = MpvPlayer::new(mpv);
        player.load(media(FILM, 0.)).unwrap();
        assert_eq!(player.events(), [PlayerEvent::EndOfFile]);
    }

    #[test]
    fn a_film_that_fails_is_reported_with_the_reason() {
        let mpv = FakeMpv::with(
            &[],
            &[
                MpvEvent::StartFile,
                MpvEvent::EndFile(EndReason::Stopped),
                MpvEvent::StartFile,
                MpvEvent::EndFile(EndReason::Error("loading failed".into())),
            ],
        );
        let mut player = MpvPlayer::new(mpv);
        player.load(media(FILM, 0.)).unwrap();
        player.load(media(FILM, 0.)).unwrap();
        assert_eq!(
            player.events(),
            [PlayerEvent::Error(PlayerError("loading failed".into()))]
        );
    }
}
