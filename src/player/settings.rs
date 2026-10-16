//! What the player's settings menu offers (speed, subtitle offset, aspect, quality) and how it
//! names what "Playback info" shows. No window here: the session keeps the choices and the view
//! draws them.

use super::{PlaybackInfo, tracks};

/// A page of the settings menu: the list of settings, or one of them opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Main,
    Speed,
    Aspect,
    Quality,
    Info,
}

/// The speeds offered, the film's own in the middle.
pub const SPEEDS: [f64; 7] = [0.5, 0.75, 1., 1.25, 1.5, 1.75, 2.];

/// "1×", "1.25×".
pub fn speed_label(speed: f64) -> String {
    format!("{speed}×")
}

/// The subtitle offset moves by tenths of a second: kept as a count of them, it never drifts.
pub fn offset_label(tenths: i32) -> String {
    let seconds = f64::from(tenths) / 10.;
    if tenths > 0 {
        format!("+{seconds:.1} s")
    } else {
        format!("{seconds:.1} s")
    }
}

/// The file as it is, or what the server converts it to, no more than so many bits per second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    Original,
    Cap(u64),
}

impl Quality {
    pub const ALL: [Quality; 8] = [
        Self::Original,
        Self::Cap(40_000_000),
        Self::Cap(20_000_000),
        Self::Cap(10_000_000),
        Self::Cap(6_000_000),
        Self::Cap(4_000_000),
        Self::Cap(2_000_000),
        Self::Cap(1_000_000),
    ];

    /// `direct`: the file plays as it is, which "Original" says.
    pub fn label(self, direct: bool) -> String {
        match self {
            Self::Original if direct => "Original (direct)".into(),
            Self::Original => "Original".into(),
            Self::Cap(bits) => bitrate(bits as f64),
        }
    }

    /// What the main page shows beside "Quality".
    pub fn short_label(self) -> String {
        match self {
            Self::Original => "Original".into(),
            Self::Cap(bits) => bitrate(bits as f64),
        }
    }
}

/// "20 Mbps", "18.5 Mbps", "850 kbps".
pub fn bitrate(bits: f64) -> String {
    if bits >= 1_000_000. {
        let megabits = bits / 1_000_000.;
        if megabits.fract() < 0.05 || megabits >= 100. {
            format!("{megabits:.0} Mbps")
        } else {
            format!("{megabits:.1} Mbps")
        }
    } else {
        format!("{:.0} kbps", bits / 1000.)
    }
}

/// The lines of "Playback info": how the film comes, then what the player says of it, the lines
/// it cannot say yet left out.
pub fn info_lines(converted: bool, info: &PlaybackInfo) -> Vec<(&'static str, String)> {
    let method = if converted {
        "Converted by the server"
    } else {
        "Direct play"
    };
    let mut lines = vec![("Playback", method.to_string())];
    let mut add = |label, value: Option<String>| {
        if let Some(value) = value {
            lines.push((label, value));
        }
    };
    add("Container", info.container.as_deref().map(container_name));
    add("Video", info.video_codec.as_deref().map(video_codec_name));
    add("Audio", info.audio_codec.as_deref().map(tracks::codec_name));
    add(
        "Resolution",
        info.size
            .map(|(width, height)| format!("{width} × {height}")),
    );
    add("Bitrate", info.video_bitrate.map(bitrate));
    add("Dropped frames", info.dropped_frames.map(|n| n.to_string()));
    lines.push((
        "Hardware decoding",
        info.hardware_decoder
            .clone()
            .unwrap_or_else(|| "Off".to_string()),
    ));
    lines
}

/// mpv names a container by its demuxer, `matroska,webm`: the name people know instead.
fn container_name(format: &str) -> String {
    match format.split(',').next().unwrap_or(format) {
        "matroska" => "MKV".into(),
        "mov" => "MP4".into(),
        "mpegts" => "MPEG-TS".into(),
        "hls" => "HLS".into(),
        other => other.to_uppercase(),
    }
}

fn video_codec_name(codec: &str) -> String {
    match codec {
        "h264" => "H.264".into(),
        "hevc" => "HEVC".into(),
        "av1" => "AV1".into(),
        "vp9" => "VP9".into(),
        "mpeg2video" => "MPEG-2".into(),
        other => other.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_read_as_multiples() {
        let labels: Vec<String> = SPEEDS.iter().map(|s| speed_label(*s)).collect();
        assert_eq!(
            labels,
            ["0.5×", "0.75×", "1×", "1.25×", "1.5×", "1.75×", "2×"]
        );
    }

    #[test]
    fn the_subtitle_offset_reads_in_seconds_with_its_sign() {
        assert_eq!(offset_label(0), "0.0 s");
        assert_eq!(offset_label(3), "+0.3 s");
        assert_eq!(offset_label(-12), "-1.2 s");
    }

    #[test]
    fn bitrates_read_in_megabits_or_kilobits() {
        assert_eq!(bitrate(20_000_000.), "20 Mbps");
        assert_eq!(bitrate(18_460_000.), "18.5 Mbps");
        assert_eq!(bitrate(850_000.), "850 kbps");
        assert_eq!(Quality::Original.label(true), "Original (direct)");
        assert_eq!(Quality::Cap(4_000_000).label(true), "4 Mbps");
    }

    #[test]
    fn original_after_a_failed_direct_play_is_not_called_direct() {
        assert_eq!(Quality::Original.label(false), "Original");
    }
}
