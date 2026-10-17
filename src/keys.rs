//! Every key of the window, once: what binds them and what the help shows are both read from
//! `KEYS`, so the help can never say a key the window does not have.
//!
//! The root of the window names the screen in its key context (`Player` or `Browse`), adds
//! `Modal` while something over the page takes the keys (the help, the palette, the playlist
//! menu), and a text field adds `TextInput` under it. While a text field has the focus, only
//! Escape (and the palette's own keys) fire.

use gpui_kit::{App, KeyBinding, actions};

actions!(
    bobine,
    [
        PlayPause,
        SeekBack,
        SeekForward,
        SeekBackLong,
        SeekForwardLong,
        VolumeUp,
        VolumeDown,
        Mute,
        Fullscreen,
        CycleSubtitles,
        CycleAudio,
        PlayNext,
        SkipSegment,
        SubtitlesEarlier,
        SubtitlesLater,
        Slower,
        Faster,
        Dismiss,
        Search,
        Back,
        Pill1,
        Pill2,
        Pill3,
        Pill4,
        Pill5,
        Pill6,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        OpenFocused,
        PlayFocused,
        Help,
        PaletteUp,
        PaletteDown,
        PaletteOpen,
        MenuConfirm,
    ]
);

/// Where a key is listed in the help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Player,
    Browse,
    /// On every screen.
    Both,
    /// The keys of a field (the palette, the playlist menu), or another layout's spelling of
    /// a listed key: not listed.
    Hidden,
}

/// One line of the keymap: its keys, where they fire, and what the help says of them.
pub struct Key {
    /// GPUI keystrokes, as `KeyBinding` reads them.
    pub keys: &'static [&'static str],
    /// The key context they fire in.
    pub context: &'static str,
    pub scope: Scope,
    pub what: &'static str,
    bind: fn(&str, &str) -> KeyBinding,
}

const PLAYER: &str = "Player && !TextInput && !Modal";
const BROWSE: &str = "Browse && !TextInput && !Modal";

macro_rules! key {
    ($keys:expr, $context:expr, $scope:ident, $what:expr, $action:ident) => {
        Key {
            keys: $keys,
            context: $context,
            scope: Scope::$scope,
            what: $what,
            bind: |keys, context| KeyBinding::new(keys, $action, Some(context)),
        }
    };
}

pub const KEYS: &[Key] = &[
    key!(&["space", "k"], PLAYER, Player, "Play or pause", PlayPause),
    key!(&["left", "j"], PLAYER, Player, "Back 10 seconds", SeekBack),
    key!(
        &["right", "l"],
        PLAYER,
        Player,
        "Forward 10 seconds",
        SeekForward
    ),
    key!(
        &["shift-left"],
        PLAYER,
        Player,
        "Back 1 minute",
        SeekBackLong
    ),
    key!(
        &["shift-right"],
        PLAYER,
        Player,
        "Forward 1 minute",
        SeekForwardLong
    ),
    key!(&["up"], PLAYER, Player, "Volume up", VolumeUp),
    key!(&["down"], PLAYER, Player, "Volume down", VolumeDown),
    key!(&["m"], PLAYER, Player, "Mute", Mute),
    key!(&["f"], PLAYER, Player, "Fullscreen", Fullscreen),
    key!(&["c"], PLAYER, Player, "Next subtitles", CycleSubtitles),
    key!(&["a"], PLAYER, Player, "Next audio track", CycleAudio),
    key!(&["n"], PLAYER, Player, "Next episode", PlayNext),
    key!(
        &["s", "enter"],
        PLAYER,
        Player,
        "Skip intro or credits",
        SkipSegment
    ),
    key!(
        &["["],
        PLAYER,
        Player,
        "Subtitles earlier",
        SubtitlesEarlier
    ),
    key!(&["]"], PLAYER, Player, "Subtitles later", SubtitlesLater),
    key!(&["<"], PLAYER, Player, "Slower", Slower),
    key!(&[">"], PLAYER, Player, "Faster", Faster),
    key!(&["left"], BROWSE, Browse, "Previous title", FocusLeft),
    key!(&["right"], BROWSE, Browse, "Next title", FocusRight),
    key!(&["up"], BROWSE, Browse, "Row above", FocusUp),
    key!(&["down"], BROWSE, Browse, "Row below", FocusDown),
    key!(&["enter"], BROWSE, Browse, "Open the title", OpenFocused),
    key!(&["p"], BROWSE, Browse, "Play the title", PlayFocused),
    key!(&["ctrl-k"], BROWSE, Browse, "Search", Search),
    key!(&["alt-left", "backspace"], BROWSE, Browse, "Back", Back),
    key!(&["alt-1"], BROWSE, Browse, "Home", Pill1),
    key!(&["alt-2"], BROWSE, Browse, "Movies", Pill2),
    key!(&["alt-3"], BROWSE, Browse, "Series", Pill3),
    key!(&["alt-4"], BROWSE, Browse, "Collections", Pill4),
    key!(&["alt-5"], BROWSE, Browse, "Lists", Pill5),
    key!(&["alt-6"], BROWSE, Browse, "Discover", Pill6),
    // The same on AZERTY, whose number row gives these without Shift: one line in the help.
    key!(&["alt-&"], BROWSE, Hidden, "Home", Pill1),
    key!(&["alt-eacute"], BROWSE, Hidden, "Movies", Pill2),
    key!(&["alt-\""], BROWSE, Hidden, "Series", Pill3),
    key!(&["alt-'"], BROWSE, Hidden, "Collections", Pill4),
    key!(&["alt-("], BROWSE, Hidden, "Lists", Pill5),
    key!(&["alt--"], BROWSE, Hidden, "Discover", Pill6),
    // Escape fires in a text field too: it closes what is open first.
    key!(
        &["escape"],
        "Player",
        Player,
        "Leave fullscreen, then the player",
        Dismiss
    ),
    key!(&["escape"], "Browse", Browse, "Close or go back", Dismiss),
    key!(
        &["?"],
        "Bobine && !TextInput",
        Both,
        "Show or hide the keys",
        Help
    ),
    // The palette's keys, its field having the focus; Ctrl+K closes it again.
    key!(&["ctrl-k"], "Palette", Hidden, "Close the search", Search),
    key!(&["up"], "Palette", Hidden, "Previous result", PaletteUp),
    key!(&["down"], "Palette", Hidden, "Next result", PaletteDown),
    key!(
        &["enter"],
        "Palette",
        Hidden,
        "Open the result",
        PaletteOpen
    ),
    key!(
        &["enter"],
        "PlaylistMenu",
        Hidden,
        "Make the playlist",
        MenuConfirm
    ),
];

