//! The first steps: the session kept from the last start, or the login. Every call here blocks
//! on the server or the keyring: the window runs them on its background executor.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use bobine::provider::{Connector, MediaServer, ProviderError, ServerInfo};

use crate::reports;

/// How long the last playback reports are waited for before the logout goes anyway: a slow
/// server must not keep the user from leaving.
const REPORTS_BEFORE_LOGOUT: Duration = Duration::from_secs(5);

/// Where the application opens.
pub enum Start {
    /// The token of the last login is still in the keyring.
    Home(Arc<dyn MediaServer>),
    /// First start, or after a logout: the login screen, with why when something failed.
    Login(Option<&'static str>),
}

pub fn start(connector: &dyn Connector) -> Start {
    match connector.restore() {
        Ok(Some(server)) => Start::Home(server),
        Ok(None) => Start::Login(None),
        Err(error) => Start::Login(Some(error_message(&error))),
    }
}

/// Signs in. The password is taken, and dropped when the call returns: nothing keeps it.
pub fn login(
    connector: &dyn Connector,
    address: &str,
    user: &str,
    password: String,
) -> Result<Arc<dyn MediaServer>, &'static str> {
    connector
        .login(address.trim(), user.trim(), &password)
        .map_err(|error| error_message(&error))
}

/// Leaves the account: the playback's last reports go first (they need the token), then the
/// server is told, the token leaves the keyring and the account file forgets the session. Blocks
/// on the server: the window runs it on its background executor. What it gives back is why the
/// local sign out could not be complete (the keyring, the account file); the server not
/// answering is no reason: the token is dropped here all the same.
pub fn sign_out(server: &dyn MediaServer, reports: Option<JoinHandle<()>>) -> Option<&'static str> {
    if let Some(reports) = reports {
        reports::wait(reports, REPORTS_BEFORE_LOGOUT);
    }
    server.logout().err().map(|error| match error {
        ProviderError::Keyring(_) => {
            "The saved token could not be removed from the system keyring."
        }
        error => error_message(&error),
    })
}

/// What the login screen says under the address once the server answered.
pub fn server_line(info: &ServerInfo) -> String {
    let version: Vec<&str> = info.version.split('.').take(2).collect();
    format!("Jellyfin {} · “{}”", version.join("."), info.name)
}

/// What the user is told. The error's own words stay out: they may name what the
/// user cannot act on.
pub fn error_message(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::InvalidAddress => "The address must start with http:// or https://.",
        ProviderError::NotAMediaServer => "No Jellyfin server responds at this address.",
        ProviderError::WrongCredentials => "Wrong username or password.",
        ProviderError::AccountDisabled => "This account is disabled on the server.",
        ProviderError::SessionExpired => "The session has expired: sign in again.",
        ProviderError::Forbidden => "The server refuses access to this content.",
        ProviderError::NotFound => "This item was not found on the server.",
        ProviderError::Unreachable(_) => "The server is not responding.",
        ProviderError::Server(_) | ProviderError::BadAnswer(_) => {
            "The server gave an unexpected response."
        }
        ProviderError::Keyring(_) => "The system keyring cannot be reached.",
        ProviderError::Storage(_) => "The account file cannot be written.",
    }
}

