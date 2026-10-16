//! The two values that carry the server's access token. Neither has a `Display`, so `{}` does not
//! compile on them, and their `Debug` says what they are without what they hold: the token cannot
//! reach a log line, an error or a panic by being formatted.
//!
//! ```compile_fail
//! let token = bobine::provider::Token::new("secret".into());
//! let _ = format!("{token}");
//! ```
//!
//! ```compile_fail
//! let url = bobine::provider::StreamUrl::new("https://example.invalid/?api_key=secret".into());
//! let _ = format!("{url}");
//! ```

use std::fmt;

/// A secret kept in the keyring: the access token the server gave at login, and the API keys of
/// the integrations.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The token itself, for the request header and the keyring only.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// A URL that carries the token in its query (`ApiKey`), because the player opens it itself.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamUrl(String);

impl StreamUrl {
    pub fn new(url: String) -> Self {
        Self(url)
    }

    /// The URL itself, only where the player receives it.
    pub fn expose_for_player(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for StreamUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamUrl(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_never_in_debug_output() {
        let token = Token::new("tok-0123456789abcdef".into());
        let shown = format!("{token:?} {token:#?} {:?}", Some(token.clone()));
        assert!(!shown.contains("0123456789abcdef"), "{shown}");
    }

    #[test]
    fn a_stream_url_never_in_debug_output() {
        let url = StreamUrl::new("http://media.invalid/Videos/1/stream?api_key=tok-0123".into());
        let shown = format!("{url:?} {url:#?}");
        assert!(!shown.contains("tok-0123"), "{shown}");
        assert!(!shown.contains("media.invalid"), "{shown}");
    }
}
