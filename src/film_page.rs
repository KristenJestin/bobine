//! What the film page shows and what its buttons start, apart from the window: the labels, the
//! version and tracks chosen with its chips, and the plan the player gets from them.

use std::time::Duration;

use bobine::provider::{
    AudioTrack, Film, FilmDetail, ItemId, MediaServer, MediaSource, MediaSourceId, Playback,
    ProviderError, SubtitleTrack, VideoRange,
};

use crate::player::{self, FirstSubtitle, Media, Stream, Track, TrackKind, tracks};

/// The version and the tracks the chips show. Tracks are named by their index in the file, as
/// the server names them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    pub version: usize,
    pub audio: Option<i32>,
    /// `None`: no subtitles.
    pub subtitle: Option<i32>,
}

impl Choices {
    /// The first version, with its default audio track and its default subtitle, if any.
    pub fn new(detail: &FilmDetail) -> Self {
        let mut choices = Self {
            version: 0,
            audio: None,
            subtitle: None,
        };
        choices.set_version(detail, 0);
        choices
    }

    /// Another version has its own tracks: they go back to its defaults.
    pub fn set_version(&mut self, detail: &FilmDetail, version: usize) {
        let Some(source) = detail.versions.get(version) else {
            return;
        };
        self.version = version;
        self.audio = source
            .audio
            .iter()
            .find(|track| track.default)
            .or(source.audio.first())
            .map(|track| track.index);
        self.subtitle = source
            .subtitles
            .iter()
            .find(|track| track.default)
            .map(|track| track.index);
    }

    pub fn source<'a>(&self, detail: &'a FilmDetail) -> Option<&'a MediaSource> {
        detail.versions.get(self.version)
    }
}

/// Where the film starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum From {
    /// Where the viewer stopped, or the beginning when they never started it.
    Resume,
    Beginning,
}

/// The subtitle the player shows first, as mpv knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstShown {
    Off,
    /// mpv numbers the embedded tracks of each kind from 1, in the order of the file.
    Embedded(i64),
    /// An external file, by the server's index.
    External(i32),
}

/// What a play button starts: the version, where, and with which tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub film: ItemId,
    pub title: String,
    pub source: MediaSourceId,
    pub start: Duration,
    /// The server's indexes, for its reports.
    pub audio_index: Option<i32>,
    pub subtitle_index: Option<i32>,
    audio: Option<i64>,
    subtitle: FirstShown,
    /// The external subtitles of the version: index, title, language.
    externals: Vec<(i32, String, Option<String>)>,
}

/// The plan of a play button, `None` when the film has no version to play.
pub fn plan(detail: &FilmDetail, choices: &Choices, from: From) -> Option<Plan> {
    let source = choices.source(detail)?;
    let audio = choices.audio.and_then(|index| {
        let position = source.audio.iter().position(|t| t.index == index)?;
        Some(position as i64 + 1)
    });
    let subtitle = match choices
        .subtitle
        .and_then(|index| source.subtitles.iter().find(|t| t.index == index))
    {
        None => FirstShown::Off,
        Some(track) if track.external => FirstShown::External(track.index),
        Some(track) => {
            let embedded = source.subtitles.iter().filter(|t| !t.external);
            let position = embedded.take_while(|t| t.index != track.index).count();
            FirstShown::Embedded(position as i64 + 1)
        }
    };
    let start = match from {
        From::Resume => detail.film.resume.unwrap_or_default(),
        From::Beginning => Duration::ZERO,
    };
    Some(Plan {
        film: detail.film.id.clone(),
        title: detail.film.title.clone(),
        source: source.id.clone(),
        start,
        audio_index: choices.audio,
        subtitle_index: choices.subtitle,
        audio,
        subtitle,
        externals: source
            .subtitles
            .iter()
            .filter(|t| t.external)
            .map(|t| (t.index, subtitle_label(Some(t)), t.language.clone()))
            .collect(),
    })
}

