//! The account file: what Bobine keeps of a Jellyfin login between two starts, nothing secret.
//! The device id stays across logouts, so the server sees the same device sign in again.

use crate::provider::ProviderError;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountFile {
    pub device_id: String,
    pub login: Option<Login>,
    /// The address of the server of the last login, kept after a logout to prefill the next one.
    #[serde(default)]
    pub last_address: Option<String>,
}

/// The signed-in user and their server; the token is in the keyring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Login {
    pub address: String,
    pub server_id: String,
    pub server_name: String,
    pub server_version: String,
    pub user_id: String,
    pub user_name: String,
}

impl Login {
    /// The keyring entry of this login.
    pub fn keyring_account(&self) -> String {
        format!("jellyfin:{}:{}", self.server_id, self.user_id)
    }
}

fn failed(path: &Path, what: &str, e: impl std::fmt::Display) -> ProviderError {
    ProviderError::Storage(format!("{what} {}: {e}", path.display()))
}

/// A device id, 32 random hexadecimal digits.
fn new_device_id() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| ProviderError::Storage(format!("no randomness for the device id: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl AccountFile {
    /// Reads the file, or makes it with a new device id when there is none.
    pub fn open(path: &Path) -> Result<Self, ProviderError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| failed(path, "cannot read", e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let account = Self {
                    device_id: new_device_id()?,
                    login: None,
                    last_address: None,
                };
                account.save(path)?;
                Ok(account)
            }
            Err(e) => Err(failed(path, "cannot read", e)),
        }
    }

    /// Writes the whole file anew, through a temporary file so a crash leaves the old one.
    pub fn save(&self, path: &Path) -> Result<(), ProviderError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| failed(dir, "cannot create", e))?;
        }
        let text = toml::to_string(self).map_err(|e| failed(path, "cannot write", e))?;
        let mut tmp = PathBuf::from(path);
        tmp.as_mut_os_string().push(".tmp");
        std::fs::write(&tmp, text).map_err(|e| failed(&tmp, "cannot write", e))?;
        std::fs::rename(&tmp, path).map_err(|e| failed(path, "cannot write", e))
    }
}
