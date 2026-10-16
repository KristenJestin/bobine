//! The tracks of a film, read from mpv's `track-list` property, and how the panel names them.

use super::{Track, TrackKind};

/// The tracks mpv lists, through `get`, which reads one mpv property as text
/// (`track-list/count`, `track-list/0/type`…). Video tracks are kept: the caller filters.
pub fn from_mpv(get: impl Fn(&str) -> Option<String>) -> Vec<Track> {
    let count: usize = get("track-list/count")
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    (0..count)
        .filter_map(|index| {
            let field = |name: &str| get(&format!("track-list/{index}/{name}"));
            let flag = |name: &str| field(name).as_deref() == Some("yes");
            let kind = match field("type")?.as_str() {
                "video" => TrackKind::Video,
                "audio" => TrackKind::Audio,
                "sub" => TrackKind::Subtitle,
                _ => return None,
            };
            Some(Track {
                id: field("id")?.parse().ok()?,
                kind,
                title: field("title"),
                language: field("lang"),
                codec: field("codec"),
                channels: field("demux-channel-count").and_then(|n| n.parse().ok()),
                forced: flag("forced"),
                external: flag("external"),
                selected: flag("selected"),
            })
        })
        .collect()
}

/// How the panel names a track: `French · E-AC-3 5.1`, `English · SRT · external file`.
pub fn label(track: &Track) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(language) = track.language.as_deref().and_then(language_name) {
        parts.push(language);
    }
    if let Some(codec) = &track.codec {
        let codec = codec_name(codec);
        parts.push(match track.channels.and_then(channel_layout) {
            Some(layout) => format!("{codec} {layout}"),
            None => codec,
        });
    }
    // An external track's title may be mpv's own, made from the end of its address: that can
    // hold an access token.
    if let Some(title) = track.title.as_ref().filter(|_| !track.external) {
        parts.push(title.clone());
    }
    if track.forced {
        parts.push("forced".into());
    }
    if track.external {
        parts.push("external file".into());
    }
    if parts.is_empty() {
        return format!("Track {}", track.id);
    }
    parts.join(" · ")
}

/// English names of the common languages, from their ISO 639 codes; others keep their code,
/// and "undetermined" has no name.
fn language_name(code: &str) -> Option<String> {
    let name = match code.to_ascii_lowercase().as_str() {
        "und" | "" => return None,
        "fr" | "fre" | "fra" => "French",
        "en" | "eng" => "English",
        "de" | "ger" | "deu" => "German",
        "es" | "spa" => "Spanish",
        "it" | "ita" => "Italian",
        "pt" | "por" => "Portuguese",
        "nl" | "dut" | "nld" => "Dutch",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "zh" | "chi" | "zho" => "Chinese",
        "ru" | "rus" => "Russian",
        _ => return Some(code.to_string()),
    };
    Some(name.to_string())
}

fn codec_name(codec: &str) -> String {
    match codec {
        "eac3" => "E-AC-3",
        "ac3" => "AC-3",
        "aac" => "AAC",
        "truehd" => "TrueHD",
        "dts" => "DTS",
        "flac" => "FLAC",
        "opus" => "Opus",
        "mp3" => "MP3",
        "ass" | "ssa" => "ASS",
        "subrip" => "SRT",
        "webvtt" => "WebVTT",
        "hdmv_pgs_subtitle" => "PGS",
        "dvd_subtitle" => "VobSub",
        other => return other.to_uppercase(),
    }
    .to_string()
}

