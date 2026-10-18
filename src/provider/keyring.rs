//! Where the access token is kept between two starts: the system keyring (Secret Service), behind
//! a port so the tests never touch the real one.

use super::{ProviderError, Token};
use secret_service::EncryptionType;
use secret_service::blocking::SecretService;
use std::collections::HashMap;

/// Keeps one token per account (an opaque key the provider chooses).
pub trait TokenStore: Send + Sync {
    fn save(&self, account: &str, token: &Token) -> Result<(), ProviderError>;
    fn load(&self, account: &str) -> Result<Option<Token>, ProviderError>;
    fn delete(&self, account: &str) -> Result<(), ProviderError>;
}

/// The keyring of the session (GNOME Keyring, KWallet…) through the Secret Service D-Bus API.
pub struct SecretServiceStore;

const APPLICATION: &str = "bobine";

fn attributes(account: &str) -> HashMap<&str, &str> {
    HashMap::from([("application", APPLICATION), ("account", account)])
}

fn failed(e: secret_service::Error) -> ProviderError {
    ProviderError::Keyring(e.to_string())
}

impl TokenStore for SecretServiceStore {
    fn save(&self, account: &str, token: &Token) -> Result<(), ProviderError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(failed)?;
        let collection = ss.get_default_collection().map_err(failed)?;
        collection.unlock().map_err(failed)?;
        collection
            .create_item(
                &format!("Bobine ({account})"),
                attributes(account),
                token.expose_secret().as_bytes(),
                true,
                "text/plain",
            )
            .map_err(failed)?;
        Ok(())
    }

    fn load(&self, account: &str) -> Result<Option<Token>, ProviderError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(failed)?;
        let found = ss.search_items(attributes(account)).map_err(failed)?;
        let item = match (found.unlocked.first(), found.locked.first()) {
            (Some(item), _) => item,
            (None, Some(item)) => {
                item.unlock().map_err(failed)?;
                item
            }
            (None, None) => return Ok(None),
        };
        let raw = item.get_secret().map_err(failed)?;
        // A secret that is not text is no token: the user signs in again.
        Ok(String::from_utf8(raw).ok().map(Token::new))
    }

    fn delete(&self, account: &str) -> Result<(), ProviderError> {
        let ss = SecretService::connect(EncryptionType::Dh).map_err(failed)?;
        let found = ss.search_items(attributes(account)).map_err(failed)?;
        for item in found.unlocked.iter().chain(found.locked.iter()) {
            item.unlock().map_err(failed)?;
            item.delete().map_err(failed)?;
        }
        Ok(())
    }
}

/// A keyring in memory, for the tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    pub tokens: std::sync::Mutex<HashMap<String, Token>>,
    /// The keyring cannot be reached when a token is deleted.
    pub delete_fails: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl TokenStore for MemoryStore {
    fn save(&self, account: &str, token: &Token) -> Result<(), ProviderError> {
        let mut tokens = self.tokens.lock().unwrap();
        tokens.insert(account.to_string(), token.clone());
        Ok(())
    }

    fn load(&self, account: &str) -> Result<Option<Token>, ProviderError> {
        Ok(self.tokens.lock().unwrap().get(account).cloned())
    }

    fn delete(&self, account: &str) -> Result<(), ProviderError> {
        if self.delete_fails.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ProviderError::Keyring("locked".into()));
        }
        self.tokens.lock().unwrap().remove(account);
        Ok(())
    }
}