/// Whether this error brings the login back. Several requests fail with an expired session at
/// once: the form is built for the first, the others leave what the user is typing alone.
pub fn brings_the_login_back(error: &ProviderError, login_shown: bool) -> bool {
    *error == ProviderError::SessionExpired && !login_shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FILM, FakeServer, PASSWORD, Report, USER};
    use crate::player::PlayerEvent;
    use crate::player::fake::FakePlayer;
    use crate::player::session::Session;
    use crate::reports::{self, Reporting};
    use crate::video::fake::FakeDisplay;
    use bobine::provider::{ItemId, MediaSourceId, PlayMethod, PlaySessionId, PlaybackReport};
    use std::time::{Duration, Instant};

    #[test]
    fn the_errors_are_told_in_plain_english() {
        assert_eq!(
            error_message(&ProviderError::NotAMediaServer),
            "No Jellyfin server responds at this address."
        );
        assert_eq!(
            error_message(&ProviderError::Unreachable(String::new())),
            "The server is not responding."
        );
        assert_eq!(
            error_message(&ProviderError::NotFound),
            "This item was not found on the server."
        );
        assert_eq!(
            error_message(&ProviderError::BadAnswer(String::new())),
            "The server gave an unexpected response."
        );
    }

    #[test]
    fn one_expired_session_shows_the_login_once() {
        let expired = ProviderError::SessionExpired;
        assert!(brings_the_login_back(&expired, false));
        // The second and third requests that failed with it.
        assert!(!brings_the_login_back(&expired, true));
    }

    #[test]
    fn only_an_expired_session_brings_the_login_back() {
        assert!(!brings_the_login_back(&ProviderError::NotFound, false));
        assert!(!brings_the_login_back(&ProviderError::Forbidden, false));
    }

    const ADDRESS: &str = "https://media.example.org";

    #[test]
    fn first_start_shows_the_login_then_home() {
        let server = FakeServer::new(3, false);
        assert!(matches!(start(&server), Start::Login(None)));
        let session = login(&server, ADDRESS, USER, PASSWORD.to_string()).unwrap();
        assert_eq!(session.user_name(), USER);
    }

    #[test]
    fn restart_goes_straight_home() {
        let server = FakeServer::new(3, false);
        login(&server, ADDRESS, USER, PASSWORD.to_string()).unwrap();
        match start(&server) {
            Start::Home(session) => assert_eq!(session.user_name(), USER),
            Start::Login(_) => panic!("the kept session was not restored"),
        }
    }

    #[test]
    fn after_logout_the_login_shows_again() {
        let server = FakeServer::new(3, true);
        let Start::Home(session) = start(&server) else {
            panic!("no kept session");
        };
        session.logout().unwrap();
        assert!(matches!(start(&server), Start::Login(None)));
    }

    #[test]
    fn after_signing_out_the_keyring_holds_no_token_and_the_next_start_shows_the_login() {
        let server = FakeServer::new(3, true);
        let Start::Home(session) = start(&server) else {
            panic!("no kept session");
        };
        sign_out(&*session, None);
        assert!(!server.state.lock().unwrap().kept);
        assert!(matches!(start(&server), Start::Login(None)));
    }

    #[test]
    fn signing_out_during_playback_stops_the_player_first() {
        let server = FakeServer::new(3, true);
        // The "stopped" report takes a while to reach the server: a logout sent meanwhile would
        // find the token gone.
        server.state.lock().unwrap().stop_delay = Duration::from_millis(150);
        let player = FakePlayer::default();
        let playing = Session::new(
            Box::new(FakePlayer {
                calls: player.calls.clone(),
                queued: player.queued.clone(),
                info: player.info.clone(),
            }),
            Box::new(FakeDisplay::default()),
            "Le Dernier Hiver".into(),
            None,
        );
        let mut reporting = Reporting::start(
            Arc::new(server.clone()),
            PlaybackReport {
                session: PlaySessionId("session-1".into()),
                film: ItemId(FILM.into()),
                source: MediaSourceId("source-4k".into()),
                position: Duration::ZERO,
                paused: false,
                audio_track: None,
                subtitle_track: None,
                method: PlayMethod::DirectPlay,
            },
        );
        let mut playing = playing;
        player.queued.borrow_mut().push(PlayerEvent::Loaded);
        let ended = playing.update(Instant::now());
        reporting.tick(Instant::now(), &playing, ended);
        // What `Shell::stop_playback` does, then what the sign out waits for.
        let stopping = reports::last_reports(Some(reporting), None, &playing);
        drop(playing);
        let Start::Home(session) = start(&server) else {
            panic!("no kept session");
        };
        assert_eq!(sign_out(&*session, stopping), None);
        let state = server.state.lock().unwrap();
        assert!(matches!(
            state.record.reports.last(),
            Some(Report::Stopped(_))
        ));
        // The one logout came after both the "started" and the "stopped".
        assert_eq!(state.record.logouts, [state.record.reports.len()]);
    }

    #[test]
    fn a_keyring_that_cannot_delete_the_token_is_told_and_the_next_start_shows_the_login() {
        let server = FakeServer::new(3, true);
        server.state.lock().unwrap().logout_error = Some(ProviderError::Keyring(String::new()));
        let Start::Home(session) = start(&server) else {
            panic!("no kept session");
        };
        assert_eq!(
            sign_out(&*session, None),
            Some("The saved token could not be removed from the system keyring.")
        );
        assert!(matches!(start(&server), Start::Login(None)));
    }

    #[test]
    fn a_logout_that_fails_for_another_reason_says_so() {
        let server = FakeServer::new(3, true);
        server.state.lock().unwrap().logout_error = Some(ProviderError::Storage(String::new()));
        let Start::Home(session) = start(&server) else {
            panic!("no kept session");
        };
        assert_eq!(
            sign_out(&*session, None),
            Some("The account file cannot be written.")
        );
    }

    #[test]
    fn a_wrong_password_is_said_in_english() {
        let server = FakeServer::new(3, false);
        let refused = login(&server, ADDRESS, USER, "faux".to_string());
        assert_eq!(refused.err(), Some("Wrong username or password."));
        assert!(matches!(start(&server), Start::Login(None)));
    }

    #[test]
    fn the_address_and_user_are_trimmed() {
        let server = FakeServer::new(3, false);
        login(
            &server,
            " https://media.example.org ",
            " camille",
            PASSWORD.into(),
        )
        .unwrap();
        let logins = server.state.lock().unwrap().record.logins.clone();
        assert_eq!(logins, [(ADDRESS.to_string(), USER.to_string())]);
    }

    #[test]
    fn the_probe_names_the_server_and_its_version() {
        let info = ServerInfo {
            id: "s".into(),
            name: "Salon".into(),
            version: "10.11.2".into(),
        };
        assert_eq!(server_line(&info), "Jellyfin 10.11 · “Salon”");
    }

    #[test]
    fn errors_never_carry_what_the_provider_said() {
        let said = error_message(&ProviderError::Unreachable(
            "media.example.org: refused".into(),
        ));
        assert!(!said.contains("media.example.org"), "{said}");
    }
}
