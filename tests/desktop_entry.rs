//! The desktop entry and the icons packaging installs: the window's app id finds its icon.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn share() -> PathBuf {
    root().join("assets/linux/share")
}

/// The value of `key` in the entry's `[Desktop Entry]` group.
fn field(entry: &str, key: &str) -> Option<String> {
    entry
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .map(str::to_string)
}

#[test]
fn the_desktop_entry_is_named_after_the_window_app_id() {
    let entry = share()
        .join("applications")
        .join(format!("{}.desktop", bobine::APP_ID));
    assert!(entry.is_file(), "{entry:?}");
    let text = fs::read_to_string(entry).unwrap();
    assert_eq!(field(&text, "Type").as_deref(), Some("Application"));
    assert_eq!(field(&text, "Name").as_deref(), Some("Bobine"));
    assert_eq!(field(&text, "Exec").as_deref(), Some("bobine"));
    assert_eq!(
        field(&text, "StartupWMClass").as_deref(),
        Some(bobine::APP_ID)
    );
}

#[test]
fn the_desktop_entry_lists_the_keywords_a_launcher_search_finds_it_by() {
    let entry = share().join("applications/bobine.desktop");
    let text = fs::read_to_string(entry).unwrap();
    let keywords = field(&text, "Keywords").expect("the entry lists keywords");
    // The spec wants a list ending with a semicolon.
    assert!(keywords.ends_with(';'), "{keywords}");
    let listed: Vec<&str> = keywords.split_terminator(';').collect();
    for wanted in [
        "film", "films", "movie", "movies", "series", "tv", "media", "jellyfin",
    ] {
        assert!(listed.contains(&wanted), "{wanted} is not in {keywords}");
    }
}

#[test]
fn the_icon_the_desktop_entry_names_exists_at_every_hicolor_size() {
    let entry = share().join("applications/bobine.desktop");
    let text = fs::read_to_string(entry).unwrap();
    let icon = field(&text, "Icon").expect("the entry names an icon");
    let hicolor = share().join("icons/hicolor");
    for side in [16, 32, 48, 64, 128, 256, 512] {
        let installed = hicolor.join(format!("{side}x{side}/apps/{icon}.png"));
        let brand = root().join(format!("assets/brand/png/icone-{side}.png"));
        assert_eq!(
            fs::read(&installed).unwrap_or_else(|_| panic!("{installed:?} is missing")),
            fs::read(brand).unwrap(),
            "{side}: the brand's render, as delivered"
        );
    }
    let scalable = hicolor.join(format!("scalable/apps/{icon}.svg"));
    assert_eq!(
        fs::read(scalable).unwrap(),
        fs::read(root().join("assets/brand/icone.svg")).unwrap()
    );
}

#[test]
fn the_package_carries_the_entry_and_the_icons() {
    let out = tempfile::tempdir().unwrap();
    let binary = out.path().join("bobine");
    fs::write(&binary, "not a program").unwrap();
    let packed = Command::new(root().join("tools/package.sh"))
        .args(["0.0.0-test"])
        .arg(&binary)
        .arg(out.path().join("dist"))
        .output()
        .unwrap();
    assert!(packed.status.success(), "{packed:?}");
    let listing = Command::new("tar")
        .arg("-tzf")
        .arg(
            out.path()
                .join("dist/bobine-0.0.0-test-linux-x86_64.tar.gz"),
        )
        .output()
        .unwrap();
    let listing = String::from_utf8(listing.stdout).unwrap();
    for wanted in [
        "share/applications/bobine.desktop",
        "share/icons/hicolor/256x256/apps/bobine.png",
        "share/icons/hicolor/scalable/apps/bobine.svg",
    ] {
        assert!(
            listing.lines().any(|line| line.ends_with(wanted)),
            "{wanted}"
        );
    }
}
