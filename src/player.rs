//! The player port: what the rest of Bobine asks of a player, whatever plays the film. libmpv
//! implements it (`mpv`); a fake implements it in the tests. The video surface it draws into is
//! the display port of `crate::video`.

pub mod controls;
pub mod libmpv;
pub mod mpv;
pub mod segments;
pub mod session;
pub mod time;
pub mod tracks;

use std::fmt;

/// What to play: an address and the HTTP headers the server wants with it. Both may carry an
/// access token, so neither is ever printed: `Debug` names the header fields only.
#[derive(Clone, Copy)]
pub struct Stream<'a> {
    pub url: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
}

impl fmt::Debug for Stream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(name, _)| *name).collect();
        f.debug_struct("Stream")
            .field("url", &"<redacted>")
            .field("headers", &names)
            .finish()
    }
}

/// A subtitle file beside the film. Its address may carry a token: it has no `Debug`.
#[derive(Clone, Copy)]
pub struct ExternalSubtitle<'a> {
    pub url: &'a str,
    /// What the panel may call it; never the address.
    pub title: &'a str,
    /// `fre`, `eng`…
    pub language: Option<&'a str>,
}

/// The subtitle shown when the film starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "chosen by the film page once the server's streams reach it"
)]
pub enum FirstSubtitle {
    /// The file's own choice.
    Default,
    Off,
    /// An embedded track, by its id.
    Track(i64),
    /// One of the media's external subtitles, by its index.
    External(usize),
}

/// A film to play and how it starts: where, with which audio and subtitles. mpv only takes the
/// tracks once the file is open, so the player applies them then, and says `Loaded`.
#[derive(Clone, Copy)]
pub struct Media<'a> {
    pub stream: Stream<'a>,
    /// Seconds from the start.
    pub start: f64,
    /// An audio track by its id; `None` keeps the file's default.
    pub audio: Option<i64>,
    pub subtitle: FirstSubtitle,
    pub external_subtitles: &'a [ExternalSubtitle<'a>],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seek {
    /// Seconds from where the film is, forward or back.
    By(f64),
    /// Seconds from the start.
    To(f64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub id: i64,
    pub kind: TrackKind,
    pub title: Option<String>,
    /// As the file says it: `fre`, `fr`, `eng`…
    pub language: Option<String>,
    pub codec: Option<String>,
    pub channels: Option<u32>,
    pub forced: bool,
    /// A subtitle file loaded beside the film.
    pub external: bool,
    pub selected: bool,
}

/// Why a film does not play. The text is the player's error code put in words, never its log:
/// a log line can hold the address of the stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerError(pub String);

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayerEvent {
    /// The file is open and what `load` chose is applied: the film is playing.
    Loaded,
    /// Seconds from the start.
    Position(f64),
    Duration(f64),
    Paused(bool),
    /// The audio and subtitle tracks changed, or the one selected did.
    Tracks(Vec<Track>),
    EndOfFile,
    Error(PlayerError),
    /// An external subtitle of the media was refused; the film plays on without it.
    SubtitleFailed(PlayerError),
}

pub trait Player {
    /// Starts `media`, replacing what was playing.
    fn load(&mut self, media: Media<'_>) -> Result<(), PlayerError>;
    fn set_paused(&mut self, paused: bool);
    fn seek(&mut self, seek: Seek);
    /// 0 to 100.
    #[allow(
        dead_code,
        reason = "part of the port; the controls have no volume yet"
    )]
    fn set_volume(&mut self, percent: f64);
    #[allow(
        dead_code,
        reason = "part of the port; the controls have no volume yet"
    )]
    fn set_muted(&mut self, muted: bool);
    /// `None` silences the film.
    fn select_audio(&mut self, id: Option<i64>);
    /// `None` hides the subtitles.
    fn select_subtitle(&mut self, id: Option<i64>);
    /// Adds a subtitle file by its address (the stream's headers go with it) and shows it, named
    /// `title` in `language` (`fre`, `eng`…): the address, which may carry a token, never names
    /// it.
    #[allow(
        dead_code,
        reason = "part of the port; the server's external subtitles come later"
    )]
    fn add_subtitle(
        &mut self,
        url: &str,
        title: &str,
        language: Option<&str>,
    ) -> Result<(), PlayerError>;
    /// Moves the subtitles up: `percent` of the window's height from its top, 100 being their
    /// own place (mpv's `sub-pos`).
    fn set_subtitle_position(&mut self, percent: f64);
    /// What happened since the last call, oldest first. Never waits.
    fn events(&mut self) -> Vec<PlayerEvent>;
}

#[cfg(test)]
pub mod fake {
    //! A player that plays nothing: it records what it is asked and hands out the events a test
    //! queued.

    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    pub enum Call {
        Load {
            url: String,
            start: f64,
            audio: Option<i64>,
            subtitle: FirstSubtitle,
            /// Title and language of each external subtitle.
            externals: Vec<(String, Option<String>)>,
        },
        Paused(bool),
        Seek(Seek),
        Volume(f64),
        Muted(bool),
        Audio(Option<i64>),
        Subtitle(Option<i64>),
        AddSubtitle(String),
        SubtitlePosition(f64),
    }

    #[derive(Default)]
    pub struct FakePlayer {
        pub calls: std::rc::Rc<std::cell::RefCell<Vec<Call>>>,
        pub queued: std::rc::Rc<std::cell::RefCell<Vec<PlayerEvent>>>,
    }

    impl FakePlayer {
        fn record(&self, call: Call) {
            self.calls.borrow_mut().push(call);
        }
    }

    impl Player for FakePlayer {
        fn load(&mut self, media: Media<'_>) -> Result<(), PlayerError> {
            self.record(Call::Load {
                url: media.stream.url.to_string(),
                start: media.start,
                audio: media.audio,
                subtitle: media.subtitle,
                externals: media
                    .external_subtitles
                    .iter()
                    .map(|s| (s.title.to_string(), s.language.map(str::to_string)))
                    .collect(),
            });
            Ok(())
        }
        fn set_paused(&mut self, paused: bool) {
            self.record(Call::Paused(paused));
        }
        fn seek(&mut self, seek: Seek) {
            self.record(Call::Seek(seek));
        }
        fn set_volume(&mut self, percent: f64) {
            self.record(Call::Volume(percent));
        }
        fn set_muted(&mut self, muted: bool) {
            self.record(Call::Muted(muted));
        }
        fn select_audio(&mut self, id: Option<i64>) {
            self.record(Call::Audio(id));
        }
        fn select_subtitle(&mut self, id: Option<i64>) {
            self.record(Call::Subtitle(id));
        }
        fn add_subtitle(&mut self, url: &str, _: &str, _: Option<&str>) -> Result<(), PlayerError> {
            self.record(Call::AddSubtitle(url.to_string()));
            Ok(())
        }
        fn set_subtitle_position(&mut self, percent: f64) {
            self.record(Call::SubtitlePosition(percent));
        }
        fn events(&mut self) -> Vec<PlayerEvent> {
            std::mem::take(&mut *self.queued.borrow_mut())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_never_prints_its_address_nor_its_header_values() {
        let stream = Stream {
            url: "http://media.example/Videos/1/stream?api_key=SECRET",
            headers: &[("Authorization", "MediaBrowser Token=\"SECRET\"")],
        };
        let printed = format!("{stream:?}");
        assert!(!printed.contains("SECRET"), "{printed}");
        assert!(printed.contains("Authorization"));
    }
}
