//! `Cargo.toml` stays at 0.0.0: a release build names its version in `BOBINE_VERSION` (the tag
//! without its `v`), a local build is `0.0.0-dev`.

use std::process::Command;

#[test]
fn the_version_comes_from_the_build_not_from_cargo_toml() {
    let expected = option_env!("BOBINE_VERSION").unwrap_or("0.0.0-dev");
    assert_eq!(bobine::VERSION, expected);
}

#[test]
fn version_on_the_command_line_prints_it_and_opens_no_window() {
    let output = Command::new(env!("CARGO_BIN_EXE_bobine"))
        .arg("--version")
        // No display: if this opened a window it would fail instead of printing.
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("bobine {}\n", bobine::VERSION)
    );
}
