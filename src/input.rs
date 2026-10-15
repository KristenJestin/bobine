//! A one-line text field: the login's address, user and password. After GPUI's own input example,
//! cut down to what a login needs: typing (input methods included), a cursor, Backspace, Delete,
//! the arrows, Home, End and pasting. A password field shows dots and is emptied by `take`.

use std::ops::Range;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Bounds, Context, CursorStyle, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    PaintQuad, Pixels, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, actions, div, fill, point, px, relative, size,
};

use crate::theme;

actions!(
    text_input,
    [Backspace, Delete, Left, Right, Home, End, Paste]
);

/// The keys of every text field, bound once for the application.
pub fn bind_keys(cx: &mut App) {
    let context = Some("TextInput");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("shift-insert", Paste, context),
    ]);
}

pub struct TextInput {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    /// Dots instead of the text.
    masked: bool,
    /// The cursor, a byte offset in `content`.
    cursor: usize,
    /// What an input method is composing.
    marked: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(placeholder: &'static str, masked: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.into(),
            masked,
            cursor: 0,
            marked: None,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    /// The text, leaving the field empty: a password is handed over, not kept.
    pub fn take(&mut self, cx: &mut Context<Self>) -> String {
        self.cursor = 0;
        self.marked = None;
        self.last_layout = None;
        cx.notify();
        std::mem::take(&mut self.content)
    }

    #[cfg(feature = "harness")]
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.content = text.to_string();
        self.cursor = self.content.len();
        self.marked = None;
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8())
    }

    fn replace(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.content.replace_range(range.clone(), text);
        self.cursor = range.start + text.len();
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let start = self.previous_boundary(self.cursor);
        self.replace(start..self.cursor, "", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.next_boundary(self.cursor);
        self.replace(self.cursor..end, "", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.previous_boundary(self.cursor);
        cx.notify();
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.next_boundary(self.cursor);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = 0;
        cx.notify();
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.cursor = self.content.len();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let line = text.replace(['\n', '\r'], "");
            self.replace(self.cursor..self.cursor, &line, cx);
        }
    }

    fn mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.cursor = self.content.len();
        cx.notify();
    }

    fn shown(&self) -> String {
        shown(&self.content, self.masked)
    }

    fn shown_offset(&self, offset: usize) -> usize {
        shown_offset(&self.content, self.masked, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content[..offset].encode_utf16().count()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (index, c) in self.content.char_indices() {
            if utf16 >= offset {
                return index;
            }
            utf16 += c.len_utf16();
        }
        self.content.len()
    }

    fn range_offset_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_offset_from_utf16(&range);
        actual.replace(self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end));
        // An input method never reads a password back.
        (!self.masked).then(|| self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let cursor = self.offset_to_utf16(self.cursor);
        Some(UTF16Selection {
            range: cursor..cursor,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.range_offset_from_utf16(&range))
            .or(self.marked.take())
            .unwrap_or(self.cursor..self.cursor);
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|range| self.range_offset_from_utf16(&range))
            .or(self.marked.take())
            .unwrap_or(self.cursor..self.cursor);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.replace(range, text, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_offset_from_utf16(&range);
        let x = |offset| layout.x_for_index(self.shown_offset(offset));
        Some(Bounds::from_corners(
            point(bounds.left() + x(range.start), bounds.top()),
            point(bounds.left() + x(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: gpui_kit::Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.cursor))
    }
}

/// The text line and the cursor, drawn by hand so the input method knows where they are.
struct TextLine {
    input: Entity<TextInput>,
}

struct Prepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
}

impl IntoElement for TextLine {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextLine {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let style = window.text_style();
        let (text, color): (SharedString, _) = if input.content.is_empty() {
            (input.placeholder.clone(), theme::dim().into())
        } else {
            (input.shown().into(), style.color)
        };
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match input.marked.as_ref().filter(|_| !input.masked) {
            Some(marked) => [
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: text.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);
        let x = if input.content.is_empty() {
            px(0.)
        } else {
            line.x_for_index(input.shown_offset(input.cursor))
        };
        let cursor = fill(
            Bounds::new(
                point(bounds.left() + x, bounds.top()),
                size(px(1.5), bounds.bottom() - bounds.top()),
            ),
            theme::text(),
        );
        Prepaint {
            line: Some(line),
            cursor: Some(cursor),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui_kit::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.input.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let Some(line) = prepaint.line.take() else {
            return;
        };
        line.paint(
            bounds.origin,
            window.line_height(),
            gpui_kit::TextAlign::Left,
            None,
            window,
            cx,
        )
        .ok();
        if focus.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        div()
            .key_context("TextInput")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .w_full()
            .px(px(14.))
            .py(px(11.))
            .rounded(px(theme::RADIUS))
            .border_1()
            .border_color(if focused {
                theme::muted()
            } else {
                theme::line()
            })
            .bg(theme::background())
            .text_size(px(14.))
            .line_height(px(20.))
            .child(TextLine { input: cx.entity() })
    }
}

/// What is drawn: the text, or a dot per character of a password.
fn shown(content: &str, masked: bool) -> String {
    if masked {
        "•".repeat(content.chars().count())
    } else {
        content.to_string()
    }
}

/// A byte offset of the text, as an offset of what is drawn.
fn shown_offset(content: &str, masked: bool, offset: usize) -> usize {
    if masked {
        content[..offset].chars().count() * '•'.len_utf8()
    } else {
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_shows_one_dot_per_character() {
        assert_eq!(shown("marée", true), "•••••");
        assert_eq!(shown_offset("marée", true, "marée".len()), 15);
        assert_eq!(shown("marée", false), "marée");
        assert_eq!(shown_offset("marée", false, "marée".len()), 6);
    }
}
