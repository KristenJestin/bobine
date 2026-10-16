//! The icons: filled ones from Phosphor (MIT licence), each an SVG under `assets/icons/` embedded
//! in the binary. The licence notice is in `THIRD-PARTY-NOTICES.md`.
//! The names are Lucide's (what the screens already say), the drawings are Phosphor's "fill"
//! (or "bold" where a fill would be a box with the symbol cut out).
//! An icon is a square the size of the text beside it, centred, coloured like that text.

use std::borrow::Cow;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, AssetSource, Div, IntoElement, RenderOnce, SharedString, Window, div, px, svg,
};

macro_rules! icons {
    ($($name:ident => $file:literal),* $(,)?) => {
        /// The icons the program draws.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, IntoElement)]
        pub enum IconName {
            $($name),*
        }

        impl IconName {
            /// Every icon, for the asset source to look one up by its path.
            const ALL: &[IconName] = &[$(Self::$name),*];

            /// Where the asset source finds this icon.
            pub fn path(self) -> SharedString {
                match self {
                    $(Self::$name => concat!("icons/", $file, ".svg").into()),*
                }
            }

            fn svg(self) -> &'static [u8] {
                match self {
                    $(Self::$name => include_bytes!(concat!("../assets/icons/", $file, ".svg"))),*
                }
            }
        }
    };
}

icons! {
    Play => "play",
    Pause => "pause",
    RotateCcw => "rotate-ccw",
    Check => "check",
    CheckCircle => "check-circle",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ChevronDown => "chevron-down",
    ChevronUp => "chevron-up",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    Plus => "plus",
    Search => "search",
    X => "x",
    List => "list",
    Volume2 => "volume-2",
    VolumeX => "volume-x",
    Maximize => "maximize",
    Minimize => "minimize",
    Star => "star",
    Circle => "circle",
}

/// The asset source of the window: the icons above and nothing else.
#[derive(Clone, Copy, Debug, Default)]
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        Ok(IconName::ALL
            .iter()
            .find(|name| name.path() == path)
            .map(|name| Cow::Borrowed(name.svg())))
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        Ok(IconName::ALL
            .iter()
            .map(|name| name.path())
            .filter(|icon| icon.starts_with(path))
            .collect())
    }
}

impl RenderOnce for IconName {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let text_style = window.text_style();
        svg()
            .path(self.path())
            .flex_shrink_0()
            .size(text_style.font_size.to_pixels(window.rem_size()))
            .text_color(text_style.color)
    }
}

/// `name`, `size` pixels square, centred, in the colour of the text around it.
pub fn icon(name: IconName, size: f32) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size))
        .child(name)
}

/// A piece of a line that holds ratings: its text, or the star that begins a rating.
#[derive(Debug, PartialEq, Eq)]
pub enum Run<'a> {
    Text(&'a str),
    Star,
}

/// A line cut at each `★` the data writes before a rating, so that the star is drawn as an
/// icon: a glyph of the font is an outline, whatever the font.
pub fn runs(text: &str) -> Vec<Run<'_>> {
    let mut runs = Vec::new();
    for (at, piece) in text.split('★').enumerate() {
        if at > 0 {
            runs.push(Run::Star);
        }
        let piece = piece.trim();
        if !piece.is_empty() {
            runs.push(Run::Text(piece));
        }
    }
    runs
}

/// `text` on a line, each rating's `★` drawn as the star icon, `size` pixels, in the colour of
/// the text.
pub fn with_stars(text: &str, size: f32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .min_w_0()
        .overflow_hidden()
        .children(runs(text).into_iter().map(|run| match run {
            // The text gives way, with its ellipsis, when the room is short: the star does not.
            Run::Text(piece) => div().min_w_0().truncate().child(piece.to_string()),
            Run::Star => icon(IconName::Star, size),
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rating_is_cut_into_its_star_and_its_number() {
        assert_eq!(runs("★ 7.8"), [Run::Star, Run::Text("7.8")]);
    }

    #[test]
    fn a_line_with_a_rating_in_it_is_cut_around_the_star() {
        assert_eq!(
            runs("2025 · ★ 6.0 · commencé"),
            [Run::Text("2025 ·"), Run::Star, Run::Text("6.0 · commencé"),]
        );
    }

    #[test]
    fn a_line_without_a_star_stays_one_piece() {
        assert_eq!(runs("2025 · commencé"), [Run::Text("2025 · commencé")]);
        assert_eq!(runs(""), []);
    }

    #[test]
    fn every_icon_drawn_is_embedded() {
        for name in IconName::ALL {
            let svg = Icons.load(&name.path()).unwrap();
            assert!(
                svg.is_some_and(|bytes| bytes.starts_with(b"<svg")),
                "{name:?}"
            );
        }
    }

    #[test]
    fn every_icon_drawn_is_filled_not_outlined() {
        for name in IconName::ALL {
            let svg = Icons.load(&name.path()).unwrap().unwrap();
            let svg = std::str::from_utf8(&svg).unwrap();
            assert!(!svg.contains("stroke"), "{name:?} is drawn as an outline");
        }
    }

    #[test]
    fn the_search_glass_is_phosphors_bold_one_with_an_open_lens() {
        // The fill weight has a solid lens, which reads as a coloured emoji: the bold weight is
        // a ring and a handle, drawn as one outline-shaped fill.
        let svg = Icons.load(&IconName::Search.path()).unwrap().unwrap();
        let svg = std::str::from_utf8(&svg).unwrap();
        assert!(svg.contains("M232.49,215.51,185,168a92.12"), "{svg}");
        assert!(!svg.contains("M168,112a56,56"), "the solid lens is back");
    }

    #[test]
    fn an_unknown_path_is_not_an_icon() {
        assert!(Icons.load("icons/nothing.svg").unwrap().is_none());
    }

    #[test]
    fn the_icons_listed_are_those_embedded() {
        let listed = Icons.list("icons/").unwrap();
        assert_eq!(listed.len(), IconName::ALL.len());
    }
}
