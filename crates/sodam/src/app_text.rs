//! 顶栏本地音乐搜索框：动作定义、键位绑定、编辑处理器与输入法桥接。
//!
//! 编辑状态在 `ui::text_field::TextField`，这里只做三件事：
//! 1. 把 keymap 动作翻译成 TextField 操作；
//! 2. 实现 `EntityInputHandler`，让系统输入法（中文候选窗）能落到搜索框；
//! 3. 搜索内容变化后触发重绘，列表按标题/歌手/专辑过滤。

use std::ops::Range;

use gpui::{
    actions, ClipboardItem, Context, EntityInputHandler, KeyBinding, UTF16Selection, Window,
};

use crate::app::Root;
use crate::shortcuts::{GLOBAL_CONTEXT, INPUT_CONTEXT};
use crate::ui::text_field::{Direction, Edge, Milliseconds, TextField, Unit};

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordBackward,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Submit,
        Cancel,
        FocusSearch
    ]
);

/// 搜索框的全部键位绑定（每次重建 keymap 后都要重新绑上）。
pub fn bindings() -> Vec<KeyBinding> {
    let ctx = Some(INPUT_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("enter", Submit, ctx),
        KeyBinding::new("escape", Cancel, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-home", SelectHome, ctx),
        KeyBinding::new("shift-end", SelectEnd, ctx),
        KeyBinding::new("secondary-a", SelectAll, ctx),
        KeyBinding::new("secondary-c", Copy, ctx),
        KeyBinding::new("secondary-x", Cut, ctx),
        KeyBinding::new("secondary-v", Paste, ctx),
        KeyBinding::new("secondary-z", Undo, ctx),
        KeyBinding::new("secondary-shift-z", Redo, ctx),
    ];
    if cfg!(target_os = "macos") {
        bindings.extend([
            KeyBinding::new("alt-left", WordLeft, ctx),
            KeyBinding::new("alt-right", WordRight, ctx),
            KeyBinding::new("alt-backspace", DeleteWordBackward, ctx),
            KeyBinding::new("cmd-left", Home, ctx),
            KeyBinding::new("cmd-right", End, ctx),
            KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
            KeyBinding::new("cmd-shift-left", SelectHome, ctx),
            KeyBinding::new("cmd-shift-right", SelectEnd, ctx),
        ]);
    } else {
        bindings.extend([
            KeyBinding::new("ctrl-left", WordLeft, ctx),
            KeyBinding::new("ctrl-right", WordRight, ctx),
            KeyBinding::new("ctrl-backspace", DeleteWordBackward, ctx),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, ctx),
            KeyBinding::new("ctrl-y", Redo, ctx),
        ]);
    }
    bindings
}

/// `Ctrl+F` / `⌘F`：把焦点交给搜索框。
pub fn focus_bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new(
        crate::shortcuts::FOCUS_SEARCH_SOURCE,
        FocusSearch,
        Some(GLOBAL_CONTEXT),
    )]
}

impl Root {
    fn edit_text(&mut self, cx: &mut Context<Self>, edit: impl FnOnce(&mut TextField) -> bool) {
        let now = Self::search_edit_time();
        self.search_field.begin_edit();
        let changed = edit(&mut self.search_field);
        self.search_field.end_edit(now);
        if changed {
            cx.notify();
        }
    }