impl Plan {
    /// The external subtitles the server gave, named from the version's tracks: their addresses
    /// carry the token and never name them.
    pub fn externals<'a>(&'a self, playback: &'a Playback) -> Vec<player::ExternalSubtitle<'a>> {
        playback
            .external_subtitles
            .iter()
            .map(|external| {
                let known = self.externals.iter().find(|(i, _, _)| *i == external.index);
                player::ExternalSubtitle {
                    url: external.url.expose_for_player(),
                    title: known.map_or("Sous-titres", |(_, title, _)| title.as_str()),
                    language: known.and_then(|(_, _, language)| language.as_deref()),
                }
            })
            .collect()
    }

    /// What the player loads. `externals` comes from [`Plan::externals`] on the same playback.
    pub fn media<'a>(
        &self,
        playback: &'a Playback,
        externals: &'a [player::ExternalSubtitle<'a>],
    ) -> Media<'a> {
        let subtitle = match self.subtitle {
            FirstShown::Off => FirstSubtitle::Off,
            FirstShown::Embedded(id) => FirstSubtitle::Track(id),
            FirstShown::External(index) => playback
                .external_subtitles
                .iter()
                .position(|external| external.index == index)
                .map_or(FirstSubtitle::Off, FirstSubtitle::External),
        };
        Media {
            stream: Stream {
                url: playback.stream.expose_for_player(),
                headers: &[],
            },
            start: self.start.as_secs_f64(),
            audio: self.audio,
            subtitle,
            external_subtitles: externals,
        }
    }
}

/// "✓ Vu": marks the film watched on the server, or no longer watched. Blocks on the server;
/// gives the film as the page shows it now.
pub fn toggle_watched(server: &dyn MediaServer, film: &Film) -> Result<Film, ProviderError> {
    let watched = !film.watched;
    server.set_watched(&film.id, watched)?;
    let mut film = film.clone();
    film.watched = watched;
    if watched {
        // Like the server: a watched film has no resume point.
        film.resume = None;
    }
    Ok(film)
}

/// `2 h 11`, `47 min`, `42 s`: the short times of the pages.
pub fn short_duration(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => format!("{} s", duration.as_secs()),
        (0, minutes) => format!("{minutes} min"),
        (hours, minutes) => format!("{hours} h {minutes:02}"),
    }
}

/// `▶ Reprendre à 1 h 12` once started, else `▶ Lecture`.
pub fn play_label(film: &Film) -> String {
    match film.resume {
        Some(resume) => format!("▶ Reprendre à {}", short_duration(resume)),
        None => "▶ Lecture".to_string(),
    }
}

/// How far the viewer went, 0 to 1, when they started it.
pub fn progress(film: &Film) -> Option<f32> {
    let resume = film.resume?.as_secs_f32();
    let runtime = film.runtime?.as_secs_f32();
    (runtime > 0.).then(|| (resume / runtime).clamp(0., 1.))
}

/// `reste 47 min`, for a film started.
pub fn time_left(film: &Film) -> Option<String> {
    let left = film.runtime?.checked_sub(film.resume?)?;
    Some(format!("reste {}", short_duration(left)))
}

/// `★ 8.1`
pub fn rating(film: &Film) -> Option<String> {
    film.community_rating.map(|rating| format!("★ {rating:.1}"))
}

/// The line under the title: year, rating, runtime, genres, age rating.
pub fn meta(detail: &FilmDetail) -> Vec<String> {
    let film = &detail.film;
    let mut parts: Vec<String> = Vec::new();
    parts.extend(film.year.map(|year| year.to_string()));
    parts.extend(rating(film));
    parts.extend(film.runtime.map(short_duration));
    if !detail.genres.is_empty() {
        parts.push(detail.genres.join(", "));
    }
    parts.extend(detail.official_rating.clone());
    parts
}

