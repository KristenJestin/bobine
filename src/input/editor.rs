//! What a one-line field holds and how keys and the mouse edit it, apart from GPUI so it is tested
//! alone: the text, and the selection as an anchor and a cursor (byte offsets on char boundaries;
//! equal when nothing is selected).

use std::ops::Range;

#[derive(Default)]
pub struct Editor {
    content: String,
    /// Where the selection started: it stays put while Shift or a drag moves the cursor.
    anchor: usize,
    cursor: usize,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// The selected bytes, in order; empty when nothing is selected.
    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    /// The cursor is before the anchor: the selection was made leftwards.
    pub fn reversed(&self) -> bool {
        self.cursor < self.anchor
    }

    /// The text, leaving the field empty.
    pub fn take(&mut self) -> String {
        self.anchor = 0;
        self.cursor = 0;
        std::mem::take(&mut self.content)
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.cursor = self.content.len();
    }

    /// Left arrow; with Shift the selection grows or shrinks, without it a selection collapses
    /// to its start.
    pub fn left(&mut self, extend: bool) {
        if !extend && !self.selection().is_empty() {
            self.move_to(self.selection().start, false);
        } else {
            self.move_to(self.previous(self.cursor), extend);
        }
    }

    pub fn right(&mut self, extend: bool) {
        if !extend && !self.selection().is_empty() {
            self.move_to(self.selection().end, false);
        } else {
            self.move_to(self.next(self.cursor), extend);
        }
    }

    pub fn home(&mut self, extend: bool) {
        self.move_to(0, extend);
    }

    pub fn end(&mut self, extend: bool) {
        self.move_to(self.content.len(), extend);
    }

    /// A click at `offset`: the cursor goes there; with Shift the selection reaches it.
    pub fn click(&mut self, offset: usize, extend: bool) {
        self.move_to(self.boundary(offset), extend);
    }

    /// The mouse dragged to `offset`, the button held since a click.
    pub fn drag(&mut self, offset: usize) {
        self.move_to(self.boundary(offset), true);
    }

    /// Typing or pasting: the text replaces the selection.
    pub fn insert(&mut self, text: &str) {
        self.replace(self.selection(), text);
    }

    /// Replaces `range` and leaves the cursor after the new text.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let range = self.boundary(range.start)..self.boundary(range.end);
        self.content.replace_range(range.clone(), text);
        self.move_to(range.start + text.len(), false);
    }

    /// Pasting a line: what would break it is dropped.
    pub fn paste(&mut self, text: &str) {
        self.insert(&text.replace(['\n', '\r'], ""));
    }

    pub fn backspace(&mut self) {
        let range = match self.selection() {
            range if range.is_empty() => self.previous(self.cursor)..self.cursor,
            range => range,
        };
        self.replace(range, "");
    }

    pub fn delete(&mut self) {
        let range = match self.selection() {
            range if range.is_empty() => self.cursor..self.next(self.cursor),
            range => range,
        };
        self.replace(range, "");
    }

    /// What Ctrl+C puts on the clipboard: the selected text, never a password's.
    pub fn copy(&self, masked: bool) -> Option<String> {
        let range = self.selection();
        (!masked && !range.is_empty()).then(|| self.content[range].to_string())
    }

    /// Ctrl+X: the selected text, taken out of the field. A password stays where it is.
    pub fn cut(&mut self, masked: bool) -> Option<String> {
        let text = self.copy(masked)?;
        self.replace(self.selection(), "");
        Some(text)
    }

    fn move_to(&mut self, offset: usize, extend: bool) {
        self.cursor = offset;
        if !extend {
            self.anchor = offset;
        }
    }

    /// `offset`, kept inside the text and moved back to a char boundary.
    fn boundary(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.content.len());
        while !self.content.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    fn previous(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next(&self, offset: usize) -> usize {
        self.content[offset..]
            .chars()
            .next()
            .map_or(offset, |c| offset + c.len_utf8())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str) -> Editor {
        let mut editor = Editor::default();
        editor.insert(text);
        editor
    }

    fn selected(editor: &Editor) -> &str {
        &editor.text()[editor.selection()]
    }

    #[test]
    fn ctrl_a_selects_all() {
        let mut e = field("marée");
        e.select_all();
        assert_eq!(selected(&e), "marée");
    }

    #[test]
    fn typing_replaces_the_selection() {
        let mut e = field("https://media.exemple.org");
        e.select_all();
        e.insert("h");
        assert_eq!(e.text(), "h");
        assert_eq!(e.cursor(), 1);
        assert!(e.selection().is_empty());
    }

    #[test]
    fn shift_arrows_extend_the_selection_char_by_char() {
        let mut e = field("abé");
        e.left(true);
        assert_eq!(selected(&e), "é");
        e.left(true);
        assert_eq!(selected(&e), "bé");
        assert!(e.reversed());
        e.right(true);
        assert_eq!(selected(&e), "é");
    }

    #[test]
    fn shift_home_and_shift_end_select_to_the_ends() {
        let mut e = field("abcd");
        e.left(false);
        e.home(true);
        assert_eq!(selected(&e), "abc");
        e.end(true);
        assert_eq!(selected(&e), "d");
    }

    #[test]
    fn an_arrow_without_shift_collapses_the_selection_to_its_side() {
        let mut e = field("abcd");
        e.select_all();
        e.left(false);
        assert_eq!((e.cursor(), e.selection()), (0, 0..0));
        e.select_all();
        e.right(false);
        assert_eq!((e.cursor(), e.selection()), (4, 4..4));
    }

    #[test]
    fn backspace_and_delete_remove_the_selection_else_one_char() {
        let mut e = field("abcé");
        e.backspace();
        assert_eq!(e.text(), "abc");
        e.home(false);
        e.right(true);
        e.right(true);
        e.backspace();
        assert_eq!(e.text(), "c");
        e.select_all();
        e.delete();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn a_click_places_the_cursor_and_a_drag_selects() {
        let mut e = field("bobine");
        e.click(2, false);
        assert_eq!((e.cursor(), e.selection()), (2, 2..2));
        e.drag(5);
        assert_eq!(selected(&e), "bin");
        // Dragged back past the click: the selection turns around the anchor.
        e.drag(0);
        assert_eq!(selected(&e), "bo");
        e.click(6, true);
        assert_eq!(selected(&e), "bine");
    }

    #[test]
    fn a_click_inside_a_char_lands_on_its_boundary() {
        let mut e = field("é");
        e.click(1, false);
        assert_eq!(e.cursor(), 0);
        e.click(99, false);
        assert_eq!(e.cursor(), 2);
    }

    #[test]
    fn ctrl_c_copies_and_ctrl_x_cuts_the_selection() {
        let mut e = field("jellyfin");
        assert_eq!(e.copy(false), None);
        e.home(false);
        e.right(true);
        e.right(true);
        assert_eq!(e.copy(false).as_deref(), Some("je"));
        assert_eq!(e.text(), "jellyfin");
        assert_eq!(e.cut(false).as_deref(), Some("je"));
        assert_eq!(e.text(), "llyfin");
    }

    #[test]
    fn pasting_replaces_the_selection_with_one_line() {
        let mut e = field("old");
        e.select_all();
        e.paste("new\nline\r");
        assert_eq!(e.text(), "newline");
        assert_eq!(e.cursor(), 7);
    }

    #[test]
    fn a_password_is_never_copied_nor_cut_out() {
        let mut e = field("secret");
        e.select_all();
        assert_eq!(e.copy(true), None);
        assert_eq!(e.cut(true), None);
        assert_eq!(e.text(), "secret");
    }
}
