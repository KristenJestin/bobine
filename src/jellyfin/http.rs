//! The requests to a Jellyfin server: the `Authorization` header that names the client and carries
//! the token, and errors told without the URL (the address, and the token of a stream URL).

use crate::provider::{ProviderError, Token};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::time::Duration;
use ureq::http::Response;

const TIMEOUT: Duration = Duration::from_secs(15);
/// The server may convert an image before answering (a person's photo): it gets longer than an
/// API call, which must stay quick to report an unreachable server.
const IMAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a download may receive nothing before it is given up.
const IDLE_LIMIT: Duration = Duration::from_secs(30);
const CLIENT: &str = "Bobine";

/// The device this installation is to the server.
#[derive(Debug, Clone)]
pub struct Device {
    /// Stable per installation: the server keys its sessions by it.
    pub id: String,
    pub name: String,
}

/// A Jellyfin server at one address, with or without a token.
pub struct Http {
    agent: ureq::Agent,
    image_timeout: Duration,
    base: String,
    device: Device,
    token: Option<Token>,
}

/// A server address as typed, trimmed and without its final slashes, when it can be one.
pub fn normalize_address(typed: &str) -> Result<String, ProviderError> {
    let address = typed.trim().trim_end_matches('/');
    let host = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))
        .ok_or(ProviderError::InvalidAddress)?;
    if host.is_empty() || host.contains(char::is_whitespace) {
        return Err(ProviderError::InvalidAddress);
    }
    Ok(address.to_string())
}

/// Percent-encodes everything but the unreserved characters, for a value in a URL.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// What failed in words, without the URL that `ureq`'s own messages carry.
pub(crate) fn transport(what: &str, e: ureq::Error) -> ProviderError {
    let why = match e {
        ureq::Error::Io(io) => io.kind().to_string(),
        ureq::Error::Timeout(_) => "timed out".to_string(),
        ureq::Error::HostNotFound => "host not found".to_string(),
        ureq::Error::ConnectionFailed => "connection failed".to_string(),
        ureq::Error::Tls(why) => format!("TLS: {why}"),
        ureq::Error::Rustls(tls) => format!("TLS: {tls}"),
        _ => "the request failed".to_string(),
    };
    ProviderError::Unreachable(format!("{what}: {why}"))
}

fn status(code: u16) -> ProviderError {
    match code {
        // Only 401 says the token is no longer good; 403 is this user refused one thing.
        401 => ProviderError::SessionExpired,
        403 => ProviderError::Forbidden,
        404 => ProviderError::NotFound,
        _ => ProviderError::Server(code),
    }
}

impl Http {
    pub fn new(base: String, device: Device, token: Option<Token>) -> Self {
        Self::with_timeouts(base, device, token, TIMEOUT, IMAGE_TIMEOUT, IDLE_LIMIT)
    }

    fn with_timeouts(
        base: String,
        device: Device,
        token: Option<Token>,
        timeout: Duration,
        image_timeout: Duration,
        _idle_limit: Duration,
    ) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .user_agent(format!("{CLIENT}/{}", crate::VERSION))
            .build()
            .into();
        Self {
            agent,
            image_timeout,
            base,
            device,
            token,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn device(&self) -> &Device {
        &self.device
    }

    pub fn token(&self) -> Option<&Token> {
        self.token.as_ref()
    }

    /// `MediaBrowser Client="Bobine", Device=…, DeviceId=…, Version=…[, Token=…]`.
    fn authorization(&self) -> String {
        let quoted = |s: &str| s.replace(['"', ','], "");
        let mut header = format!(
            "MediaBrowser Client=\"{CLIENT}\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
            quoted(&self.device.name),
            quoted(&self.device.id),
            crate::VERSION,
        );
        if let Some(token) = &self.token {
            header.push_str(&format!(", Token=\"{}\"", token.expose_secret()));
        }
        header
    }

    fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("{}{path}", self.base);
        for (i, (key, value)) in query.iter().enumerate() {
            url.push(if i == 0 { '?' } else { '&' });
            url.push_str(key);
            url.push('=');
            url.push_str(&encode(value));
        }
        url
    }

    fn checked(
        what: &str,
        sent: Result<Response<ureq::Body>, ureq::Error>,
    ) -> Result<Response<ureq::Body>, ProviderError> {
        let response = sent.map_err(|e| transport(what, e))?;
        let code = response.status().as_u16();
        if (200..300).contains(&code) {
            Ok(response)
        } else {
            Err(status(code))
        }
    }

    fn json<T: DeserializeOwned>(
        what: &str,
        mut response: Response<ureq::Body>,
    ) -> Result<T, ProviderError> {
        response
            .body_mut()
            .read_json()
            .map_err(|_| ProviderError::BadAnswer(format!("{what}: not the expected JSON")))
    }