fn channel_layout(channels: u32) -> Option<&'static str> {
    match channels {
        1 => Some("mono"),
        2 => Some("2.0"),
        6 => Some("5.1"),
        8 => Some("7.1"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// mpv's answers for a film with a video, two audio tracks, an embedded ASS track and an
    /// external SRT.
    fn mpv_properties() -> HashMap<String, String> {
        let mut properties = HashMap::new();
        let mut track = |index: usize, fields: &[(&str, &str)]| {
            for (name, value) in fields {
                properties.insert(format!("track-list/{index}/{name}"), value.to_string());
            }
        };
        track(
            0,
            &[
                ("id", "1"),
                ("type", "video"),
                ("codec", "hevc"),
                ("selected", "yes"),
                ("external", "no"),
                ("forced", "no"),
            ],
        );
        track(
            1,
            &[
                ("id", "1"),
                ("type", "audio"),
                ("lang", "fre"),
                ("codec", "eac3"),
                ("demux-channel-count", "6"),
                ("selected", "yes"),
                ("external", "no"),
                ("forced", "no"),
            ],
        );
        track(
            2,
            &[
                ("id", "2"),
                ("type", "audio"),
                ("lang", "eng"),
                ("title", "Commentary"),
                ("codec", "aac"),
                ("demux-channel-count", "2"),
                ("selected", "no"),
                ("external", "no"),
                ("forced", "no"),
            ],
        );
        track(
            3,
            &[
                ("id", "1"),
                ("type", "sub"),
                ("lang", "fre"),
                ("codec", "ass"),
                ("selected", "no"),
                ("external", "no"),
                ("forced", "yes"),
            ],
        );
        track(
            4,
            &[
                ("id", "2"),
                ("type", "sub"),
                ("lang", "eng"),
                ("codec", "subrip"),
                ("selected", "yes"),
                ("external", "yes"),
                ("forced", "no"),
            ],
        );
        properties.insert("track-list/count".into(), "5".into());
        properties
    }

    fn tracks() -> Vec<Track> {
        let properties = mpv_properties();
        from_mpv(|name| properties.get(name).cloned())
    }

    #[test]
    fn the_track_list_is_read_from_mpv_track_list() {
        let tracks = tracks();
        assert_eq!(tracks.len(), 5);
        assert_eq!(
            tracks[1],
            Track {
                id: 1,
                kind: TrackKind::Audio,
                title: None,
                language: Some("fre".into()),
                codec: Some("eac3".into()),
                channels: Some(6),
                forced: false,
                external: false,
                selected: true,
            }
        );
        assert_eq!(tracks[4].kind, TrackKind::Subtitle);
        assert!(tracks[4].external);
        assert!(tracks[3].forced);
    }

    #[test]
    fn the_current_track_is_the_one_mpv_selected() {
        let selected: Vec<(TrackKind, i64)> = tracks()
            .iter()
            .filter(|track| track.selected)
            .map(|track| (track.kind, track.id))
            .collect();
        assert_eq!(
            selected,
            [
                (TrackKind::Video, 1),
                (TrackKind::Audio, 1),
                (TrackKind::Subtitle, 2)
            ]
        );
    }

    #[test]
    fn a_film_without_tracks_has_an_empty_list() {
        assert_eq!(from_mpv(|_| None), []);
    }

    #[test]
    fn a_track_of_an_unknown_type_is_left_out() {
        let mut properties = mpv_properties();
        properties.insert("track-list/0/type".into(), "data".into());
        let tracks = from_mpv(|name| properties.get(name).cloned());
        assert_eq!(tracks.len(), 4);
    }

    #[test]
    fn an_audio_track_is_named_by_language_codec_and_channels() {
        let tracks = tracks();
        assert_eq!(label(&tracks[1]), "French · E-AC-3 5.1");
        assert_eq!(label(&tracks[2]), "English · AAC 2.0 · Commentary");
    }

    #[test]
    fn a_subtitle_track_says_forced_and_external() {
        let tracks = tracks();
        assert_eq!(label(&tracks[3]), "French · ASS · forced");
        assert_eq!(label(&tracks[4]), "English · SRT · external file");
    }

    #[test]
    fn an_external_subtitle_added_from_a_url_with_a_query_shows_neither_in_its_label() {
        // mpv names a track added without a title after the end of its address.
        let mut properties = mpv_properties();
        properties.insert(
            "track-list/4/title".into(),
            "Stream.srt?ApiKey=0123456789abcdef".into(),
        );
        properties.remove("track-list/4/lang");
        let tracks = from_mpv(|name| properties.get(name).cloned());
        let label = label(&tracks[4]);
        assert!(!label.contains("Stream.srt"), "{label}");
        assert!(!label.contains("ApiKey"), "{label}");
        assert!(!label.contains("0123456789abcdef"), "{label}");
        assert_eq!(label, "SRT · external file");
    }

    #[test]
    fn a_track_without_language_keeps_its_codec_or_its_number() {
        let track = Track {
            id: 3,
            kind: TrackKind::Subtitle,
            title: None,
            language: None,
            codec: None,
            channels: None,
            forced: false,
            external: false,
            selected: false,
        };
        assert_eq!(label(&track), "Track 3");
        let with_codec = Track {
            codec: Some("hdmv_pgs_subtitle".into()),
            ..track.clone()
        };
        assert_eq!(label(&with_codec), "PGS");
        let unknown = Track {
            language: Some("und".into()),
            ..track
        };
        assert_eq!(label(&unknown), "Track 3");
    }
}
