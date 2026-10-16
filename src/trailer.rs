//! Which trailer a film's or a series' button plays. A trailer file the server holds comes
//! first; one hosted elsewhere (YouTube…) plays only through mpv's `yt-dlp` hook, so without
//! `yt-dlp` there is no trailer to offer.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use bobine::provider::Trailers;

/// The program mpv's hook runs to turn a page address into a stream.
const YTDLP: &str = "yt-dlp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trailer {
    /// The file the server holds.
    Local,
    /// A page address, played through the `yt-dlp` hook.
    Remote(String),
}

/// The trailer to play, if there is one: `ytdlp` says whether `yt-dlp` is installed.
pub fn choose(trailers: &Trailers, ytdlp: bool) -> Option<Trailer> {
    if trailers.local {
        return Some(Trailer::Local);
    }
    trailers
        .remote
        .clone()
        .filter(|_| ytdlp)
        .map(Trailer::Remote)
}

/// The options of a trailer's mpv: those of every mpv, and the `yt-dlp` hook only for a page
/// address, so that a film's address never goes through `yt-dlp`.
pub fn mpv_options(
    every: &[(&'static str, &'static str)],
    ytdl: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut options = every.to_vec();
    if ytdl {
        options.push(("ytdl", "yes"));
    }
    options
}

/// Whether `yt-dlp` is on this machine's `PATH`. Read once at start: the window asks on every
/// frame whether to show the button.
pub fn ytdlp_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|path| ytdlp_in(&path))
}

/// Whether an executable `yt-dlp` sits in one of the folders of `path` (a `PATH` value).
fn ytdlp_in(path: &OsStr) -> bool {
    std::env::split_paths(path).any(|folder| is_executable(&folder.join(YTDLP)))
}

fn is_executable(file: &Path) -> bool {
    file.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str = "https://video.example/watch?v=abc";

    fn trailers(local: bool, remote: bool) -> Trailers {
        Trailers {
            local,
            remote: remote.then(|| ADDRESS.to_string()),
        }
    }

    #[test]
    fn a_local_trailer_plays() {
        assert_eq!(choose(&trailers(true, false), false), Some(Trailer::Local));
    }

    #[test]
    fn a_local_trailer_comes_before_a_remote_one() {
        assert_eq!(choose(&trailers(true, true), true), Some(Trailer::Local));
        assert_eq!(choose(&trailers(true, true), false), Some(Trailer::Local));
    }

    #[test]
    fn a_remote_trailer_plays_when_ytdlp_is_present() {
        assert_eq!(
            choose(&trailers(false, true), true),
            Some(Trailer::Remote(ADDRESS.into()))
        );
    }

    #[test]
    fn there_is_no_trailer_when_there_is_none() {
        assert_eq!(choose(&trailers(false, false), true), None);
        assert_eq!(choose(&trailers(false, false), false), None);
    }

    #[test]
    fn there_is_no_trailer_when_only_a_remote_one_exists_and_ytdlp_is_missing() {
        assert_eq!(choose(&trailers(false, true), false), None);
    }

    fn folder_with(name: &str, mode: u32) -> tempfile::TempDir {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join(name);
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        folder
    }

    fn path_of(folders: &[&Path]) -> std::ffi::OsString {
        std::env::join_paths(folders).unwrap()
    }

    #[test]
    fn ytdlp_is_found_in_a_folder_of_the_path() {
        let empty = tempfile::tempdir().unwrap();
        let with = folder_with("yt-dlp", 0o755);
        assert!(ytdlp_in(&path_of(&[empty.path(), with.path()])));
    }

    #[test]
    fn ytdlp_is_missing_from_a_path_that_does_not_hold_it() {
        let empty = tempfile::tempdir().unwrap();
        let other = folder_with("mpv", 0o755);
        assert!(!ytdlp_in(&path_of(&[empty.path(), other.path()])));
        assert!(!ytdlp_in(OsStr::new("")));
    }

    #[test]
    fn a_ytdlp_that_cannot_run_does_not_count() {
        let not_executable = folder_with("yt-dlp", 0o644);
        assert!(!ytdlp_in(&path_of(&[not_executable.path()])));
    }
}
