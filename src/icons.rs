//! The icons, from Lucide (ISC licence) as GPUI Kit bundles them: only those drawn are embedded.
//! An icon is a square the size of the text beside it, centred, coloured like that text.

pub use gpui_kit::assets::IconName;
use gpui_kit::assets::icon_assets;
use gpui_kit::prelude::*;
use gpui_kit::{Div, div, px};

icon_assets!(pub Icons, [Play, Pause, RotateCcw, Check, ArrowLeft, ChevronDown]);

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

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::AssetSource;

    #[test]
    fn every_icon_drawn_is_embedded() {
        for name in [
            IconName::Play,
            IconName::Pause,
            IconName::RotateCcw,
            IconName::Check,
            IconName::ArrowLeft,
            IconName::ChevronDown,
        ] {
            let svg = Icons.load(&name.path()).unwrap();
            assert!(
                svg.is_some_and(|bytes| bytes.starts_with(b"<svg")),
                "{name:?}"
            );
        }
    }
}
