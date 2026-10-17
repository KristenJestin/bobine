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
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        OpenFocused,
        PlayFocused,
        PaletteUp,
        PaletteDown,
        PaletteOpen,
        MenuConfirm,
    ]
);

/// Where a key is listed in the help.
#[allow(dead_code, reason = "read by the help, which comes next")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Player,
    Browse,
    /// On every screen.
    Both,
    /// The keys of a field (the palette, the playlist menu): not listed.
    Hidden,
}

/// One line of the keymap: its keys, where they fire, and what the help says of them.
#[allow(dead_code, reason = "read by the help, which comes next")]
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
    // Escape fires in a text field too: it closes what is open first.
    key!(
        &["escape"],
        "Player",
        Player,
        "Leave fullscreen, then the player",
        Dismiss
    ),
    key!(&["escape"], "Browse", Browse, "Close or go back", Dismiss),
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
