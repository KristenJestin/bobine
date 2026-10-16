//! Where Bobine keeps the account file (device id, server address, user name: nothing secret),
//! and the viewer's settings beside it.

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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// "Black out other screens": the other monitors turn black while a film plays fullscreen.
    pub black_out_other_monitors: bool,
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
