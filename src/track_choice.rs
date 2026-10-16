//! Which audio and subtitle tracks a playback starts with, the way Jellyfin's own clients choose
//! them from the user's settings. Pure: the shell reads the settings, this decides.

use bobine::provider::{AudioTrack, MediaSource, SubtitleMode, SubtitleTrack, TrackPreferences};

/// The languages the viewer chose by hand for the episode before, for the next one to keep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Remembered {
    pub audio: Option<String>,
    /// `Some(None)`: they turned the subtitles off.
    pub subtitle: Option<Option<String>>,
}

/// What decides the tracks besides the file: the user's settings, when the server gave them,
/// and what was chosen by hand before.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    /// None: the server could not say, and the file's own flags choose.
    pub preferences: Option<TrackPreferences>,
    pub remembered: Remembered,
}

/// The audio and the subtitle track a playback of `source` starts with, by index.
pub fn pick(source: &MediaSource, rules: &Rules) -> (Option<i32>, Option<i32>) {
    let Some(preferences) = &rules.preferences else {
        return (default_audio(source), default_subtitle(source));
    };
    let audio = pick_audio(source, preferences, &rules.remembered);
    let subtitle = pick_subtitle(source, preferences, &rules.remembered, audio);
    (audio, subtitle)
}

/// What the chips showed before the settings were read: the file's default audio track or the
/// first, and the default subtitle only.
fn default_audio(source: &MediaSource) -> Option<i32> {
    source
        .audio
        .iter()
        .find(|track| track.default)
        .or(source.audio.first())
        .map(|track| track.index)
}

fn default_subtitle(source: &MediaSource) -> Option<i32> {
    source
        .subtitles
        .iter()
        .find(|track| track.default)
        .map(|track| track.index)
}

fn pick_audio(
    source: &MediaSource,
    preferences: &TrackPreferences,
    remembered: &Remembered,
) -> Option<i32> {
    let in_language = |language: &str| {
        source
            .audio
            .iter()
            .find(|track| is_in(track.language.as_deref(), language))
    };
    let by_hand = remembered
        .audio
        .as_deref()
        .filter(|_| preferences.remember_audio)
        .and_then(in_language);
    let default = source.audio.iter().find(|track| track.default);
    let wanted = preferences
        .audio_language
        .as_deref()
        .filter(|_| !(preferences.play_default_audio && default.is_some()))
        .and_then(in_language);
    by_hand
        .or(wanted)
        .or(default)
        .or(source.audio.first())
        .map(|track| track.index)
}

fn pick_subtitle(
    source: &MediaSource,
    preferences: &TrackPreferences,
    remembered: &Remembered,
    audio: Option<i32>,
) -> Option<i32> {
    let tracks = &source.subtitles;
    let preferred = preferences.subtitle_language.as_deref();
    // A full track over a forced one: forced tracks only translate the odd foreign line.
    let in_language = |language: &str| {
        let mut found = tracks
            .iter()
            .filter(|track| is_in(track.language.as_deref(), language));
        let first = found.next();
        match first {
            Some(track) if track.forced => found.find(|track| !track.forced).or(first),
            other => other,
        }
    };
    let forced = || {
        let forced = || tracks.iter().filter(|track| track.forced);
        preferred
            .and_then(|language| forced().find(|t| is_in(t.language.as_deref(), language)))
            .or_else(|| forced().next())
    };
    let default = || tracks.iter().find(|track| track.default);

    if preferences.remember_subtitle
        && let Some(by_hand) = &remembered.subtitle
    {
        // They turned the subtitles off: they stay off.
        let language = by_hand.as_ref()?;
        if let Some(track) = in_language(language) {
            return Some(track.index);
        }
    }
    let track: Option<&SubtitleTrack> = match preferences.subtitle_mode {
        SubtitleMode::None => None,
        SubtitleMode::OnlyForced => forced(),
        SubtitleMode::Always => preferred
            .and_then(in_language)
            .or_else(default)
            .or(tracks.first()),
        SubtitleMode::Default => default().or_else(forced),
        SubtitleMode::Smart => {
            let heard: Option<&AudioTrack> = source.audio.iter().find(|t| Some(t.index) == audio);
            preferred
                .filter(|language| !is_in(heard.and_then(|t| t.language.as_deref()), language))
                .and_then(in_language)
        }
    };
    track.map(|track| track.index)
}

fn is_in(track: Option<&str>, language: &str) -> bool {
    track.is_some_and(|track| same_language(track, language))
}

/// ISO 639-2 has two codes for twenty languages, the bibliographic one (`fre`) and the
/// terminological one (`fra`); files and settings use either.
const TWO_CODES: [(&str, &str); 20] = [
    ("alb", "sqi"),
    ("arm", "hye"),
    ("baq", "eus"),
    ("bur", "mya"),
    ("chi", "zho"),
    ("cze", "ces"),
    ("dut", "nld"),
    ("fre", "fra"),
    ("geo", "kat"),
    ("ger", "deu"),
    ("gre", "ell"),
    ("ice", "isl"),
    ("mac", "mkd"),
    ("mao", "mri"),
    ("may", "msa"),
    ("per", "fas"),
    ("rum", "ron"),
    ("slo", "slk"),
    ("tib", "bod"),
    ("wel", "cym"),
];

