//! The first steps: the session kept from the last start, or the login. Every call here blocks
//! on the server or the keyring: the window runs them on its background executor.

use std::sync::Arc;

use bobine::provider::{Connector, MediaServer, ProviderError, ServerInfo};

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

/// What the login screen says under the address once the server answered.
pub fn server_line(info: &ServerInfo) -> String {
    let version: Vec<&str> = info.version.split('.').take(2).collect();
    format!("Jellyfin {} · « {} »", version.join("."), info.name)
}

/// What the user is told, in French. The error's own words stay out: they may name what the
/// user cannot act on.
pub fn error_message(error: &ProviderError) -> &'static str {
    match error {
        ProviderError::InvalidAddress => "L'adresse doit commencer par http:// ou https://.",
        ProviderError::NotAMediaServer => "Aucun serveur Jellyfin ne répond à cette adresse.",
        ProviderError::WrongCredentials => "Nom d'utilisateur ou mot de passe incorrect.",
        ProviderError::AccountDisabled => "Ce compte est désactivé sur le serveur.",
        ProviderError::SessionExpired => "La session a expiré : reconnectez-vous.",
        ProviderError::Forbidden => "Le serveur refuse l'accès à ce contenu.",
        ProviderError::NotFound => "Le serveur ne connaît pas ce contenu.",
        ProviderError::Unreachable(_) => "Le serveur ne répond pas.",
        ProviderError::Server(_) | ProviderError::BadAnswer(_) => {
            "Le serveur a répondu de façon inattendue."
        }
        ProviderError::Keyring(_) => "Le trousseau de clés du système est inaccessible.",
        ProviderError::Storage(_) => "Le fichier du compte ne peut pas être écrit.",
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
    use crate::fake::{FakeServer, PASSWORD, USER};

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

    const ADDRESS: &str = "https://media.exemple.org";

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
    fn a_wrong_password_is_said_in_french() {
        let server = FakeServer::new(3, false);
        let refused = login(&server, ADDRESS, USER, "faux".to_string());
        assert_eq!(
            refused.err(),
            Some("Nom d'utilisateur ou mot de passe incorrect.")
        );
        assert!(matches!(start(&server), Start::Login(None)));
    }

    #[test]
    fn the_address_and_user_are_trimmed() {
        let server = FakeServer::new(3, false);
        login(
            &server,
            " https://media.exemple.org ",
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
        assert_eq!(server_line(&info), "Jellyfin 10.11 · « Salon »");
    }

    #[test]
    fn errors_never_carry_what_the_provider_said() {
        let said = error_message(&ProviderError::Unreachable(
            "media.exemple.org: refused".into(),
        ));
        assert!(!said.contains("media.exemple.org"), "{said}");
    }
}
