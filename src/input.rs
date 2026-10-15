//! A one-line text field: the login's address, user and password. After GPUI's own input example,
//! cut down to what a login needs: typing (input methods included), a cursor and a selection
//! (Shift with the arrows, Home and End, Ctrl+A, a click, a drag), Backspace, Delete, copying,
//! cutting and pasting. A password field shows dots, never lets its text out and is emptied by
//! `take`.

mod editor;

use std::ops::Range;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, KeyBinding, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill,
    point, px, relative, rgba, size,
};

use crate::theme;
use editor::Editor;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Home,
        End,
        SelectLeft,
        SelectRight,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste
    ]
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
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("ctrl-a", SelectAll, context),
        KeyBinding::new("ctrl-c", Copy, context),
        KeyBinding::new("ctrl-insert", Copy, context),
        KeyBinding::new("ctrl-x", Cut, context),
        KeyBinding::new("shift-delete", Cut, context),
        KeyBinding::new("ctrl-v", Paste, context),
        KeyBinding::new("shift-insert", Paste, context),
    ]);
}

pub struct TextInput {
    focus: FocusHandle,
    editor: Editor,
    placeholder: SharedString,
    /// Dots instead of the text.
    masked: bool,
    /// What an input method is composing.
    marked: Option<Range<usize>>,
    /// The left button went down in the field and is still held: moving selects.
    selecting: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(placeholder: &'static str, masked: bool, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            editor: Editor::default(),
            placeholder: placeholder.into(),
            masked,
            marked: None,
            selecting: false,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn text(&self) -> &str {
        self.editor.text()
    }

    /// The text, leaving the field empty: a password is handed over, not kept.
    pub fn take(&mut self, cx: &mut Context<Self>) -> String {
        self.marked = None;
        self.last_layout = None;
        cx.notify();
        self.editor.take()
    }

    #[cfg(feature = "harness")]
    pub fn selected(&self) -> &str {
        &self.editor.text()[self.editor.selection()]
    }

    #[cfg(feature = "harness")]
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.editor.take();
        self.editor.insert(text);
        self.marked = None;
        cx.notify();
    }

    /// Edits the text or moves the cursor, then redraws.
    fn edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut Editor)) {
        // Whatever the edit, the composed range no longer matches the text.
        self.marked = None;
        change(&mut self.editor);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, Editor::backspace);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, Editor::delete);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.left(false));
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.right(false));
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.home(false));
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.end(false));
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.left(true));
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.right(true));
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.home(true));
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, |e| e.end(true));
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, Editor::select_all);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.editor.copy(self.masked) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.editor.cut(self.masked) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.marked = None;
            cx.notify();
        }
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(cx, |e| e.paste(&text));
        }
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.selecting = true;
        let offset = self.offset_at(event.position);
        let extend = event.modifiers.shift;
        self.edit(cx, |e| e.click(offset, extend));
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.dragging() {
            let offset = self.offset_at(event.position);
            self.edit(cx, |e| e.drag(offset));
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    /// The byte offset of the text nearest to `position`, from the line last drawn.
    fn offset_at(&self, position: Point<Pixels>) -> usize {
        let (Some(layout), Some(bounds)) = (&self.last_layout, &self.last_bounds) else {
            return self.editor.text().len();
        };
        if self.editor.text().is_empty() {
            return 0;
        }
        let shown = layout.closest_index_for_x(position.x - bounds.left());
        content_offset(self.editor.text(), self.masked, shown)
    }

    fn shown(&self) -> String {
        shown(self.editor.text(), self.masked)
    }

    fn shown_offset(&self, offset: usize) -> usize {
        shown_offset(self.editor.text(), self.masked, offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.editor.text()[..offset].encode_utf16().count()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        for (index, c) in self.editor.text().char_indices() {
            if utf16 >= offset {
                return index;
            }
            utf16 += c.len_utf16();
        }
        self.editor.text().len()
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
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
        (!self.masked).then(|| self.editor.text()[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.editor.selection()),
            reversed: self.editor.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
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
            .unwrap_or(self.editor.selection());
        self.edit(cx, |e| e.replace(range, text));
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
            .unwrap_or(self.editor.selection());
        let marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.edit(cx, |e| e.replace(range, text));
        self.marked = marked;
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
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.offset_at(point)))
    }
}

/// The text line and the cursor, drawn by hand so the input method knows where they are.
struct TextLine {
    input: Entity<TextInput>,
}

struct Prepaint {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
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
        let (text, color): (SharedString, _) = if input.text().is_empty() {
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
            Some(marked) => {
                let [before, composed, after] = marked_lengths(marked, text.len());
                [
                    TextRun {
                        len: before,
                        ..run.clone()
                    },
                    TextRun {
                        len: composed,
                        underline: Some(UnderlineStyle {
                            color: Some(run.color),
                            thickness: px(1.),
                            wavy: false,
                        }),
                        ..run.clone()
                    },
                    TextRun { len: after, ..run },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            }
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(text, font_size, &runs, None);
        let x_of = |offset| {
            if input.text().is_empty() {
                px(0.)
            } else {
                line.x_for_index(input.shown_offset(offset))
            }
        };
        let x = x_of(input.editor.cursor());
        let selected = input.editor.selection();
        let selection = (!selected.is_empty()).then(|| {
            let (start, end) = (x_of(selected.start), x_of(selected.end));
            fill(
                Bounds::from_corners(
                    point(bounds.left() + start, bounds.top()),
                    point(bounds.left() + end, bounds.bottom()),
                ),
                rgba(0x4ade8059),
            )
        });
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
            selection,
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
        if focus.is_focused(window)
            && let Some(selection) = prepaint.selection.take()
        {
            window.paint_quad(selection);
        }
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
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
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

/// An offset of what is drawn, as a byte offset of the text.
fn content_offset(content: &str, masked: bool, shown: usize) -> usize {
    if masked {
        let dots = shown / '•'.len_utf8();
        content
            .char_indices()
            .nth(dots)
            .map_or(content.len(), |(i, _)| i)
    } else {
        shown.min(content.len())
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

/// The lengths of the text before, inside and after the composed range, which an edit may have
/// left past the end of the text.
fn marked_lengths(marked: &Range<usize>, len: usize) -> [usize; 3] {
    let start = marked.start.min(len);
    let end = marked.end.min(len);
    [start, end - start, len - end]
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

    #[test]
    fn a_composition_left_past_the_end_by_a_cut_never_underflows() {
        assert_eq!(marked_lengths(&(2..5), 8), [2, 3, 3]);
        // Ctrl+X while composing left `marked` past the end of the text.
        assert_eq!(marked_lengths(&(2..5), 3), [2, 1, 0]);
        assert_eq!(marked_lengths(&(4..6), 3), [3, 0, 0]);
    }

    #[test]
    fn a_click_on_a_password_lands_between_its_characters() {
        // The third dot starts at byte 6 of what is drawn: before the "r" of the text.
        assert_eq!(content_offset("marée", true, 6), 2);
        assert_eq!(content_offset("marée", true, 15), 6);
        assert_eq!(content_offset("marée", false, 4), 4);
    }
}
