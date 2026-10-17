//! `bobine`: a native client for films and series. The library holds what the window is built
//! on: the port to a media server and its implementations.

pub mod jellyfin;
pub mod provider;
pub mod requests;
pub mod seerr;

/// The version of this build. `Cargo.toml` stays at 0.0.0: the release workflow names the tag
/// (without its `v`) in `BOBINE_VERSION`, and a local build is `0.0.0-dev`.
pub const VERSION: &str = match option_env!("BOBINE_VERSION") {
    Some(version) => version,
    None => "0.0.0-dev",
};
