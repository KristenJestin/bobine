//! What the film page shows and what its buttons start, apart from the window: the labels, the
//! version and tracks chosen with its chips, and the plan the player gets from them.

use std::time::Duration;

use bobine::provider::{
    AudioTrack, Film, FilmDetail, ItemId, MediaServer, MediaSource, MediaSourceId, Playback,
    ProviderError, SubtitleTrack, TranscodeRequest, VideoRange,
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
    /// The first version, with the tracks the server starts it with.
    pub fn new(versions: &[MediaSource]) -> Self {
        let mut choices = Self {
            version: 0,
            audio: None,
            subtitle: None,
        };
        choices.set_version(versions, 0);
        choices
    }

    /// Another version has its own tracks: those the server chose for the user from their
    /// settings, else the file's default audio track (or the first) and its default subtitle.
    pub fn set_version(&mut self, versions: &[MediaSource], version: usize) {
        let Some(source) = versions.get(version) else {
            return;
        };
        self.version = version;
        let audio = |index| source.audio.iter().find(|track| track.index == index);
        let subtitle = |index| source.subtitles.iter().find(|track| track.index == index);
        self.audio = source
            .default_audio
            .and_then(audio)
            .or_else(|| source.audio.iter().find(|track| track.default))
            .or(source.audio.first())
            .map(|track| track.index);
        self.subtitle = match source.default_subtitle {
            Some(None) => None,
            Some(Some(index)) if subtitle(index).is_some() => Some(index),
            _ => source
                .subtitles
                .iter()
                .find(|track| track.default)
                .map(|track| track.index),
        };
    }

    /// An audio track picked on the chip: it wins for this playback.
    pub fn choose_audio(&mut self, track: Option<i32>) {
        self.audio = track;
    }

    /// A subtitle track picked on the chip, or none.
    pub fn choose_subtitle(&mut self, track: Option<i32>) {
        self.subtitle = track;
    }

    pub fn source<'a>(&self, versions: &'a [MediaSource]) -> Option<&'a MediaSource> {
        versions.get(self.version)
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
    /// The subtitles of the version that may come as files: index, title, language. An embedded
    /// one does in a conversion.
    externals: Vec<(i32, String, Option<String>)>,
}

