use std::path::PathBuf;

use ratatui::layout::Direction;

use crate::buffer::Buffer;
use crate::command::Command;
use crate::indent::INDENT_WIDTH;
use crate::minibuffer::Minibuffer;
use crate::pane::{Pane, PaneTree};

mod isearch;
mod fileops;
mod prompts;

pub use isearch::{ISearchState, SearchDirection};

/// Position for recenter-top-bottom cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecenterPosition {
    Center,
    Top,
    Bottom,
}

/// How an edit made through [`Editor::apply_edit`] is recorded in undo
/// history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditRecord {
    Insert,
    Delete,
    Replace,
    /// Don't record — used when replaying undo/redo groups.
    NoHistory,
}

#[allow(dead_code)]
pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub next_buffer_id: usize,
    pub pane_tree: PaneTree,
    pub clipboard: String,
    pub cwd: PathBuf,
    pub should_quit: bool,
    /// Set when the user aborts via the quit prompt's `a` answer; main exits
    /// non-zero so callers like git abandon the operation.
    pub quit_abort: bool,
    pub pending_keys: String,
    pub minibuffer: Minibuffer,
    pub minibuffer_buffer: Buffer,
    pub minibuffer_pane: Pane,
    pub isearch: Option<ISearchState>,
    /// Tracks the previously executed command (for consecutive-command detection).
    last_command: Option<Command>,
    /// Tracks last recenter position for C-l cycling.
    last_recenter_position: Option<RecenterPosition>,
    /// Buffer ids still awaiting a save-confirm answer during a quit.
    quit_pending: Vec<usize>,
}

#[allow(dead_code)]
impl Editor {
    pub fn new() -> Self {
        let buf = Buffer::new_scratch(0);
        let mut mb_pane = Pane::new(usize::MAX);
        mb_pane.viewport_height = 1;
        Self {
            buffers: vec![buf],
            next_buffer_id: 1,
            pane_tree: PaneTree::new(0),
            clipboard: String::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            should_quit: false,
            quit_abort: false,
            pending_keys: String::new(),
            minibuffer: Minibuffer::new(),
            minibuffer_buffer: Buffer::from_str(usize::MAX, "*minibuffer*", ""),
            minibuffer_pane: mb_pane,
            isearch: None,
            last_command: None,
            last_recenter_position: None,
            quit_pending: Vec::new(),
        }
    }

    pub fn new_with_text(text: &str) -> Self {
        let buf = Buffer::from_str(0, "*scratch*", text);
        let mut mb_pane = Pane::new(usize::MAX);
        mb_pane.viewport_height = 1;
        Self {
            buffers: vec![buf],
            next_buffer_id: 1,
            pane_tree: PaneTree::new(0),
            clipboard: String::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            should_quit: false,
            quit_abort: false,
            pending_keys: String::new(),
            minibuffer: Minibuffer::new(),
            minibuffer_buffer: Buffer::from_str(usize::MAX, "*minibuffer*", ""),
            minibuffer_pane: mb_pane,
            isearch: None,
            last_command: None,
            last_recenter_position: None,
            quit_pending: Vec::new(),
        }
    }


    pub fn clear_last_command(&mut self) {
        self.last_command = None;
    }

    pub fn current_buffer(&self) -> &Buffer {
        let bid = self.pane_tree.focused_pane().buffer_id;
        self.buffers
            .iter()
            .find(|b| b.id == bid)
            .expect("current buffer must exist")
    }

    pub fn current_buffer_mut(&mut self) -> &mut Buffer {
        let bid = self.pane_tree.focused_pane().buffer_id;
        self.buffers
            .iter_mut()
            .find(|b| b.id == bid)
            .expect("current buffer must exist")
    }

    /// Get the active buffer (minibuffer buffer when prompt is active, else focused pane's buffer).
    pub fn active_buffer(&self) -> &Buffer {
        if self.minibuffer.is_active() {
            &self.minibuffer_buffer
        } else {
            self.current_buffer()
        }
    }

