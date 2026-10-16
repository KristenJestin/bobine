//! The logic of Discover and the request pages (#63): what a title's pill says, what is in the
//! library, which seasons a request offers, and the lines that say how a request went. No window
//! here: the views read it, the tests drive it.

use bobine::requests::{RequestError, ServerCheck};

/// A failure of the request server, in a line of the page.
pub fn error_line(error: &RequestError) -> &'static str {
    match error {
        RequestError::InvalidAddress => "The address must start with http:// or https://.",
        RequestError::NotARequestServer => "No Seerr server answers at this address.",
        RequestError::KeyRefused => "Seerr refused the API key.",
        RequestError::Quota => "Your request quota is reached.",
        RequestError::Forbidden => "Seerr refused the request.",
        RequestError::AlreadyRequested => "It was requested already.",
        RequestError::NotFound => "Seerr does not know this title.",
        RequestError::Unreachable(_) => "Seerr cannot be reached.",
        RequestError::Server(_) | RequestError::BadAnswer(_) => "Seerr gave an unexpected answer.",
    }
}

/// What a test of the connection says, and the line that says whom requests go as when it is
/// not the viewer.
pub fn check_lines(check: &ServerCheck) -> (String, Option<String>) {
    let owner = match &check.acting_as {
        bobine::requests::ActingAs::Viewer(_) => None,
        bobine::requests::ActingAs::Owner(name) => Some(format!(
            "No Seerr user has your Jellyfin account: requests go as {name}."
        )),
    };
    (format!("Connected to Seerr {}.", check.version), owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bobine::requests::ActingAs;

    #[test]
    fn the_test_of_a_connection_says_the_version_and_whom_requests_go_as() {
        let viewer = ServerCheck {
            version: "2.7.3".into(),
            acting_as: ActingAs::Viewer("camille".into()),
        };
        assert_eq!(
            check_lines(&viewer),
            ("Connected to Seerr 2.7.3.".into(), None)
        );
        let owner = ServerCheck {
            acting_as: ActingAs::Owner("Owner".into()),
            ..viewer
        };
        assert_eq!(
            check_lines(&owner).1.as_deref(),
            Some("No Seerr user has your Jellyfin account: requests go as Owner.")
        );
    }
}