/// `4K HDR10`, `1080p`: what tells two versions apart.
pub fn version_label(source: &MediaSource) -> String {
    let Some(video) = &source.video else {
        return source.name.clone();
    };
    let width = video.width.unwrap_or(0);
    let height = video.height.unwrap_or(0);
    let resolution = match (width, height) {
        (w, h) if w >= 3200 || h >= 2000 => "4K",
        (w, h) if w >= 1800 || h >= 1000 => "1080p",
        (w, h) if w >= 1200 || h >= 700 => "720p",
        (0, 0) => return source.name.clone(),
        _ => "SD",
    };
    let range = match video.range {
        VideoRange::Hdr10 => " HDR10",
        VideoRange::Hdr10Plus => " HDR10+",
        VideoRange::Hlg => " HLG",
        VideoRange::DolbyVision => " Dolby Vision",
        VideoRange::Sdr | VideoRange::Unknown => "",
    };
    format!("{resolution}{range}")
}

/// `Français · E-AC-3 5.1`, as the player's panel names it.
pub fn audio_label(track: &AudioTrack) -> String {
    tracks::label(&Track {
        id: i64::from(track.index),
        kind: TrackKind::Audio,
        title: track.title.clone(),
        language: track.language.clone(),
        codec: track.codec.clone(),
        channels: track.channels,
        forced: false,
        external: false,
        selected: false,
    })
}

/// `Aucun`, `Français · SRT · forcés`, `Anglais · SRT · fichier externe`.
pub fn subtitle_label(track: Option<&SubtitleTrack>) -> String {
    let Some(track) = track else {
        return "Aucun".to_string();
    };
    tracks::label(&Track {
        id: i64::from(track.index),
        kind: TrackKind::Subtitle,
        title: track.title.clone(),
        language: track.language.clone(),
        codec: track.codec.clone(),
        channels: None,
        forced: track.forced,
        external: track.external,
        selected: false,
    })
}

