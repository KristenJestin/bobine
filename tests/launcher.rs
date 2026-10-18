//! The launcher's commands as a process, with no display: a window would fail to open instead of
//! answering. Their folders are temporary; none of them reaches a keyring, a session bus or a
//! server. What needs a session (the lists, a server unreachable) is tried in `src/launcher.rs`
//! on the fake server: the program reaches a token only through the real keyring.

use std::io::Read;
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

use serde_json::Value;

/// `bobine` with `args`, its configuration in `config`, its runtime folder `runtime`.
fn bobine(args: &[&str], config: &std::path::Path, runtime: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bobine"))
        .args(args)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("XDG_CONFIG_HOME", config)
        .env("HOME", config)
        .env("XDG_RUNTIME_DIR", runtime)
        .output()
        .unwrap()
}

fn entries(output: &Output) -> Vec<Value> {
    let text = String::from_utf8(output.stdout.clone()).unwrap();
    match serde_json::from_str(&text) {
        Ok(Value::Array(entries)) => entries,
        _ => panic!("not a JSON array: {text}"),
    }
}

#[test]
fn not_signed_in_exits_with_its_own_code_and_a_json_message() {
    let config = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    for args in [&["home", "--json"][..], &["search", "--json", "hiver"]] {
        let output = bobine(args, config.path(), runtime.path());
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        let said = entries(&output);
        assert_eq!(said.len(), 1);
        assert_eq!(said[0]["name"], "Sign in to Bobine");
    }
}

#[test]
fn a_command_bobine_does_not_know_is_a_usage_error_in_json() {
    let config = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    for args in [&["frobnicate"][..], &["open", "album:a1"], &["play"]] {
        let output = bobine(args, config.path(), runtime.path());
        assert_eq!(output.status.code(), Some(4), "{args:?}");
        assert_eq!(entries(&output).len(), 1);
    }
}

#[test]
fn open_movie_with_a_window_running_opens_no_second_window() {
    let config = tempfile::tempdir().unwrap();
    let runtime = tempfile::tempdir().unwrap();
    // The window running, as the command sees it: something listening on the socket.
    let window = UnixListener::bind(runtime.path().join("bobine.sock")).unwrap();
    let output = bobine(&["open", "movie:film-7"], config.path(), runtime.path());
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    let (mut stream, _) = window.accept().unwrap();
    let mut asked = String::new();
    stream.read_to_string(&mut asked).unwrap();
    assert_eq!(asked, "bobine:open/movie/film-7\n");
}
