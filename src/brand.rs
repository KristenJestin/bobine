//! Bobine's mark, drawn where the program shows its name. The drawing is the maintainer's
//! (`assets/brand/`), embedded as the PNG renders of the icon: a PNG whose side is the window's
//! pixels for the mark stays sharp, where a small copy stretched, or a big one squeezed, blurs.

use std::sync::{Arc, OnceLock};

use gpui_kit::prelude::*;
use gpui_kit::{App, Div, Image, ImageFormat, RenderImage, Window, div, img, px};

macro_rules! renders {
    ($($side:literal),* $(,)?) => {
        /// Every side the icon is rendered at, smallest first, with its PNG.
        const RENDERS: &[(u32, &[u8])] = &[$((
            $side,
            include_bytes!(concat!("../assets/brand/png/icone-", $side, ".png")),
        )),*];
    };
}

renders!(32, 48, 64, 128, 256);

/// The render to draw `pixels` wide: the smallest that is at least that, else the largest.
fn render_for(pixels: f32) -> usize {
    RENDERS
        .iter()
        .position(|(side, _)| *side as f32 >= pixels)
        .unwrap_or(RENDERS.len() - 1)
}

/// Decoded once: the mark is on every screen, and on the first frame.
static DECODED: [OnceLock<Arc<RenderImage>>; RENDERS.len()] =
    [const { OnceLock::new() }; RENDERS.len()];

/// The mark, `side` logical pixels square, from the render that fits the window's scale.
pub fn mark(side: f32, window: &Window, cx: &App) -> Div {
    let at = render_for(side * window.scale_factor());
    let image = DECODED[at].get_or_init(|| {
        Arc::new(Image::from_bytes(ImageFormat::Png, RENDERS[at].1.to_vec()))
            .to_image_data(cx.svg_renderer())
            .expect("the embedded render decodes")
    });
    div()
        .debug_selector(|| "brand-mark".into())
        .size(px(side))
        .flex_none()
        .child(img(image.clone()).size_full())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width and height a PNG says it has.
    fn png_size(bytes: &[u8]) -> (u32, u32) {
        let read = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
        (read(16), read(20))
    }

    #[test]
    fn every_render_is_the_square_its_name_says() {
        for (side, bytes) in RENDERS {
            assert_eq!(png_size(bytes), (*side, *side));
        }
    }

    #[test]
    fn the_mark_is_drawn_from_the_smallest_render_that_is_not_stretched() {
        let side = |pixels: f32| RENDERS[render_for(pixels)].0;
        assert_eq!(side(32.), 32, "the pill bar on a 1x screen");
        assert_eq!(side(64.), 64, "the pill bar on a 2x screen");
        assert_eq!(side(44.), 48);
        assert_eq!(side(88.), 128);
        assert_eq!(side(5000.), 256, "never past the biggest");
    }
}