/// What a person's disc shows without a photo: the first letter of the first and the last
/// word of the name, `PM` for Paul Mercier.
pub fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next();
    let last = words.next_back();
    first
        .into_iter()
        .chain(last)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FakeServer, PLAY_SESSION};
    use crate::player::Player;
    use crate::player::fake::{Call, FakePlayer};

    fn detail() -> FilmDetail {
        FakeServer::new(3, false)
            .film(&ItemId(FILM.into()))
            .unwrap()
    }

    fn load(plan: &Plan) -> Call {
        let server = FakeServer::new(3, false);
        let playback = server.play(&plan.film, &plan.source).unwrap();
        assert_eq!(playback.session.0, PLAY_SESSION);
        let externals = plan.externals(&playback);
        let mut player = FakePlayer::default();
        player.load(plan.media(&playback, &externals)).unwrap();
        player.calls.borrow()[0].clone()
    }

    #[test]
    fn resume_starts_where_the_viewer_stopped() {
        let detail = detail();
        let plan = plan(&detail, &Choices::new(&detail), From::Resume).unwrap();
        assert_eq!(plan.start, Duration::from_secs(72 * 60));
        let Call::Load { start, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(start, 72. * 60.);
    }

    #[test]
    fn from_the_beginning_starts_at_zero() {
        let detail = detail();
        let plan = plan(&detail, &Choices::new(&detail), From::Beginning).unwrap();
        let Call::Load { start, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(start, 0.);
    }

    #[test]
    fn a_film_never_started_resumes_at_the_beginning() {
        let mut detail = detail();
        detail.film.resume = None;
        let plan = plan(&detail, &Choices::new(&detail), From::Resume).unwrap();
        assert_eq!(plan.start, Duration::ZERO);
    }

    #[test]
    fn the_defaults_are_the_first_version_its_default_audio_and_no_subtitles() {
        let detail = detail();
        assert_eq!(
            Choices::new(&detail),
            Choices {
                version: 0,
                audio: Some(1),
                subtitle: None
            }
        );
    }

    #[test]
    fn tracks_chosen_on_the_detail_page_reach_the_player() {
        let detail = detail();
        let mut choices = Choices::new(&detail);
        // The English TrueHD track, the second audio track of the file; the full French ASS,
        // the second embedded subtitle.
        choices.audio = Some(2);
        choices.subtitle = Some(4);
        let plan = plan(&detail, &choices, From::Resume).unwrap();
        let Call::Load {
            audio,
            subtitle,
            externals,
            ..
        } = load(&plan)
        else {
            panic!("not a load");
        };
        assert_eq!(audio, Some(2));
        assert_eq!(subtitle, FirstSubtitle::Track(2));
        assert_eq!(
            externals,
            [(
                "Anglais · SRT · fichier externe".to_string(),
                Some("eng".to_string())
            )]
        );
        assert_eq!((plan.audio_index, plan.subtitle_index), (Some(2), Some(4)));
    }

    #[test]
    fn an_external_subtitle_chosen_on_the_detail_page_is_shown_first() {
        let detail = detail();
        let mut choices = Choices::new(&detail);
        choices.subtitle = Some(5);
        let plan = plan(&detail, &choices, From::Resume).unwrap();
        let Call::Load { subtitle, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(subtitle, FirstSubtitle::External(0));
    }

    #[test]
    fn no_subtitles_chosen_turns_them_off() {
        let detail = detail();
        let plan = plan(&detail, &Choices::new(&detail), From::Resume).unwrap();
        let Call::Load { subtitle, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(subtitle, FirstSubtitle::Off);
    }

    #[test]
    fn the_version_chosen_is_the_one_played_with_its_own_tracks() {
        let detail = detail();
        let mut choices = Choices::new(&detail);
        choices.subtitle = Some(5);
        choices.set_version(&detail, 1);
        assert_eq!(choices.audio, Some(1));
        assert_eq!(choices.subtitle, None);
        let plan = plan(&detail, &choices, From::Resume).unwrap();
        assert_eq!(plan.source, MediaSourceId("source-1080".into()));
    }

    #[test]
    fn the_watched_toggle_marks_and_unmarks_the_film_on_the_server() {
        let server = FakeServer::new(3, false);
        let film = server.film(&ItemId(FILM.into())).unwrap().film;
        let watched = toggle_watched(&server, &film).unwrap();
        assert!(watched.watched);
        assert_eq!(watched.resume, None);
        assert_eq!(play_label(&watched), "▶ Lecture");
        let unwatched = toggle_watched(&server, &watched).unwrap();
        assert!(!unwatched.watched);
        assert_eq!(
            server.state.lock().unwrap().record.watched,
            [(film.id.clone(), true), (film.id.clone(), false)]
        );
    }

    #[test]
    fn the_play_button_says_where_it_resumes() {
        let detail = detail();
        assert_eq!(play_label(&detail.film), "▶ Reprendre à 1 h 12");
        let mut film = detail.film.clone();
        film.resume = None;
        assert_eq!(play_label(&film), "▶ Lecture");
        film.resume = Some(Duration::from_secs(42));
        assert_eq!(play_label(&film), "▶ Reprendre à 42 s");
    }

    #[test]
    fn the_page_says_year_rating_runtime_genres_and_age() {
        assert_eq!(
            meta(&detail()),
            ["2025", "★ 6.0", "2 h 11", "Drame, Aventure", "12+"]
        );
    }

    #[test]
    fn the_chips_name_the_version_and_the_tracks() {
        let versions = crate::fake::versions();
        assert_eq!(version_label(&versions[0]), "4K HDR10");
        assert_eq!(version_label(&versions[1]), "1080p");
        assert_eq!(audio_label(&versions[0].audio[0]), "Français · E-AC-3 5.1");
        assert_eq!(subtitle_label(None), "Aucun");
        assert_eq!(
            subtitle_label(Some(&versions[0].subtitles[0])),
            "Français · SRT · forcés"
        );
    }

    #[test]
    fn a_started_film_says_what_is_left_and_how_far() {
        let film = detail().film;
        assert_eq!(time_left(&film).as_deref(), Some("reste 59 min"));
        let progress = progress(&film).unwrap();
        assert!((progress - 72. / 131.).abs() < 1e-4, "{progress}");
    }

    #[test]
    fn a_person_without_a_photo_shows_their_initials() {
        assert_eq!(initials("Paul Mercier"), "PM");
        assert_eq!(initials("Zoé"), "Z");
        assert_eq!(initials("élise de la Fabre"), "ÉF");
        assert_eq!(initials("  "), "");
    }
}
