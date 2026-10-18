//! The Downloads page: what is kept on disk and what is coming, films first, then each series by
//! season, with the disk's use at the top.

use gpui_kit::prelude::*;
use gpui_kit::{Context, IntoElement, Window, div};

use crate::shell::Shell;

pub fn render(
    _shell: &mut Shell,
    _window: &mut Window,
    _cx: &mut Context<Shell>,
) -> impl IntoElement {
    div()
}
