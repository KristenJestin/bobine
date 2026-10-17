//! Where Bobine keeps the account file (device id, server address, user name: nothing secret),
//! and the viewer's settings beside it.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// `$XDG_CONFIG_HOME/bobine/account.toml`, else `~/.config/bobine/account.toml`. A relative
/// `XDG_CONFIG_HOME` is ignored, as the XDG specification says.
pub fn account_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let config = xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("bobine").join("account.toml"))
}

/// `settings.toml`, beside the account file.
pub fn settings_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    Some(account_path(xdg_config_home, home)?.with_file_name("settings.toml"))
}

/// What the viewer chose, kept between two starts. A setting the file lacks keeps its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "Black out other screens": the other monitors turn black while a film plays fullscreen.
    pub black_out_other_monitors: bool,
    /// What the player asks of the server for a film that is converted: the file as it is, or
    /// a cap on the bitrate. The player's quality menu reads it; nothing does yet.
    pub default_quality: Quality,
    /// Where the request server (Seerr) answers. Its API key is in the keyring, never here.
    pub seerr_address: Option<String>,
    /// The desktop is told when a series the viewer follows gains an episode.
    pub notify_new_episodes: bool,
    /// For each account (`<server id>/<user id>`), the newest episode the last check of new
    /// episodes saw: ticks of 100 ns since 1970, by the server's clock, which the episodes are
    /// dated by.
    /// Last: a table goes after the plain values.
    pub episodes_seen: BTreeMap<String, i64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            black_out_other_monitors: false,
            default_quality: Quality::default(),
            seerr_address: None,
            notify_new_episodes: true,
            episodes_seen: BTreeMap::new(),
        }
    }
}

/// How much of the picture the viewer wants when the server converts a film.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub enum Quality {
    /// The file as it is, whatever its bitrate.
    #[default]
    Original,
    /// At most this many megabits a second.
    Cap(u32),
}

impl Quality {
    /// What the settings offer, the original first, then the caps from the highest down.
    pub const CHOICES: [Quality; 6] = [
        Quality::Original,
        Quality::Cap(40),
        Quality::Cap(20),
        Quality::Cap(10),
        Quality::Cap(4),
        Quality::Cap(2),
    ];

    /// The same choice, as the player's quality menu holds it (bits a second).
    pub fn for_player(self) -> crate::player::settings::Quality {
        use crate::player::settings::Quality as Player;
        match self {
            Quality::Original => Player::Original,
            Quality::Cap(mbps) => Player::Cap(u64::from(mbps) * 1_000_000),
        }
    }

    pub fn label(self) -> String {
        match self {
            Quality::Original => "Original".to_string(),
            Quality::Cap(mbps) => format!("{mbps} Mbps"),
        }
    }
}

impl From<Quality> for String {
    fn from(quality: Quality) -> String {
        match quality {
            Quality::Original => "original".to_string(),
            Quality::Cap(mbps) => format!("{mbps}mbps"),
        }
    }
}

/// A value the file holds that is not a quality, a number as much as an unknown word, is the
/// original: the rest of the file still counts.
impl<'de> Deserialize<'de> for Quality {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match toml::Value::deserialize(deserializer)? {
            toml::Value::String(text) => Quality::from(text),
            _ => Quality::Original,
        })
    }
}

impl From<String> for Quality {
    fn from(text: String) -> Quality {
        text.strip_suffix("mbps")
            .and_then(|mbps| mbps.parse().ok())
            .filter(|mbps| *mbps > 0)
            .map_or(Quality::Original, Quality::Cap)
    }
}

impl Settings {
    /// The settings kept at `path`; the defaults when there are none, or none that can be read.
    pub fn read(path: &Path) -> Settings {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        toml::from_str(&text).unwrap_or_default()
    }

    /// Writes the whole file anew, through a temporary file so a crash leaves the old one.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        let mut tmp = PathBuf::from(path);
        tmp.as_mut_os_string().push(".tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_go_beside_the_account_file() {
        assert_eq!(
            settings_path(Some("/tmp/conf".into()), Some("/home/ada".into())),
            Some(PathBuf::from("/tmp/conf/bobine/settings.toml"))
        );
    }