/// The plan of a play button for a film or an episode and its versions, `None` when it has no
/// version to play.
pub fn plan(film: &Film, versions: &[MediaSource], choices: &Choices, from: From) -> Option<Plan> {
    let source = choices.source(versions)?;
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
        From::Resume => film.resume.unwrap_or_default(),
        From::Beginning => Duration::ZERO,
    };
    Some(Plan {
        film: film.id.clone(),
        title: film.title.clone(),
        source: source.id.clone(),
        start,
        audio_index: choices.audio,
        subtitle_index: choices.subtitle,
        audio,
        subtitle,
        externals: source
            .subtitles
            .iter()
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
                    title: known.map_or("Subtitles", |(_, title, _)| title.as_str()),
                    language: known.and_then(|(_, _, language)| language.as_deref()),
                }
            })
            .collect()
    }

    /// What the server is asked to convert when the version does not play as it is: the tracks
    /// chosen, from `from`. An external subtitle stays a file the player adds.
    pub fn transcode_request(&self, from: Duration) -> TranscodeRequest {
        TranscodeRequest {
            film: self.film.clone(),
            source: self.source.clone(),
            start: from,
            audio_track: self.audio_index,
            subtitle_track: self
                .subtitle_index
                .filter(|_| matches!(self.subtitle, FirstShown::Embedded(_))),
        }
    }

    /// The plan of the converted stream `opened`, from `from`: it holds only the audio track
    /// asked, so the player keeps its default. An embedded subtitle chosen is the file the
    /// server extracted, or else drawn into the picture.
    pub fn converted(&self, from: Duration, opened: &Playback) -> Plan {
        let subtitle = match (self.subtitle, self.subtitle_index) {
            (FirstShown::Embedded(_), Some(index))
                if opened.external_subtitles.iter().any(|e| e.index == index) =>
            {
                FirstShown::External(index)
            }
            (FirstShown::Embedded(_), _) => FirstShown::Off,
            (other, _) => other,
        };
        Plan {
            start: from,
            audio: None,
            subtitle,
            ..self.clone()
        }
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

/// `2h 11m`, `47m`, `42s`: the short times of the pages.
pub fn short_duration(duration: Duration) -> String {
    let minutes = duration.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => format!("{}s", duration.as_secs()),
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

/// `Resume from 1h 12m` once started, else `Play`, after a play icon.
pub fn play_label(film: &Film) -> String {
    match film.resume {
        Some(resume) => format!("Resume from {}", short_duration(resume)),
        None => "Play".to_string(),
    }
}

/// How far the viewer went, 0 to 1, when they started it.
pub fn progress(film: &Film) -> Option<f32> {
    let resume = film.resume?.as_secs_f32();
    let runtime = film.runtime?.as_secs_f32();
    (runtime > 0.).then(|| (resume / runtime).clamp(0., 1.))
}

/// `47m left`, for a film started.
pub fn time_left(film: &Film) -> Option<String> {
    let left = film.runtime?.checked_sub(film.resume?)?;
    Some(format!("{} left", short_duration(left)))
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

/// `French · E-AC-3 5.1`, as the player's panel names it.
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

/// `None`, `French · SRT · forced`, `English · SRT · external file`.
pub fn subtitle_label(track: Option<&SubtitleTrack>) -> String {
    let Some(track) = track else {
        return "None".to_string();
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
        let plan = plan(
            &detail.film,
            &detail.versions,
            &Choices::new(&detail.versions),
            From::Resume,
        )
        .unwrap();
        assert_eq!(plan.start, Duration::from_secs(72 * 60));
        let Call::Load { start, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(start, 72. * 60.);
    }

    #[test]
    fn from_the_beginning_starts_at_zero() {
        let detail = detail();
        let plan = plan(
            &detail.film,
            &detail.versions,
            &Choices::new(&detail.versions),
            From::Beginning,
        )
        .unwrap();
        let Call::Load { start, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(start, 0.);
    }

    #[test]
    fn a_film_never_started_resumes_at_the_beginning() {
        let mut detail = detail();
        detail.film.resume = None;
        let plan = plan(
            &detail.film,
            &detail.versions,
            &Choices::new(&detail.versions),
            From::Resume,
        )
        .unwrap();
        assert_eq!(plan.start, Duration::ZERO);
    }

    fn shown(choices: &Choices) -> (usize, Option<i32>, Option<i32>) {
        (choices.version, choices.audio, choices.subtitle)
    }

    #[test]
    fn the_servers_indexes_are_used() {
        let detail = detail();
        // The fake server starts the first version with English audio and the full French
        // subtitles.
        assert_eq!(shown(&Choices::new(&detail.versions)), (0, Some(2), Some(4)));
    }

    #[test]
    fn a_version_change_takes_its_own_indexes() {
        let detail = detail();
        let mut choices = Choices::new(&detail.versions);
        choices.set_version(&detail.versions, 1);
        assert_eq!(shown(&choices), (1, Some(1), Some(2)));
    }

    #[test]
    fn a_missing_index_falls_back_to_the_default_audio_and_no_subtitles() {
        let mut detail = detail();
        detail.versions[0].default_audio = None;
        detail.versions[0].default_subtitle = None;
        assert_eq!(shown(&Choices::new(&detail.versions)), (0, Some(1), None));
    }

    #[test]
    fn a_wrong_index_falls_back_to_the_default_audio_and_no_subtitles() {
        let mut detail = detail();
        detail.versions[0].default_audio = Some(9);
        detail.versions[0].default_subtitle = Some(Some(9));
        assert_eq!(shown(&Choices::new(&detail.versions)), (0, Some(1), None));
    }

    #[test]
    fn no_subtitles_from_the_server_turns_them_off() {
        let mut detail = detail();
        detail.versions[0].default_subtitle = Some(None);
        assert_eq!(shown(&Choices::new(&detail.versions)), (0, Some(2), None));
    }

    #[test]
    fn a_chip_choice_wins() {
        let detail = detail();
        let mut choices = Choices::new(&detail.versions);
        choices.choose_audio(Some(1));
        choices.choose_subtitle(Some(3));
        let plan = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        assert_eq!((plan.audio_index, plan.subtitle_index), (Some(1), Some(3)));
    }

    #[test]
    fn tracks_chosen_on_the_detail_page_reach_the_player() {
        let detail = detail();
        let mut choices = Choices::new(&detail.versions);
        // The English TrueHD track, the second audio track of the file; the full French ASS,
        // the second embedded subtitle.
        choices.audio = Some(2);
        choices.subtitle = Some(4);
        let plan = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
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
                "English · SRT · external file".to_string(),
                Some("eng".to_string())
            )]
        );
        assert_eq!((plan.audio_index, plan.subtitle_index), (Some(2), Some(4)));
    }

    #[test]
    fn an_external_subtitle_chosen_on_the_detail_page_is_shown_first() {
        let detail = detail();
        let mut choices = Choices::new(&detail.versions);
        choices.subtitle = Some(5);
        let plan = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let Call::Load { subtitle, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(subtitle, FirstSubtitle::External(0));
    }

    #[test]
    fn the_conversion_keeps_the_tracks_chosen_for_the_server_and_the_external_file_for_the_player()
    {
        let server = FakeServer::new(3, false);
        let detail = server.film(&ItemId(FILM.into())).unwrap();
        let mut choices = Choices::new(&detail.versions);
        choices.audio = Some(2);
        choices.subtitle = Some(4);
        let embedded = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let from = Duration::from_secs(4321);
        let request = embedded.transcode_request(from);
        assert_eq!(
            request,
            TranscodeRequest {
                film: embedded.film.clone(),
                source: embedded.source.clone(),
                start: from,
                audio_track: Some(2),
                subtitle_track: Some(4),
            }
        );
        let opened = server.transcode(&request).unwrap();
        let converted = embedded.converted(from, &opened);
        assert_eq!(converted.start, from);
        assert_eq!(converted.audio, None);
        assert_eq!(converted.audio_index, Some(2), "still reported");
        // The server extracted the chosen subtitle into a file: the player adds it and shows it.
        assert_eq!(converted.subtitle, FirstShown::External(4));
        let externals = converted.externals(&opened);
        let media = converted.media(&opened, &externals);
        let shown = opened
            .external_subtitles
            .iter()
            .position(|e| e.index == 4)
            .unwrap();
        assert_eq!(media.subtitle, FirstSubtitle::External(shown));
        assert_eq!(externals[shown].language, Some("fre"));

        // Drawn into the picture by the server: nothing for the player to show.
        let drawn = Playback {
            external_subtitles: Vec::new(),
            ..opened
        };
        assert_eq!(embedded.converted(from, &drawn).subtitle, FirstShown::Off);

        choices.subtitle = Some(5);
        let external = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let request = external.transcode_request(from);
        assert_eq!(request.subtitle_track, None);
        let opened = server.transcode(&request).unwrap();
        assert_eq!(
            external.converted(from, &opened).subtitle,
            FirstShown::External(5)
        );
    }

    #[test]
    fn no_subtitles_chosen_turns_them_off() {
        let detail = detail();
        let mut choices = Choices::new(&detail.versions);
        choices.choose_subtitle(None);
        let plan = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        let Call::Load { subtitle, .. } = load(&plan) else {
            panic!("not a load");
        };
        assert_eq!(subtitle, FirstSubtitle::Off);
    }

    #[test]
    fn the_version_chosen_is_the_one_played_with_its_own_tracks() {
        let detail = detail();
        let mut choices = Choices::new(&detail.versions);
        choices.subtitle = Some(5);
        choices.set_version(&detail.versions, 1);
        assert_eq!((choices.audio, choices.subtitle), (Some(1), Some(2)));
        let plan = plan(&detail.film, &detail.versions, &choices, From::Resume).unwrap();
        assert_eq!(plan.source, MediaSourceId("source-1080".into()));
    }

    #[test]
    fn the_watched_toggle_marks_and_unmarks_the_film_on_the_server() {
        let server = FakeServer::new(3, false);
        let film = server.film(&ItemId(FILM.into())).unwrap().film;
        let watched = toggle_watched(&server, &film).unwrap();
        assert!(watched.watched);
        assert_eq!(watched.resume, None);
        assert_eq!(play_label(&watched), "Play");
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
        assert_eq!(play_label(&detail.film), "Resume from 1h 12m");
        let mut film = detail.film.clone();
        film.resume = None;
        assert_eq!(play_label(&film), "Play");
        film.resume = Some(Duration::from_secs(42));
        assert_eq!(play_label(&film), "Resume from 42s");
    }

    #[test]
    fn a_short_duration_drops_the_minutes_of_a_whole_hour() {
        let secs = |seconds| short_duration(Duration::from_secs(seconds));
        assert_eq!(secs(42), "42s");
        assert_eq!(secs(47 * 60), "47m");
        assert_eq!(secs(2 * 3600), "2h");
        assert_eq!(secs(2 * 3600 + 11 * 60), "2h 11m");
    }

    #[test]
    fn the_page_says_year_rating_runtime_genres_and_age() {
        assert_eq!(
            meta(&detail()),
            ["2025", "★ 6.0", "2h 11m", "Drame, Aventure", "12+"]
        );
    }

    #[test]
    fn the_chips_name_the_version_and_the_tracks() {
        let versions = crate::fake::versions();
        assert_eq!(version_label(&versions[0]), "4K HDR10");
        assert_eq!(version_label(&versions[1]), "1080p");
        assert_eq!(audio_label(&versions[0].audio[0]), "French · E-AC-3 5.1");
        assert_eq!(subtitle_label(None), "None");
        assert_eq!(
            subtitle_label(Some(&versions[0].subtitles[0])),
            "French · SRT · forced"
        );
    }

    #[test]
    fn a_started_film_says_what_is_left_and_how_far() {
        let film = detail().film;
        assert_eq!(time_left(&film).as_deref(), Some("59m left"));
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