    /// Get the active buffer mutably.
    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        if self.minibuffer.is_active() {
            &mut self.minibuffer_buffer
        } else {
            self.current_buffer_mut()
        }
    }

    /// Get the active pane (minibuffer pane when prompt is active, else focused pane).
    pub fn active_pane(&self) -> &Pane {
        if self.minibuffer.is_active() {
            &self.minibuffer_pane
        } else {
            self.pane_tree.focused_pane()
        }
    }

    /// Get the active pane mutably.
    pub fn active_pane_mut(&mut self) -> &mut Pane {
        if self.minibuffer.is_active() {
            &mut self.minibuffer_pane
        } else {
            self.pane_tree.focused_pane_mut()
        }
    }

    /// Get buffer by id.
    pub fn buffer_by_id(&self, id: usize) -> &Buffer {
        self.buffers
            .iter()
            .find(|b| b.id == id)
            .expect("buffer must exist")
    }

    pub fn buffer_text(&self) -> String {
        self.current_buffer().text.to_string()
    }

    pub fn point(&self) -> usize {
        self.pane_tree.focused_pane().point
    }

    pub fn commit_undo_group(&mut self) {
        self.active_buffer_mut().history.commit();
    }

    pub fn buffer_names(&self) -> Vec<String> {
        self.buffers.iter().map(|b| b.name.clone()).collect()
    }

    /// Get the ordered region (start, end) for the focused pane.
    pub fn region(&self) -> Option<(usize, usize)> {
        let pane = self.pane_tree.focused_pane();
        pane.mark.map(|mark| {
            let start = pane.point.min(mark);
            let end = pane.point.max(mark);
            (start, end)
        })
    }

    pub fn execute(&mut self, cmd: Command) {
        // When minibuffer is active, intercept InsertNewline (submit) and InsertTab (complete)
        if self.minibuffer.is_active() {
            match &cmd {
                Command::InsertNewline => {
                    self.submit_prompt();
                    return;
                }
                Command::InsertTab | Command::IndentLine | Command::DedentLine => {
                    // Tab completion is handled in app.rs key routing
                    return;
                }
                _ => {}
            }
        }

        // Mark non-edit actions for undo grouping
        match &cmd {
            Command::InsertChar(_) | Command::InsertNewline | Command::InsertTab => {}
            Command::IndentLine | Command::DedentLine => {}
            Command::DeleteBackward
            | Command::DeleteForward
            | Command::DeleteWordBackward
            | Command::KillLine => {}
            Command::Undo | Command::Redo => {}
            _ => {
                self.active_buffer_mut().history.mark_action();
            }
        }

        let cmd_clone = cmd.clone();

        match cmd {
            Command::ForwardChar => self.forward_char(),
            Command::BackwardChar => self.backward_char(),
            Command::ForwardWord => self.forward_word(),
            Command::BackwardWord => self.backward_word(),
            Command::NextLine => self.next_line(),
            Command::PreviousLine => self.previous_line(),
            Command::BeginningOfLine => self.beginning_of_line(),
            Command::EndOfLine => self.end_of_line(),
            Command::BufferBeginning => self.buffer_beginning(),
            Command::BufferEnd => self.buffer_end(),
            Command::PageDown => self.page_down(),
            Command::PageUp => self.page_up(),
            Command::RecenterTopBottom => self.recenter_top_bottom(),
            Command::InsertChar(c) => self.insert_char(c),
            Command::InsertNewline => self.insert_newline(),
            Command::InsertTab => self.insert_tab(),
            Command::IndentLine => self.indent_line(),
            Command::DedentLine => self.dedent_line(),
            Command::DeleteBackward => self.delete_backward(),
            Command::DeleteForward => self.delete_forward(),
            Command::DeleteWordBackward => self.delete_word_backward(),
            Command::KillLine => self.kill_line(),
            Command::Undo => self.undo(),
            Command::Redo => self.redo(),
            Command::Save => self.save(),
            Command::WriteFile => self.write_file_prompt(),
            Command::FindFile => self.find_file_prompt(),
            Command::SwitchBuffer => self.switch_buffer_prompt(),
            Command::KillBuffer => self.kill_buffer(),
            Command::GotoLine => self.goto_line_prompt(),
            Command::SetMark => self.set_mark(),
            Command::SwapPointAndMark => self.swap_point_and_mark(),
            Command::Cut => self.cut(),
            Command::Copy => self.copy(),
            Command::Paste => self.paste(),
            Command::SplitVertical => self.split_vertical(),
            Command::SplitHorizontal => self.split_horizontal(),
            Command::DeletePane => self.delete_pane(),
            Command::DeleteOtherPanes => self.delete_other_panes(),
            Command::CycleFocus => self.cycle_focus(),
            Command::ISearchForward => self.isearch_start(SearchDirection::Forward),
            Command::ISearchBackward => self.isearch_start(SearchDirection::Backward),
            Command::Cancel => self.cancel(),
            Command::Quit => self.quit(),
        }

        self.last_command = Some(cmd_clone);

        self.ensure_cursor_visible();
    }

    /// Scroll the focused pane so its point is visible. Runs after every
    /// command and after minibuffer prompt submission (which moves point
    /// outside of `execute()`, e.g. goto-line). No-op while a prompt is
    /// active — the minibuffer pane has its own cursor.
    pub(crate) fn ensure_cursor_visible(&mut self) {
        if self.minibuffer.is_active() {
            return;
        }
        let pane = self.pane_tree.focused_pane();
        let point = pane.point;
        let scroll_top = pane.scroll_top;
        let vh = pane.viewport_height;
        let vw = pane.viewport_width;
        let buf = self.current_buffer();
        let (line, _) = buf.char_to_line_col(point);
        // Wrap by tab-expanded visual width, matching the renderer.
        let new_top = crate::pane::compute_scroll_top(scroll_top, line, vh, vw, |l| {
            crate::render::line_visual_width(buf, l)
        });
        self.pane_tree.focused_pane_mut().scroll_top = new_top;
    }

    // === Pane split commands ===

    fn split_vertical(&mut self) {
        let buf_id = self.pane_tree.focused_pane().buffer_id;
        self.pane_tree.split(Direction::Vertical, buf_id);
    }

    fn split_horizontal(&mut self) {
        let buf_id = self.pane_tree.focused_pane().buffer_id;
        self.pane_tree.split(Direction::Horizontal, buf_id);
    }

    fn delete_pane(&mut self) {
        if !self.pane_tree.delete_focused() {
            self.minibuffer
                .show_message("Cannot delete the only pane".to_string());
        }
    }

    fn delete_other_panes(&mut self) {
        self.pane_tree.delete_others();
    }

    fn cycle_focus(&mut self) {
        self.pane_tree.cycle_focus();
    }


    // === Movement commands ===

    /// True if `pos` sits between the `\r` and `\n` of a CRLF pair.
    /// Point must never rest there; the pair is treated as one unit.
    fn inside_crlf(&self, pos: usize) -> bool {
        let buf = self.active_buffer();
        pos > 0
            && pos < buf.char_count()
            && buf.text.char(pos - 1) == '\r'
            && buf.text.char(pos) == '\n'
    }

    /// Char index of the next grapheme-cluster boundary after `pos`.
    /// Line endings (including CRLF pairs) count as one step.
    fn next_grapheme_boundary(&self, pos: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let buf = self.active_buffer();
        let len = buf.char_count();
        if pos >= len {
            return len;
        }
        let (line, col) = buf.char_to_line_col(pos);
        let line_len = buf.line_len_chars(line);
        if col >= line_len {
            // Stepping over the line ending.
            let next = pos + 1;
            return if self.inside_crlf(next) { next + 1 } else { next };
        }
        let line_start = buf.line_col_to_char(line, 0);
        let line_text: String = buf
            .text
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let mut start = 0;
        for g in line_text.graphemes(true) {
            let g_len = g.chars().count();
            if col < start + g_len {
                return line_start + start + g_len;
            }
            start += g_len;
        }
        pos + 1
    }

    /// Char index of the previous grapheme-cluster boundary before `pos`.
    fn prev_grapheme_boundary(&self, pos: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let buf = self.active_buffer();
        if pos == 0 {
            return 0;
        }
        let (line, col) = buf.char_to_line_col(pos);
        if col == 0 {
            // Stepping back over the previous line's ending.
            let prev = pos - 1;
            return if self.inside_crlf(prev) { prev - 1 } else { prev };
        }
        let line_start = buf.line_col_to_char(line, 0);
        let line_len = buf.line_len_chars(line);
        let line_text: String = buf
            .text
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let mut start = 0;
        for g in line_text.graphemes(true) {
            let g_len = g.chars().count();
            if start < col && col <= start + g_len {
                return line_start + start;
            }
            start += g_len;
        }
        pos - 1
    }

    fn forward_char(&mut self) {
        let pos = self.active_pane().point;
        let new_pos = self.next_grapheme_boundary(pos);
        let pane = self.active_pane_mut();
        pane.point = new_pos;
        pane.preferred_column = None;
    }

    fn backward_char(&mut self) {
        let pos = self.active_pane().point;
        let new_pos = self.prev_grapheme_boundary(pos);
        let pane = self.active_pane_mut();
        pane.point = new_pos;
        pane.preferred_column = None;
    }

    fn forward_word(&mut self) {
        let buf = self.active_buffer();
        let len = buf.char_count();
        let mut pos = self.active_pane().point;

        // Skip non-word characters
        while pos < len {
            let ch = buf.text.char(pos);
            if ch.is_alphanumeric() || ch == '_' {
                break;
            }
            pos += 1;
        }
        // Skip word characters
        while pos < len {
            let ch = buf.text.char(pos);
            if !ch.is_alphanumeric() && ch != '_' {
                break;
            }
            pos += 1;
        }

        let pane = self.active_pane_mut();
        pane.point = pos;
        pane.preferred_column = None;
    }

    fn backward_word(&mut self) {
        let buf = self.active_buffer();
        let mut pos = self.active_pane().point;

        // Skip non-word characters backward
        while pos > 0 {
            let ch = buf.text.char(pos - 1);
            if ch.is_alphanumeric() || ch == '_' {
                break;
            }
            pos -= 1;
        }
        // Skip word characters backward
        while pos > 0 {
            let ch = buf.text.char(pos - 1);
            if !ch.is_alphanumeric() && ch != '_' {
                break;
            }
            pos -= 1;
        }

        let pane = self.active_pane_mut();
        pane.point = pos;
        pane.preferred_column = None;
    }

    fn delete_word_backward(&mut self) {
        let buf = self.active_buffer();
        let start = self.active_pane().point;
        if start == 0 {
            return;
        }

        // Find the word boundary (same logic as backward_word)
        let mut pos = start;
        while pos > 0 {
            let ch = buf.text.char(pos - 1);
            if ch.is_alphanumeric() || ch == '_' {
                break;
            }
            pos -= 1;
        }
        while pos > 0 {
            let ch = buf.text.char(pos - 1);
            if !ch.is_alphanumeric() && ch != '_' {
                break;
            }
            pos -= 1;
        }

        // Delete from pos to start
        self.apply_edit(pos, start, "", EditRecord::Delete);
        self.active_pane_mut().point = pos;
        self.active_pane_mut().preferred_column = None;
    }

    fn next_line(&mut self) {
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point);
        let target_col = pane.preferred_column.unwrap_or(col);

        if line + 1 < buf.line_count() {
            let new_point = buf.line_col_to_char(line + 1, target_col);
            let pane = self.active_pane_mut();
            pane.point = new_point;
            pane.preferred_column = Some(target_col);
        }
    }

    fn previous_line(&mut self) {
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point);
        let target_col = pane.preferred_column.unwrap_or(col);

        if line > 0 {
            let new_point = buf.line_col_to_char(line - 1, target_col);
            let pane = self.active_pane_mut();
            pane.point = new_point;
            pane.preferred_column = Some(target_col);
        }
    }

    fn beginning_of_line(&mut self) {
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point);
        let new_point = buf.line_col_to_char(line, 0);
        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.preferred_column = None;
    }

    fn end_of_line(&mut self) {
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point);
        let line_len = buf.line_len_chars(line);
        let new_point = buf.line_col_to_char(line, line_len);
        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.preferred_column = None;
    }

    fn buffer_beginning(&mut self) {
        let pane = self.active_pane_mut();
        pane.point = 0;
        pane.preferred_column = None;
    }

    fn buffer_end(&mut self) {
        let len = self.active_buffer().char_count();
        let pane = self.active_pane_mut();
        pane.point = len;
        pane.preferred_column = None;
    }

    fn page_down(&mut self) {
        let height = self.active_pane().viewport_height;
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point);
        let target_col = pane.preferred_column.unwrap_or(col);
        let new_line = (line + height).min(buf.line_count().saturating_sub(1));
        let new_point = buf.line_col_to_char(new_line, target_col);
        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.preferred_column = Some(target_col);
    }

    fn page_up(&mut self) {
        let height = self.active_pane().viewport_height;
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point);
        let target_col = pane.preferred_column.unwrap_or(col);
        let new_line = line.saturating_sub(height);
        let new_point = buf.line_col_to_char(new_line, target_col);
        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.preferred_column = Some(target_col);
    }

    fn recenter_top_bottom(&mut self) {
        let pane = self.active_pane();
        let buf = self.active_buffer();
        let (cursor_line, _) = buf.char_to_line_col(pane.point);
        let height = pane.viewport_height;

        let is_consecutive = self.last_command == Some(Command::RecenterTopBottom);
        let position = match if is_consecutive {
            self.last_recenter_position
        } else {
            None
        } {
            None | Some(RecenterPosition::Bottom) => RecenterPosition::Center,
            Some(RecenterPosition::Center) => RecenterPosition::Top,
            Some(RecenterPosition::Top) => RecenterPosition::Bottom,
        };

        let new_scroll_top = match position {
            RecenterPosition::Center => cursor_line.saturating_sub(height / 2),
            RecenterPosition::Top => cursor_line,
            RecenterPosition::Bottom => cursor_line.saturating_sub(height.saturating_sub(1)),
        };

        self.active_pane_mut().scroll_top = new_scroll_top;
        self.last_recenter_position = Some(position);
    }

    // === Editing commands ===

    /// Central edit primitive: replace the chars in `[start, end)` of the
    /// active buffer with `text`. All positions are char indices — never mix
    /// in byte lengths. Records history according to `record` and adjusts
    /// point/mark and saved view state in every pane viewing the buffer.
    /// Callers that want a point other than what marker semantics give
    /// (e.g. point after inserted text) set the active pane's point
    /// explicitly afterwards, in char units. Returns the deleted text.
    pub(crate) fn apply_edit(
        &mut self,
        start: usize,
        end: usize,
        text: &str,
        record: EditRecord,
    ) -> String {
        let (buffer_id, delta, deleted) = {
            let buf = self.active_buffer_mut();
            let len = buf.char_count();
            let start = start.min(len);
            let end = end.min(len).max(start);
            let deleted: String = if end > start {
                buf.text.slice(start..end).chars().collect()
            } else {
                String::new()
            };
            // Line-level effect of the edit, measured with the rope's own
            // line semantics: removed line breaks before the edit, inserted
            // line breaks after it. Everything before `start` is unchanged,
            // so `first_line` is valid on both sides of the edit.
            let first_line = buf.char_to_line_col(start).0;
            let removed_lines = buf.char_to_line_col(end).0 - first_line;
            match record {
                EditRecord::Insert => buf.history.record_insert(start, text),
                EditRecord::Delete => buf.history.record_delete(start, &deleted),
                EditRecord::Replace => buf.history.record_replace(start, &deleted, text),
                EditRecord::NoHistory => {}
            }
            if end > start {
                buf.remove(start, end);
            }
            if !text.is_empty() {
                buf.insert(start, text);
            }
            let inserted = text.chars().count();
            let inserted_lines = buf.char_to_line_col(start + inserted).0 - first_line;
            let delta = crate::pane::EditDelta {
                start,
                removed: end - start,
                inserted,
                first_line,
                removed_lines,
                inserted_lines,
            };
            (buf.id, delta, deleted)
        };

        if buffer_id == usize::MAX {
            // The minibuffer buffer is viewed only by the minibuffer pane.
            self.minibuffer_pane.adjust_for_edit(buffer_id, delta);
        } else {
            self.pane_tree.for_each_pane_mut(&mut |pane| {
                pane.adjust_for_edit(buffer_id, delta);
            });
        }
        deleted
    }

    fn insert_char(&mut self, c: char) {
        let pos = self.active_pane().point;
        let s = c.to_string();
        self.apply_edit(pos, pos, &s, EditRecord::Insert);
        let pane = self.active_pane_mut();
        pane.point = pos + 1;
        pane.preferred_column = None;
    }

    fn insert_newline(&mut self) {
        let pos = self.active_pane().point;
        let buf = self.active_buffer();
        let le = buf.line_ending.as_str().to_string();

        // Get current line's leading whitespace (spaces only)
        let (line, _) = buf.char_to_line_col(pos);
        let line_start = buf.line_col_to_char(line, 0);
        let line_len = buf.line_len_chars(line);
        let mut indent = String::new();
        for i in 0..line_len {
            let ch = buf.text.char(line_start + i);
            if ch == ' ' {
                indent.push(' ');
            } else if ch == '\t' {
                // Convert tabs to spaces
                let spaces = INDENT_WIDTH - (indent.len() % INDENT_WIDTH);
                for _ in 0..spaces {
                    indent.push(' ');
                }
            } else {
                break;
            }
        }

        let insert_str = format!("{le}{indent}");
        self.apply_edit(pos, pos, &insert_str, EditRecord::Insert);
        let pane = self.active_pane_mut();
        pane.point = pos + insert_str.chars().count();
        pane.preferred_column = None;
    }

    fn insert_tab(&mut self) {
        let pos = self.active_pane().point;
        let spaces = " ".repeat(INDENT_WIDTH);
        self.apply_edit(pos, pos, &spaces, EditRecord::Insert);
        let pane = self.active_pane_mut();
        pane.point = pos + INDENT_WIDTH;
        pane.preferred_column = None;
    }

    fn indent_line(&mut self) {
        if self.active_region().is_some() {
            self.indent_region();
            return;
        }
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point);
        let line_start = buf.line_col_to_char(line, 0);

        // Get current leading whitespace
        let line_len = buf.line_len_chars(line);
        let mut ws_len = 0;
        for i in 0..line_len {
            if buf.text.char(line_start + i) == ' ' {
                ws_len += 1;
            } else {
                break;
            }
        }
        let old_ws: String = " ".repeat(ws_len);
        let new_ws = format!("{}{}", " ".repeat(INDENT_WIDTH), old_ws);

        let old_point = self.active_pane().point;
        self.apply_edit(line_start, line_start + ws_len, &new_ws, EditRecord::Replace);
        self.active_buffer_mut().history.commit();
        let pane = self.active_pane_mut();
        pane.point = old_point + INDENT_WIDTH;
        pane.preferred_column = None;
    }

    fn dedent_line(&mut self) {
        if self.active_region().is_some() {
            self.dedent_region();
            return;
        }
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point);
        let line_start = buf.line_col_to_char(line, 0);

        // Count leading spaces (up to INDENT_WIDTH)
        let line_len = buf.line_len_chars(line);
        let mut remove_count = 0;
        for i in 0..line_len.min(INDENT_WIDTH) {
            if buf.text.char(line_start + i) == ' ' {
                remove_count += 1;
            } else {
                break;
            }
        }

        if remove_count == 0 {
            return;
        }

        // We remove these spaces, so new whitespace is empty for those chars
        let remaining_ws_len = {
            let buf = self.active_buffer();
            let line_len = buf.line_len_chars(line);
            let mut total = 0;
            for i in 0..line_len {
                if buf.text.char(line_start + i) == ' ' {
                    total += 1;
                } else {
                    break;
                }
            }
            total
        };
        let new_ws: String = " ".repeat(remaining_ws_len - remove_count);

        let old_point = self.active_pane().point;
        self.apply_edit(
            line_start,
            line_start + remaining_ws_len,
            &new_ws,
            EditRecord::Replace,
        );
        self.active_buffer_mut().history.commit();

        let pane = self.active_pane_mut();
        // Adjust point, but don't go before line start
        let point_col = old_point - line_start;
        if point_col <= remove_count {
            pane.point = line_start;
        } else {
            pane.point = old_point - remove_count;
        }
        pane.preferred_column = None;
    }

    fn indent_region(&mut self) {
        let (region_start, region_end) = match self.active_region() {
            Some(r) => r,
            None => return,
        };
        let buf = self.active_buffer();
        let (first_line, _) = buf.char_to_line_col(region_start);
        let (last_line_candidate, last_col) = buf.char_to_line_col(region_end);
        // Exclude last line if region end is exactly at column 0
        let last_line = if last_col == 0 && last_line_candidate > first_line {
            last_line_candidate - 1
        } else {
            last_line_candidate
        };

        let span_start = buf.line_col_to_char(first_line, 0);
        // span_end: end of the last line's content (including newline if present)
        let span_end = if last_line + 1 < buf.line_count() {
            buf.line_col_to_char(last_line + 1, 0)
        } else {
            buf.char_count()
        };

        let indent_str = " ".repeat(INDENT_WIDTH);

        // Build new text and track per-line delta for cursor adjustment
        let mut new_text = String::new();
        let pane = self.active_pane();
        let point = pane.point;
        let mark = pane.mark.unwrap_or(point);

        let mut new_point = point;
        let mut new_mark = mark;

        for line_idx in first_line..=last_line {
            let line_start = buf.line_col_to_char(line_idx, 0);
            let next_line_start = if line_idx + 1 < buf.line_count() {
                buf.line_col_to_char(line_idx + 1, 0)
            } else {
                buf.char_count()
            };
            let line_text: String = buf
                .text
                .slice(line_start..next_line_start)
                .chars()
                .collect();

            new_text.push_str(&indent_str);
            new_text.push_str(&line_text);

            // Adjust point and mark if they're on this line
            if point >= line_start
                && (point < next_line_start || (line_idx == last_line && point == next_line_start))
            {
                new_point = point + INDENT_WIDTH * (line_idx - first_line + 1);
            }
            if mark >= line_start
                && (mark < next_line_start || (line_idx == last_line && mark == next_line_start))
            {
                new_mark = mark + INDENT_WIDTH * (line_idx - first_line + 1);
            }
        }

        self.apply_edit(span_start, span_end, &new_text, EditRecord::Replace);
        self.active_buffer_mut().history.commit();

        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.mark = Some(new_mark);
        pane.preferred_column = None;
    }

    fn dedent_region(&mut self) {
        let (region_start, region_end) = match self.active_region() {
            Some(r) => r,
            None => return,
        };
        let buf = self.active_buffer();
        let (first_line, _) = buf.char_to_line_col(region_start);
        let (last_line_candidate, last_col) = buf.char_to_line_col(region_end);
        let last_line = if last_col == 0 && last_line_candidate > first_line {
            last_line_candidate - 1
        } else {
            last_line_candidate
        };

        let span_start = buf.line_col_to_char(first_line, 0);
        let span_end = if last_line + 1 < buf.line_count() {
            buf.line_col_to_char(last_line + 1, 0)
        } else {
            buf.char_count()
        };

        let pane = self.active_pane();
        let point = pane.point;
        let mark = pane.mark.unwrap_or(point);

        let mut new_point = point;
        let mut new_mark = mark;
        let mut new_text = String::new();
        let mut cumulative_delta: usize = 0;

        for line_idx in first_line..=last_line {
            let line_start = buf.line_col_to_char(line_idx, 0);
            let next_line_start = if line_idx + 1 < buf.line_count() {
                buf.line_col_to_char(line_idx + 1, 0)
            } else {
                buf.char_count()
            };
            let line_text: String = buf
                .text
                .slice(line_start..next_line_start)
                .chars()
                .collect();

            // Count leading spaces to remove (up to INDENT_WIDTH)
            let remove_count = line_text
                .chars()
                .take(INDENT_WIDTH)
                .take_while(|&c| c == ' ')
                .count();

            new_text.push_str(&line_text[remove_count..]);
            cumulative_delta += remove_count;

            // Adjust point
            if point >= line_start
                && (point < next_line_start || (line_idx == last_line && point == next_line_start))
            {
                let col = point - line_start;
                if col <= remove_count {
                    new_point = line_start - (cumulative_delta - remove_count);
                } else {
                    new_point = point - cumulative_delta;
                }
            }
            // Adjust mark
            if mark >= line_start
                && (mark < next_line_start || (line_idx == last_line && mark == next_line_start))
            {
                let col = mark - line_start;
                if col <= remove_count {
                    new_mark = line_start - (cumulative_delta - remove_count);
                } else {
                    new_mark = mark - cumulative_delta;
                }
            }
        }

        if cumulative_delta == 0 {
            return;
        }

        self.apply_edit(span_start, span_end, &new_text, EditRecord::Replace);
        self.active_buffer_mut().history.commit();

        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.mark = Some(new_mark);
        pane.preferred_column = None;
    }

    fn delete_backward(&mut self) {
        let pos = self.active_pane().point;
        if pos > 0 {
            // Delete a whole grapheme cluster (or CRLF pair) as a unit.
            let start = self.prev_grapheme_boundary(pos);
            self.apply_edit(start, pos, "", EditRecord::Delete);
            let pane = self.active_pane_mut();
            pane.point = start;
            pane.preferred_column = None;
        }
    }

    fn delete_forward(&mut self) {
        let len = self.active_buffer().char_count();
        let pos = self.active_pane().point;
        if pos < len {
            // Delete a whole grapheme cluster (or CRLF pair) as a unit.
            let end = self.next_grapheme_boundary(pos);
            self.apply_edit(pos, end, "", EditRecord::Delete);
            self.active_pane_mut().preferred_column = None;
        }
    }

    fn kill_line(&mut self) {
        let append = self.last_command == Some(Command::KillLine);
        let buf = self.active_buffer();
        let pos = self.active_pane().point;
        let (line, col) = buf.char_to_line_col(pos);
        let line_len = buf.line_len_chars(line);

        if col == line_len {
            let total = buf.char_count();
            if pos < total {
                // At EOL, kill the whole line ending (one or two chars for CRLF).
                let end = if self.inside_crlf(pos + 1) { pos + 2 } else { pos + 1 };
                let deleted = self.apply_edit(pos, end, "", EditRecord::Delete);
                if append {
                    self.clipboard.push_str(&deleted);
                } else {
                    self.clipboard = deleted;
                }
            }
        } else {
            let end = buf.line_col_to_char(line, line_len);
            let deleted = self.apply_edit(pos, end, "", EditRecord::Delete);
            if append {
                self.clipboard.push_str(&deleted);
            } else {
                self.clipboard = deleted;
            }
        }
        self.active_buffer_mut().history.commit();
        self.set_os_clipboard(&self.clipboard.clone());
        self.active_pane_mut().preferred_column = None;
    }

    // === Undo/Redo ===

    fn undo(&mut self) {
        if let Some(group) = self.active_buffer_mut().history.undo() {
            for edit in group.edits.iter().rev() {
                // Reverse the edit: replace what it inserted with what it
                // deleted. Lengths are char counts, matching positions.
                let end = edit.position + edit.inserted.chars().count();
                self.apply_edit(edit.position, end, &edit.deleted, EditRecord::NoHistory);
            }
            if let Some(first) = group.edits.first() {
                let len = self.active_buffer().char_count();
                self.active_pane_mut().point = first.position.min(len);
            }
            self.active_buffer_mut().update_modified();
            if !self.minibuffer.is_active() {
                self.minibuffer.show_message("Undo!".to_string());
            }
        } else if !self.minibuffer.is_active() {
            self.minibuffer
                .show_message("No further undo information".to_string());
        }
    }

    fn redo(&mut self) {
        if let Some(group) = self.active_buffer_mut().history.redo() {
            for edit in &group.edits {
                // Re-apply the edit: replace what it deleted with what it
                // inserted. Lengths are char counts, matching positions.
                let end = edit.position + edit.deleted.chars().count();
                self.apply_edit(edit.position, end, &edit.inserted, EditRecord::NoHistory);
            }
            if let Some(last) = group.edits.last() {
                let len = self.active_buffer().char_count();
                self.active_pane_mut().point =
                    (last.position + last.inserted.chars().count()).min(len);
            }
            self.active_buffer_mut().update_modified();
            if !self.minibuffer.is_active() {
                self.minibuffer.show_message("Redo!".to_string());
            }
        } else if !self.minibuffer.is_active() {
            self.minibuffer
                .show_message("No further redo information".to_string());
        }
    }


    // === Mark/Region ===

    fn set_mark(&mut self) {
        let point = self.active_pane().point;
        self.active_pane_mut().mark = Some(point);
        self.minibuffer.show_message("Mark set".to_string());
    }

    fn swap_point_and_mark(&mut self) {
        let pane = self.active_pane_mut();
        if let Some(mark) = pane.mark {
            let old_point = pane.point;
            pane.point = mark;
            pane.mark = Some(old_point);
        } else {
            self.minibuffer.show_message("No mark set".to_string());
        }
    }

    fn region_text(&self) -> Option<String> {
        self.region().map(|(start, end)| {
            self.current_buffer()
                .text
                .slice(start..end)
                .chars()
                .collect()
        })
    }

    /// Get region from the active pane (minibuffer pane or focused pane).
    fn active_region(&self) -> Option<(usize, usize)> {
        let pane = self.active_pane();
        pane.mark.map(|mark| {
            let start = pane.point.min(mark);
            let end = pane.point.max(mark);
            (start, end)
        })
    }

    fn cut(&mut self) {
        if let Some((start, end)) = self.active_region() {
            let text = self.apply_edit(start, end, "", EditRecord::Delete);
            self.active_buffer_mut().history.commit();
            self.clipboard = text.clone();
            self.set_os_clipboard(&text);
            let pane = self.active_pane_mut();
            pane.point = start;
            pane.mark = None;
        } else {
            self.minibuffer
                .show_message("No region selected".to_string());
        }
    }

    fn copy(&mut self) {
        if let Some((start, end)) = self.active_region() {
            let text: String = self
                .active_buffer()
                .text
                .slice(start..end)
                .chars()
                .collect();
            self.clipboard = text.clone();
            self.set_os_clipboard(&text);
            self.active_pane_mut().mark = None;
            self.minibuffer.show_message("Region copied".to_string());
        } else {
            self.minibuffer
                .show_message("No region selected".to_string());
        }
    }

    fn paste(&mut self) {
        let text = self
            .get_os_clipboard()
            .unwrap_or_else(|| self.clipboard.clone());
        if !text.is_empty() {
            // Sanitize: replace newlines with spaces when pasting into minibuffer
            let text = if self.minibuffer.is_active() {
                text.replace("\r\n", " ").replace('\n', " ")
            } else {
                text
            };
            let pos = self.active_pane().point;
            self.apply_edit(pos, pos, &text, EditRecord::Insert);
            self.active_buffer_mut().history.commit();
            self.active_pane_mut().point = pos + text.chars().count();
        }
        self.active_pane_mut().preferred_column = None;
    }

    fn set_os_clipboard(&self, _text: &str) {
        #[cfg(all(feature = "clipboard", not(test)))]
        {
            if let Ok(mut clip) = arboard::Clipboard::new() {
                let _ = clip.set_text(_text.to_string());
            }
        }
    }

    fn get_os_clipboard(&self) -> Option<String> {
        #[cfg(all(feature = "clipboard", not(test)))]
        {
            if let Ok(mut clip) = arboard::Clipboard::new() {
                return clip.get_text().ok();
            }
        }
        None
    }

    fn cancel(&mut self) {
        if let Some(isearch) = self.isearch.take() {
            // Restore original position
            let pane = self.pane_tree.focused_pane_mut();
            pane.point = isearch.original_point;
            pane.scroll_top = isearch.original_scroll_top;
            self.minibuffer.finish();
            self.minibuffer.show_message("Quit".to_string());
        } else if self.minibuffer.is_active() {
            self.minibuffer.cancel();
        } else {
            self.pane_tree.focused_pane_mut().mark = None;
            self.minibuffer.show_message("Quit".to_string());
        }
    }

}

#[cfg(test)]
mod tests;