    #[test]
    fn black_out_other_monitors_is_off_by_default() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::read(&dir.path().join("bobine/settings.toml"));
        assert!(!settings.black_out_other_monitors);
    }

    #[test]
    fn black_out_other_monitors_is_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine/settings.toml");
        let on = Settings {
            black_out_other_monitors: true,
            ..Settings::default()
        };
        on.save(&path).unwrap();
        assert_eq!(Settings::read(&path), on);
    }

    #[test]
    fn an_unreadable_settings_file_gives_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(&path, "black_out_other_monitors = \"maybe\"").unwrap();
        assert_eq!(Settings::read(&path), Settings::default());
    }

    #[test]
    fn new_episodes_are_notified_by_default() {
        assert!(Settings::default().notify_new_episodes);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.toml"),
            "black_out_other_monitors = true\n",
        )
        .unwrap();
        assert!(Settings::read(&dir.path().join("settings.toml")).notify_new_episodes);
    }

    #[test]
    fn the_last_check_of_new_episodes_is_kept_for_each_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine/settings.toml");
        let mut settings = Settings::default();
        settings
            .episodes_seen
            .insert("server-1/user-1".into(), 1_757_838_605);
        settings.episodes_seen.insert("server-2/user-1".into(), 12);
        settings.save(&path).unwrap();
        let read = Settings::read(&path);
        assert_eq!(
            read.episodes_seen.get("server-1/user-1"),
            Some(&1_757_838_605)
        );
        assert_eq!(read.episodes_seen.get("server-2/user-1"), Some(&12));
    }

    #[test]
    fn the_default_quality_is_the_original_file() {
        assert_eq!(Settings::default().default_quality, Quality::Original);
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings::read(&dir.path().join("settings.toml"));
        assert_eq!(settings.default_quality, Quality::Original);
    }

    #[test]
    fn a_change_is_saved_and_read_back_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bobine/settings.toml");
        let changed = Settings {
            black_out_other_monitors: true,
            default_quality: Quality::Cap(8),
            seerr_address: Some("https://seerr.example.org".into()),
            notify_new_episodes: false,
            episodes_seen: [("server-1/user-1".to_string(), 7)].into(),
        };
        changed.save(&path).unwrap();
        assert_eq!(Settings::read(&path), changed);
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn a_quality_the_file_does_not_know_falls_back_to_original_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "black_out_other_monitors = true\ndefault_quality = \"fast\"\n",
        )
        .unwrap();
        let settings = Settings::read(&path);
        assert_eq!(settings.default_quality, Quality::Original);
        assert!(settings.black_out_other_monitors);
    }

    #[test]
    fn a_quality_written_as_a_number_falls_back_to_original_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        std::fs::write(
            &path,
            "black_out_other_monitors = true\ndefault_quality = 20\n",
        )
        .unwrap();
        let settings = Settings::read(&path);
        assert_eq!(settings.default_quality, Quality::Original);
        assert!(settings.black_out_other_monitors);
    }

    #[test]
    fn a_cap_is_written_as_a_number_of_megabits_a_second() {
        assert_eq!(String::from(Quality::Original), "original");
        assert_eq!(String::from(Quality::Cap(20)), "20mbps");
        assert_eq!(Quality::from("20mbps".to_string()), Quality::Cap(20));
        // A cap of nothing caps nothing sensible: the original.
        assert_eq!(Quality::from("0mbps".to_string()), Quality::Original);
    }

    #[test]
    fn a_quality_is_said_in_words() {
        assert_eq!(Quality::Original.label(), "Original");
        assert_eq!(Quality::Cap(8).label(), "8 Mbps");
    }

    #[test]
    fn the_account_file_goes_under_xdg_config_home() {
        assert_eq!(
            account_path(Some("/tmp/conf".into()), Some("/home/ada".into())),
            Some(PathBuf::from("/tmp/conf/bobine/account.toml"))
        );
    }

    #[test]
    fn without_xdg_config_home_the_account_file_goes_under_dot_config() {
        assert_eq!(
            account_path(None, Some("/home/ada".into())),
            Some(PathBuf::from("/home/ada/.config/bobine/account.toml"))
        );
        assert_eq!(
            account_path(Some("relative".into()), Some("/home/ada".into())),
            Some(PathBuf::from("/home/ada/.config/bobine/account.toml"))
        );
    }
}
