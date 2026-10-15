//! Where Bobine keeps the account file (device id, server address, user name: nothing secret).

use std::ffi::OsString;
use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/bobine/account.toml`, else `~/.config/bobine/account.toml`. A relative
/// `XDG_CONFIG_HOME` is ignored, as the XDG specification says.
pub fn account_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let config = xdg_config_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("bobine").join("account.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