fn same_language(a: &str, b: &str) -> bool {
    let canonical = |code: &str| {
        let code = code.to_ascii_lowercase();
        match TWO_CODES.iter().find(|(old, _)| *old == code) {
            Some((_, new)) => new.to_string(),
            None => code,
        }
    };
    canonical(a) == canonical(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobine::provider::MediaSourceId;

    fn audio(index: i32, language: &str, default: bool) -> AudioTrack {
        AudioTrack {
            index,
            language: Some(language.into()),
            codec: None,
            channels: None,
            title: None,
            default,
        }
    }

    fn subtitle(index: i32, language: &str, forced: bool, default: bool) -> SubtitleTrack {
        SubtitleTrack {
            index,
            language: Some(language.into()),
            codec: None,
            title: None,
            forced,
            default,
            external: false,
        }
    }

    /// French (default) then English audio; a forced French, a full French, a full English and
    /// a forced English subtitle.
    fn file() -> MediaSource {
        MediaSource {
            id: MediaSourceId("source".into()),
            name: "Invented".into(),
            container: None,
            video: None,
            audio: vec![audio(1, "fre", true), audio(2, "eng", false)],
            subtitles: vec![
                subtitle(3, "fre", true, false),
                subtitle(4, "fre", false, false),
                subtitle(5, "eng", false, false),
                subtitle(6, "eng", true, false),
            ],
        }
    }

    fn preferences() -> TrackPreferences {
        TrackPreferences {
            audio_language: None,
            play_default_audio: false,
            subtitle_language: None,
            subtitle_mode: SubtitleMode::Default,
            remember_audio: false,
            remember_subtitle: false,
        }
    }

    fn rules(preferences: TrackPreferences) -> Rules {
        Rules {
            preferences: Some(preferences),
            remembered: Remembered::default(),
        }
    }

    fn mode(mode: SubtitleMode, language: Option<&str>) -> Rules {
        rules(TrackPreferences {
            subtitle_mode: mode,
            subtitle_language: language.map(Into::into),
            ..preferences()
        })
    }

    #[test]
    fn with_no_preferences_the_choice_is_the_same_as_today() {
        assert_eq!(pick(&file(), &Rules::default()), (Some(1), None));
        let mut no_default = file();
        no_default.audio[0].default = false;
        assert_eq!(pick(&no_default, &Rules::default()), (Some(1), None));
        no_default.subtitles[2].default = true;
        assert_eq!(pick(&no_default, &Rules::default()), (Some(1), Some(5)));
        let mut empty = file();
        empty.audio.clear();
        assert_eq!(pick(&empty, &Rules::default()).0, None);
    }

    #[test]
    fn a_preferred_audio_language_picks_that_track_over_the_default_one() {
        let rules = rules(TrackPreferences {
            audio_language: Some("eng".into()),
            ..preferences()
        });
        assert_eq!(pick(&file(), &rules).0, Some(2));
    }

    #[test]
    fn the_default_audio_track_wins_when_the_user_plays_the_default_track() {
        let rules = rules(TrackPreferences {
            audio_language: Some("eng".into()),
            play_default_audio: true,
            ..preferences()
        });
        assert_eq!(pick(&file(), &rules).0, Some(1));
        // No default track in the file: the preferred language is used all the same.
        let mut flagless = file();
        flagless.audio[0].default = false;
        assert_eq!(pick(&flagless, &rules).0, Some(2));
    }

    #[test]
    fn a_preferred_audio_language_no_track_has_leaves_the_default_then_the_first() {
        let rules = rules(TrackPreferences {
            audio_language: Some("jpn".into()),
            ..preferences()
        });
        assert_eq!(pick(&file(), &rules).0, Some(1));
        let mut flagless = file();
        flagless.audio[0].default = false;
        assert_eq!(pick(&flagless, &rules).0, Some(1));
    }

    #[test]
    fn both_forms_of_a_language_code_name_the_same_language() {
        assert!(same_language("fre", "fra"));
        assert!(same_language("FRA", "fre"));
        assert!(same_language("ger", "deu"));
        assert!(same_language("eng", "eng"));
        assert!(!same_language("fre", "ger"));
        let rules = rules(TrackPreferences {
            audio_language: Some("fra".into()),
            ..preferences()
        });
        let mut english_first = file();
        english_first.audio = vec![audio(1, "eng", true), audio(2, "fre", false)];
        assert_eq!(pick(&english_first, &rules).0, Some(2));
    }

    #[test]
    fn subtitle_mode_none_picks_no_subtitles() {
        let mut flagged = file();
        flagged.subtitles[1].default = true;
        assert_eq!(
            pick(&flagged, &mode(SubtitleMode::None, Some("fre"))).1,
            None
        );
    }

    #[test]
    fn subtitle_mode_only_forced_picks_a_forced_track_preferred_language_first() {
        let only_forced = |language| pick(&file(), &mode(SubtitleMode::OnlyForced, language)).1;
        assert_eq!(only_forced(Some("eng")), Some(6));
        assert_eq!(only_forced(Some("fra")), Some(3));
        assert_eq!(only_forced(Some("jpn")), Some(3), "any forced track");
        assert_eq!(only_forced(None), Some(3));
        let mut none_forced = file();
        none_forced.subtitles.retain(|track| !track.forced);
        assert_eq!(
            pick(&none_forced, &mode(SubtitleMode::OnlyForced, Some("eng"))).1,
            None
        );
    }

    #[test]
    fn subtitle_mode_always_picks_the_preferred_language_else_the_default_else_the_first() {
        let always =
            |file: &MediaSource, language| pick(file, &mode(SubtitleMode::Always, language)).1;
        // The full track, not the forced one that comes first.
        assert_eq!(always(&file(), Some("fre")), Some(4));
        assert_eq!(always(&file(), Some("eng")), Some(5));
        let mut flagged = file();
        flagged.subtitles[2].default = true;
        assert_eq!(always(&flagged, Some("jpn")), Some(5));
        assert_eq!(always(&file(), Some("jpn")), Some(3));
        let mut empty = file();
        empty.subtitles.clear();
        assert_eq!(always(&empty, Some("eng")), None);
    }

    #[test]
    fn subtitle_mode_default_picks_the_default_or_the_forced_track() {
        let default = |file: &MediaSource| pick(file, &mode(SubtitleMode::Default, Some("eng"))).1;
        assert_eq!(
            default(&file()),
            Some(6),
            "the forced track of the language"
        );
        let mut flagged = file();
        flagged.subtitles[1].default = true;
        assert_eq!(default(&flagged), Some(4), "the default track first");
        let mut none = file();
        none.subtitles.retain(|track| !track.forced);
        assert_eq!(default(&none), None);
    }

    #[test]
    fn smart_mode_shows_subtitles_in_the_preferred_language_when_the_audio_is_not() {
        let mut rules = mode(SubtitleMode::Smart, Some("fra"));
        // English audio, French wanted.
        rules.preferences.as_mut().unwrap().audio_language = Some("eng".into());
        assert_eq!(pick(&file(), &rules), (Some(2), Some(4)));
    }

    #[test]
    fn smart_mode_shows_no_subtitles_when_the_audio_is_already_in_the_preferred_language() {
        // French audio by default, French subtitles wanted (the other code of the language).
        assert_eq!(
            pick(&file(), &mode(SubtitleMode::Smart, Some("fra"))),
            (Some(1), None)
        );
        // No preferred subtitle language: nothing to show.
        assert_eq!(pick(&file(), &mode(SubtitleMode::Smart, None)).1, None);
    }

    #[test]
    fn the_next_episode_keeps_the_audio_and_subtitle_languages_chosen_by_hand() {
        let remembering = TrackPreferences {
            remember_audio: true,
            remember_subtitle: true,
            ..preferences()
        };
        let kept = Rules {
            preferences: Some(remembering.clone()),
            remembered: Remembered {
                audio: Some("eng".into()),
                subtitle: Some(Some("fra".into())),
            },
        };
        assert_eq!(pick(&file(), &kept), (Some(2), Some(4)));

        let off = Rules {
            remembered: Remembered {
                audio: Some("eng".into()),
                subtitle: Some(None),
            },
            ..kept.clone()
        };
        let mut forced_default = file();
        forced_default.subtitles[0].default = true;
        assert_eq!(pick(&forced_default, &off), (Some(2), None));
    }

    #[test]
    fn the_languages_chosen_by_hand_are_forgotten_when_the_settings_do_not_remember_them() {
        let forgetting = Rules {
            preferences: Some(preferences()),
            remembered: Remembered {
                audio: Some("eng".into()),
                subtitle: Some(Some("eng".into())),
            },
        };
        let nothing_remembered = Rules {
            remembered: Remembered::default(),
            ..forgetting.clone()
        };
        assert_eq!(
            pick(&file(), &forgetting),
            pick(&file(), &nothing_remembered)
        );
    }

    #[test]
    fn a_language_chosen_by_hand_the_next_file_lacks_falls_back_on_the_settings() {
        let kept = Rules {
            preferences: Some(TrackPreferences {
                remember_audio: true,
                remember_subtitle: true,
                ..preferences()
            }),
            remembered: Remembered {
                audio: Some("jpn".into()),
                subtitle: Some(Some("jpn".into())),
            },
        };
        assert_eq!(pick(&file(), &kept), (Some(1), Some(3)));
    }
}
