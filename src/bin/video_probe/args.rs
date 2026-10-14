//! The probe's command line.

use std::path::PathBuf;

/// How the video reaches the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// Way 1: mpv's OpenGL render API draws into a `wl_subsurface` placed under the GPUI surface.
    Subsurface,
    /// Way 2: mpv opens and owns its own window (`vo=gpu-next`).
    Window,
    /// Way 3: mpv's software render API, copied into a GPUI image each frame.
    Software,
}

#[derive(Debug, PartialEq)]
pub struct Args {
    pub way: Way,
    pub file: PathBuf,
    pub sub_file: Option<PathBuf>,
    /// Seconds of playback before the measurement starts.
    pub warmup: f64,
    /// Seconds the measurement lasts.
    pub measure: f64,
    /// mpv options set after the way's own, to try a variant without rebuilding.
    pub mpv_options: Vec<(String, String)>,
}

pub const USAGE: &str = "usage: video_probe --way subsurface|window|software [--sub FILE.srt] \
                         [--warmup SECONDS] [--measure SECONDS] [--mpv NAME=VALUE]... FILE";

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
        let mut args = args.into_iter();
        let (mut way, mut file, mut sub_file) = (None, None, None);
        let (mut warmup, mut measure) = (3.0, 10.0);
        let mut mpv_options = Vec::new();
        while let Some(arg) = args.next() {
            let mut value = || args.next().ok_or(format!("{arg} needs a value"));
            match arg.as_str() {
                "--way" => {
                    way = Some(match value()?.as_str() {
                        "subsurface" => Way::Subsurface,
                        "window" => Way::Window,
                        "software" => Way::Software,
                        other => return Err(format!("unknown way {other}")),
                    })
                }
                "--sub" => sub_file = Some(PathBuf::from(value()?)),
                "--warmup" => warmup = seconds(&value()?)?,
                "--measure" => measure = seconds(&value()?)?,
                "--mpv" => {
                    let option = value()?;
                    let (name, value) = option
                        .split_once('=')
                        .ok_or(format!("--mpv {option} is not NAME=VALUE"))?;
                    mpv_options.push((name.to_string(), value.to_string()));
                }
                option if option.starts_with("--") => {
                    return Err(format!("unknown option {option}"));
                }
                _ => file = Some(PathBuf::from(arg)),
            }
        }
        Ok(Args {
            way: way.ok_or("--way is required")?,
            file: file.ok_or("a file to play is required")?,
            sub_file,
            warmup,
            measure,
            mpv_options,
        })
    }
}

fn seconds(value: &str) -> Result<f64, String> {
    value
        .parse()
        .map_err(|_| format!("{value} is not a number of seconds"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Args, String> {
        Args::parse(line.split_whitespace().map(String::from))
    }

    #[test]
    fn parses_every_option() {
        let args = parse("--way software --sub a.srt --warmup 1.5 --measure 8 a.mp4").unwrap();
        assert_eq!(
            args,
            Args {
                way: Way::Software,
                file: "a.mp4".into(),
                sub_file: Some("a.srt".into()),
                warmup: 1.5,
                measure: 8.0,
                mpv_options: Vec::new(),
            }
        );
    }

    #[test]
    fn defaults_to_three_seconds_of_warmup_and_ten_of_measure() {
        let args = parse("--way subsurface a.mkv").unwrap();
        assert_eq!(
            (args.warmup, args.measure, args.sub_file),
            (3.0, 10.0, None)
        );
    }

    #[test]
    fn collects_extra_mpv_options_in_order() {
        let args = parse("--way window --mpv gpu-api=opengl --mpv hwdec=no a.mp4").unwrap();
        assert_eq!(
            args.mpv_options,
            vec![
                ("gpu-api".to_string(), "opengl".to_string()),
                ("hwdec".to_string(), "no".to_string())
            ]
        );
    }

    #[test]
    fn rejects_an_mpv_option_without_a_value() {
        assert!(parse("--way window --mpv gpu-api a.mp4").is_err());
    }

    #[test]
    fn requires_a_way_and_a_file() {
        assert!(parse("a.mp4").is_err());
        assert!(parse("--way window").is_err());
    }

    #[test]
    fn rejects_an_unknown_way_or_option() {
        assert!(parse("--way texture a.mp4").is_err());
        assert!(parse("--way window --fast a.mp4").is_err());
    }
}