/// Binds every key of `KEYS`.
pub fn bind(cx: &mut App) {
    let bindings = KEYS.iter().flat_map(|key| {
        key.keys
            .iter()
            .map(move |keys| (key.bind)(keys, key.context))
    });
    cx.bind_keys(bindings);
}

/// The lines the help shows on the player (`player`) or on another screen.
pub fn listed(player: bool) -> impl Iterator<Item = &'static Key> {
    let shown = if player { Scope::Player } else { Scope::Browse };
    KEYS.iter()
        .filter(move |key| key.scope == shown || key.scope == Scope::Both)
}

/// The caps a keystroke is drawn as: "shift-left" is "Shift" and "←".
pub fn caps(keystroke: &str) -> Vec<String> {
    // "-" alone, or a binding of "ctrl--", ends in the key itself.
    let mut parts: Vec<&str> = keystroke.split('-').collect();
    if keystroke.ends_with('-') {
        parts.retain(|part| !part.is_empty());
        parts.push("-");
    }
    parts
        .into_iter()
        .map(|part| match part {
            "ctrl" => "Ctrl".into(),
            "alt" => "Alt".into(),
            "shift" => "Shift".into(),
            "space" => "Space".into(),
            "enter" => "Enter".into(),
            "escape" => "Esc".into(),
            "backspace" => "Backspace".into(),
            "left" => "←".into(),
            "right" => "→".into(),
            "up" => "↑".into(),
            "down" => "↓".into(),
            key => key.to_uppercase(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keystroke_is_drawn_as_its_caps() {
        assert_eq!(caps("shift-left"), ["Shift", "←"]);
        assert_eq!(caps("ctrl-k"), ["Ctrl", "K"]);
        assert_eq!(caps("alt-1"), ["Alt", "1"]);
        assert_eq!(caps("?"), ["?"]);
        assert_eq!(caps("escape"), ["Esc"]);
    }

    #[test]
    fn the_help_of_the_player_lists_its_keys_and_not_the_pages_ones() {
        let whats: Vec<&str> = listed(true).map(|key| key.what).collect();
        assert!(whats.contains(&"Play or pause"));
        assert!(whats.contains(&"Show or hide the keys"));
        assert!(!whats.contains(&"Search"));
        let whats: Vec<&str> = listed(false).map(|key| key.what).collect();
        assert!(whats.contains(&"Search"));
        assert!(!whats.contains(&"Play or pause"));
        assert!(
            !whats.contains(&"Previous result"),
            "a field's keys are not listed"
        );
    }
}