    fn search_edit_time() -> Milliseconds {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as Milliseconds)
            .unwrap_or(0)
    }

    pub(crate) fn text_undo(&mut self, _: &Undo, _window: &mut Window, cx: &mut Context<Self>) {
        if self.search_field.undo() {
            cx.notify();
        }
    }

    pub(crate) fn text_redo(&mut self, _: &Redo, _window: &mut Window, cx: &mut Context<Self>) {
        if self.search_field.redo() {
            cx.notify();
        }
    }

    pub(crate) fn text_backspace(
        &mut self,
        _: &Backspace,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| field.delete(Direction::Backward, Unit::Char));
    }

    pub(crate) fn text_delete(&mut self, _: &Delete, _window: &mut Window, cx: &mut Context<Self>) {
        self.edit_text(cx, |field| field.delete(Direction::Forward, Unit::Char));
    }

    pub(crate) fn text_delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| field.delete(Direction::Backward, Unit::Word));
    }

    pub(crate) fn text_left(&mut self, _: &Left, _window: &mut Window, cx: &mut Context<Self>) {
        self.edit_text(cx, |field| {
            field.move_caret(Direction::Backward, Unit::Char)
        });
    }

    pub(crate) fn text_right(&mut self, _: &Right, _window: &mut Window, cx: &mut Context<Self>) {
        self.edit_text(cx, |field| field.move_caret(Direction::Forward, Unit::Char));
    }

    pub(crate) fn text_word_left(
        &mut self,
        _: &WordLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| {
            field.move_caret(Direction::Backward, Unit::Word)
        });
    }

    pub(crate) fn text_word_right(
        &mut self,
        _: &WordRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| field.move_caret(Direction::Forward, Unit::Word));
    }

    pub(crate) fn text_select_left(
        &mut self,
        _: &SelectLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| {
            field.extend_selection(Direction::Backward, Unit::Char)
        });
    }

    pub(crate) fn text_select_right(
        &mut self,
        _: &SelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| {
            field.extend_selection(Direction::Forward, Unit::Char)
        });
    }

    pub(crate) fn text_select_word_left(
        &mut self,
        _: &SelectWordLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| {
            field.extend_selection(Direction::Backward, Unit::Word)
        });
    }

    pub(crate) fn text_select_word_right(
        &mut self,
        _: &SelectWordRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| {
            field.extend_selection(Direction::Forward, Unit::Word)
        });
    }

    pub(crate) fn text_home(&mut self, _: &Home, _window: &mut Window, cx: &mut Context<Self>) {
        self.edit_text(cx, |field| field.move_caret_to(Edge::Start));
    }

    pub(crate) fn text_end(&mut self, _: &End, _window: &mut Window, cx: &mut Context<Self>) {
        self.edit_text(cx, |field| field.move_caret_to(Edge::End));
    }

    pub(crate) fn text_select_home(
        &mut self,
        _: &SelectHome,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| field.extend_to_edge(Edge::Start));
    }

    pub(crate) fn text_select_end(
        &mut self,
        _: &SelectEnd,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| field.extend_to_edge(Edge::End));
    }

    pub(crate) fn text_select_all(
        &mut self,
        _: &SelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit_text(cx, |field| field.select_all());
    }

    pub(crate) fn text_copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.search_field.selected_text();
        if !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected));
        }
    }

    pub(crate) fn text_cut(&mut self, _: &Cut, _window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.search_field.selected_text();
        if selected.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(selected));
        self.edit_text(cx, |field| field.delete(Direction::Backward, Unit::Char));
    }

    pub(crate) fn text_paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.edit_text(cx, |field| field.paste(&text));
    }

    /// Enter：播放当前搜索结果的第一首。
    pub(crate) fn text_submit(&mut self, _: &Submit, _window: &mut Window, cx: &mut Context<Self>) {
        self.play_search_track(0, cx);
    }

    /// Esc：先撤 IME 组合，再折叠选区，最后清空查询/退出聚焦。
    pub(crate) fn text_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_field.clear_marked() {
            cx.notify();
            return;
        }
        if self.search_field.collapse_selection() {
            cx.notify();
            return;
        }
        if !self.search_field.is_empty() {
            self.edit_text(cx, |field| field.clear());
            return;
        }
        window.focus(&self.workspace_focus, cx);
        cx.notify();
    }

    /// Ctrl+F / ⌘F：聚焦顶栏搜索框。
    pub(crate) fn focus_search(
        &mut self,
        _: &FocusSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.search_focus, cx);
        cx.notify();
    }

    fn search_input_focused(&self, window: &Window) -> bool {
        self.search_focus.is_focused(window)
    }
}

impl EntityInputHandler for Root {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        if !self.search_input_focused(window) {
            return None;
        }
        let range = self.search_field.byte_range_from_utf16(range_utf16);
        actual_range.replace(self.search_field.utf16_range_from_byte(range.clone()));
        Some(self.search_field.slice(range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        if !self.search_input_focused(window) {
            return None;
        }
        Some(UTF16Selection {
            range: self
                .search_field
                .utf16_range_from_byte(self.search_field.selection_range()),
            reversed: self.search_field.is_reversed(),
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if !self.search_input_focused(window) {
            return None;
        }
        self.search_field
            .marked()
            .map(|range| self.search_field.utf16_range_from_byte(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.search_field.clear_marked() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_input_focused(window) {
            return;
        }
        let range = range_utf16.map(|range| self.search_field.byte_range_from_utf16(range));
        let now = Self::search_edit_time();
        self.search_field.begin_edit();
        self.search_field.replace(range, text);
        self.search_field.end_edit(now);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.search_input_focused(window) {
            return;
        }
        let range = range_utf16.map(|range| self.search_field.byte_range_from_utf16(range));
        let now = Self::search_edit_time();
        self.search_field.begin_edit();
        self.search_field.replace_and_mark(range, new_text);
        self.search_field.end_edit(now);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: gpui::Bounds<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        // 候选窗贴在搜索框下方即可；逐字定位需要完整排版，搜索框不需要那么精确。
        Some(self.search_bounds.get().unwrap_or(element_bounds))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<gpui::Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.search_field.byte_to_utf16(self.search_field.len()))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = self.search_field.byte_range_from_utf16(range_utf16);
        self.search_field.set_position(range.end, range.start);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.search_field.utf16_len())
    }

    fn accepts_text_input(&self, window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.search_input_focused(window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_bindings_parse_to_keystrokes() {
        for binding in bindings() {
            assert!(!binding.keystrokes().is_empty(), "绑定必须能按");
        }
    }

    #[test]
    fn focus_binding_is_a_single_key() {
        let bindings = focus_bindings();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].keystrokes().len(), 1);
    }
}