    pub fn get<T: DeserializeOwned>(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ProviderError> {
        let sent = self
            .agent
            .get(&self.url(path, query))
            .header("Authorization", &self.authorization())
            .call();
        Self::json(what, Self::checked(what, sent)?)
    }

    /// An image, which gets the longer image timeout.
    pub fn get_bytes(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<u8>, ProviderError> {
        let sent = self
            .agent
            .get(&self.url(path, query))
            .header("Authorization", &self.authorization())
            .config()
            .timeout_global(Some(self.image_timeout))
            .build()
            .call();
        Self::checked(what, sent)?
            .body_mut()
            .with_config()
            .limit(32 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| transport(what, e))
    }

    /// A file to download, from the headers given (`Range`, `If-Range`): its answer, the body
    /// not read yet. No time limit on the whole, a film takes what it takes; only on the wait for
    /// the server's answer.
    pub fn download(
        &self,
        what: &str,
        path: &str,
        headers: &[(&str, &str)],
    ) -> Result<Response<ureq::Body>, ProviderError> {
        let mut request = self
            .agent
            .get(&self.url(path, &[]))
            .header("Authorization", &self.authorization());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let sent = request
            .config()
            .timeout_global(None)
            .timeout_connect(Some(TIMEOUT))
            .timeout_recv_response(Some(TIMEOUT))
            .build()
            .call();
        Self::checked(what, sent)
    }

    /// Posts a JSON body; the answer is read as JSON.
    pub fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
        body: &B,
    ) -> Result<T, ProviderError> {
        Self::json(what, self.post_raw(what, path, query, body)?)
    }

    /// Posts a JSON body; the answer, often empty, is not read.
    pub fn send<B: Serialize>(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
        body: &B,
    ) -> Result<(), ProviderError> {
        self.post_raw(what, path, query, body).map(drop)
    }

    fn post_raw<B: Serialize>(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
        body: &B,
    ) -> Result<Response<ureq::Body>, ProviderError> {
        let sent = self
            .agent
            .post(&self.url(path, query))
            .header("Authorization", &self.authorization())
            .send_json(body);
        Self::checked(what, sent)
    }

    /// Posts without a body; the answer is not read.
    pub fn post_empty(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<(), ProviderError> {
        let sent = self
            .agent
            .post(&self.url(path, query))
            .header("Authorization", &self.authorization())
            .send_empty();
        Self::checked(what, sent).map(drop)
    }

    pub fn delete(
        &self,
        what: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<(), ProviderError> {
        let sent = self
            .agent
            .delete(&self.url(path, query))
            .header("Authorization", &self.authorization())
            .call();
        Self::checked(what, sent).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_image_still_arrives_after_the_api_timeout() {
        use crate::jellyfin::fake::{FakeServer, IMAGE, TOKEN};
        let fake = FakeServer::start();
        fake.state.lock().unwrap().image_delay = Duration::from_millis(1500);
        let device = Device {
            id: "device".into(),
            name: "Test".into(),
        };
        let token = Some(Token::new(TOKEN.into()));
        let path = "/Items/f0000000000000000000000000000001/Images/Primary";
        let http = Http::with_timeouts(
            fake.url.clone(),
            device.clone(),
            token.clone(),
            Duration::from_secs(1),
            Duration::from_secs(10),
            IDLE_LIMIT,
        );
        assert_eq!(http.get_bytes("image", path, &[]).unwrap(), IMAGE);
        // Not forever: an image still takes at most its own timeout.
        let http = Http::with_timeouts(
            fake.url.clone(),
            device,
            token,
            Duration::from_secs(10),
            Duration::from_secs(1),
            IDLE_LIMIT,
        );
        assert!(matches!(
            http.get_bytes("image", path, &[]),
            Err(ProviderError::Unreachable(_))
        ));
    }

    #[test]
    fn a_body_that_stalls_fails_instead_of_hanging() {
        use crate::jellyfin::fake::{FakeServer, SOURCE, TOKEN};
        use std::io::Read;
        let fake = FakeServer::start();
        fake.state.lock().unwrap().stall_body = true;
        let device = Device {
            id: "device".into(),
            name: "Test".into(),
        };
        let http = Http::with_timeouts(
            fake.url.clone(),
            device,
            Some(Token::new(TOKEN.into())),
            TIMEOUT,
            IMAGE_TIMEOUT,
            Duration::from_millis(300),
        );
        let path = &format!("/Items/{SOURCE}/Download");
        let started = std::time::Instant::now();
        let mut response = http.download("download", path, &[]).unwrap();
        let mut body = Vec::new();
        let read = response.body_mut().as_reader().read_to_end(&mut body);
        assert!(read.is_err(), "a stalled body is not a whole one");
        assert!(
            started.elapsed() < Duration::from_millis(1500),
            "given up after the idle limit, not when the server lets go: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn an_address_is_trimmed_and_needs_a_scheme_and_a_host() {
        assert_eq!(
            normalize_address(" https://media.example.invalid:8096/ "),
            Ok("https://media.example.invalid:8096".to_string())
        );
        assert_eq!(
            normalize_address("media.example.invalid"),
            Err(ProviderError::InvalidAddress)
        );
        assert_eq!(
            normalize_address("http://"),
            Err(ProviderError::InvalidAddress)
        );
    }
}
