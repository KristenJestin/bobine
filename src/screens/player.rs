use gpui_kit::{Context, IntoElement};

use super::empty_screen;
use crate::shell::Shell;

pub fn render(_cx: &mut Context<Shell>) -> impl IntoElement {
    empty_screen("Lecteur")
}
