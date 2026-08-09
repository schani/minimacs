use std::path::{Path, PathBuf};

use ratatui::layout::Direction;

use crate::buffer::Buffer;
use crate::command::Command;
use crate::indent::INDENT_WIDTH;
use crate::keymap::default_keymap;
use crate::minibuffer::Minibuffer;
use crate::pane::{Pane, PaneTree};

mod fileops;
mod isearch;
mod prompts;

pub use isearch::{ISearchState, SearchDirection};

/// Position for recenter-top-bottom cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecenterPosition {
    Center,
    Top,
    Bottom,
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// The editor's connection to the OS clipboard. One instance lives for the
/// editor's lifetime: on X11 the selection is owned by the process that set
/// it, and dropping the arboard handle right after a kill (as a per-call
/// handle would) can drop the selection before another application reads
/// it. Compiled to a no-op under test, where no display is available.
pub(crate) struct OsClipboard {
    #[cfg(all(feature = "clipboard", not(test)))]
    inner: Option<arboard::Clipboard>,
    /// Test-only recorder: the last text passed to `set_text`, so tests can
    /// assert whether (and with what) the OS clipboard would be written.
    #[cfg(test)]
    pub(crate) last_set_text: Option<String>,
}

impl OsClipboard {
    pub(crate) fn new() -> Self {
        Self {
            #[cfg(all(feature = "clipboard", not(test)))]
            inner: None,
            #[cfg(test)]
            last_set_text: None,
        }
    }

    /// Connect lazily; a failed attempt (e.g. no display) is retried on the
    /// next call rather than cached forever.
    #[cfg(all(feature = "clipboard", not(test)))]
    fn connect(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.inner.is_none() {
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }

    pub(crate) fn set_text(&mut self, _text: &str) {
        #[cfg(test)]
        {
            self.last_set_text = Some(_text.to_string());
        }
        #[cfg(all(feature = "clipboard", not(test)))]
        if let Some(clip) = self.connect() {
            if clip.set_text(_text.to_string()).is_err() {
                // The connection may have gone stale; drop it so the next
                // call reconnects instead of failing forever.
                self.inner = None;
            }
        }
    }

    pub(crate) fn get_text(&mut self) -> Option<String> {
        #[cfg(all(feature = "clipboard", not(test)))]
        if let Some(clip) = self.connect() {
            // Errors here also cover "clipboard empty / not text", so they
            // don't indicate a dead connection; fall through to None.
            return clip.get_text().ok();
        }
        None
    }
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

pub const MINIBUFFER_BUFFER_ID: usize = usize::MAX;

pub struct Editor {
    buffers: Vec<Buffer>,
    next_buffer_id: usize,
    pane_tree: PaneTree,
    clipboard: String,
    cwd: PathBuf,
    should_quit: bool,
    /// Set when the user aborts via the quit prompt's `a` answer; main exits
    /// non-zero so callers like git abandon the operation.
    quit_abort: bool,
    minibuffer: Minibuffer,
    minibuffer_buffer: Buffer,
    minibuffer_pane: Pane,
    isearch: Option<ISearchState>,
    /// Persistent OS clipboard connection (see [`OsClipboard`]).
    os_clipboard: OsClipboard,
    /// Tracks the previously executed command (for consecutive-command detection).
    last_command: Option<Command>,
    /// Tracks last recenter position for C-l cycling.
    last_recenter_position: Option<RecenterPosition>,
    /// Buffer ids still awaiting a save-confirm answer during a quit.
    quit_pending: Vec<usize>,
}

impl Editor {
    pub fn buffers(&self) -> &[Buffer] {
        &self.buffers
    }

    pub fn clipboard(&self) -> &str {
        &self.clipboard
    }

    pub fn pane_tree(&self) -> &PaneTree {
        &self.pane_tree
    }

    pub fn minibuffer(&self) -> &Minibuffer {
        &self.minibuffer
    }

    pub fn minibuffer_buffer(&self) -> &Buffer {
        &self.minibuffer_buffer
    }

    pub fn minibuffer_pane(&self) -> &Pane {
        &self.minibuffer_pane
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn quit_aborted(&self) -> bool {
        self.quit_abort
    }

    pub fn isearch(&self) -> Option<&ISearchState> {
        self.isearch.as_ref()
    }

    pub(crate) fn set_pending_display_message(&mut self, message: String) {
        self.minibuffer.show_message(message);
    }

    #[cfg(test)]
    pub(crate) fn set_clipboard_for_test(&mut self, text: &str) {
        self.clipboard = text.to_string();
    }

    #[cfg(test)]
    pub(crate) fn set_cwd_for_test(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
    }

    #[cfg(test)]
    pub(crate) fn enable_buffer_syntax_for_test(
        &mut self,
        index: usize,
        language: crate::syntax::Language,
    ) {
        self.buffers[index].enable_syntax(language);
    }

    #[cfg(test)]
    pub(crate) fn rename_buffer_for_test(&mut self, index: usize, name: &str) {
        self.buffers[index].rename(name.to_string());
    }

    #[cfg(test)]
    pub(crate) fn set_minibuffer_fixture(&mut self, text: &str, point: usize) {
        self.minibuffer_buffer.reset_transient_text(text);
        self.minibuffer_pane.set_point(point);
    }

    #[cfg(test)]
    pub(crate) fn set_focused_preferred_column_for_test(&mut self, column: Option<usize>) {
        self.pane_tree.set_focused_preferred_column(column);
    }

    #[cfg(test)]
    pub(crate) fn set_minibuffer_completions_for_test(&mut self, candidates: Vec<String>) {
        self.minibuffer.set_completion_candidates(candidates);
    }

    #[cfg(test)]
    pub(crate) fn start_prompt_for_test(
        &mut self,
        kind: crate::minibuffer::PromptKind,
        label: &str,
    ) {
        self.minibuffer.start_prompt(kind, label);
    }

    #[cfg(test)]
    pub(crate) fn show_message_for_test(&mut self, message: &str) {
        self.minibuffer.show_message(message.to_string());
    }

    pub(crate) fn set_pane_focus(&mut self, path: Vec<usize>) {
        self.pane_tree.set_focus_path(path);
    }

    pub(crate) fn set_focused_point(&mut self, point: usize) {
        self.pane_tree.set_focused_point(point);
    }

    pub(crate) fn clear_focused_preferred_column(&mut self) {
        self.pane_tree.set_focused_preferred_column(None);
    }

    pub(crate) fn set_pane_scroll_position(&mut self, path: &[usize], top: usize, offset: usize) {
        self.pane_tree.set_pane_scroll_position(path, top, offset);
    }

    pub(crate) fn update_pane_viewport(&mut self, path: &[usize], height: usize, width: usize) {
        self.pane_tree.update_pane_viewport(path, height, width);
    }

    pub(crate) fn update_minibuffer_viewport(&mut self, height: usize, width: usize) {
        self.minibuffer_pane.set_viewport(height, width);
    }

    pub(crate) fn accept_syntax_completion(
        &self,
        completion: crate::syntax::SyntaxCompletion,
    ) -> (bool, bool) {
        let Some(buffer) = self.buffers.iter().find(|buffer| {
            buffer
                .syntax()
                .is_some_and(|syntax| syntax.background_key() == completion.key)
        }) else {
            return (false, false);
        };
        let syntax = buffer.syntax().expect("matching syntax state disappeared");
        let accepted = syntax.accept_background_completion(completion, buffer.edit_generation());
        (accepted, accepted && syntax.take_disabled_message())
    }

    pub fn new() -> Self {
        let buf = Buffer::new_scratch(0);
        let mut mb_pane = Pane::new(MINIBUFFER_BUFFER_ID);
        mb_pane.set_viewport(1, mb_pane.viewport_width());
        Self {
            buffers: vec![buf],
            next_buffer_id: 1,
            pane_tree: PaneTree::new(0),
            clipboard: String::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            should_quit: false,
            quit_abort: false,
            minibuffer: Minibuffer::new(),
            minibuffer_buffer: Buffer::from_str(MINIBUFFER_BUFFER_ID, "*minibuffer*", ""),
            minibuffer_pane: mb_pane,
            isearch: None,
            os_clipboard: OsClipboard::new(),
            last_command: None,
            last_recenter_position: None,
            quit_pending: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn new_with_text(text: &str) -> Self {
        let buf = Buffer::from_str(0, "*scratch*", text);
        let mut mb_pane = Pane::new(MINIBUFFER_BUFFER_ID);
        mb_pane.set_viewport(1, mb_pane.viewport_width());
        Self {
            buffers: vec![buf],
            next_buffer_id: 1,
            pane_tree: PaneTree::new(0),
            clipboard: String::new(),
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            should_quit: false,
            quit_abort: false,
            minibuffer: Minibuffer::new(),
            minibuffer_buffer: Buffer::from_str(MINIBUFFER_BUFFER_ID, "*minibuffer*", ""),
            minibuffer_pane: mb_pane,
            isearch: None,
            os_clipboard: OsClipboard::new(),
            last_command: None,
            last_recenter_position: None,
            quit_pending: Vec::new(),
        }
    }

    pub fn clear_last_command(&mut self) {
        self.last_command = None;
    }

    pub fn current_buffer(&self) -> &Buffer {
        let bid = self.pane_tree.focused_pane().buffer_id();
        self.buffers
            .iter()
            .find(|b| b.id() == bid)
            .expect("current buffer must exist")
    }

    fn current_buffer_mut(&mut self) -> &mut Buffer {
        let bid = self.pane_tree.focused_pane().buffer_id();
        self.buffers
            .iter_mut()
            .find(|b| b.id() == bid)
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
    fn active_buffer_mut(&mut self) -> &mut Buffer {
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

    fn set_active_point(&mut self, point: usize) {
        if self.minibuffer.is_active() {
            self.minibuffer_pane.set_point(point);
        } else {
            self.pane_tree.set_focused_point(point);
        }
    }

    fn set_active_mark(&mut self, mark: Option<usize>) {
        if self.minibuffer.is_active() {
            self.minibuffer_pane.set_mark(mark);
        } else {
            self.pane_tree.set_focused_mark(mark);
        }
    }

    fn set_active_preferred_column(&mut self, column: Option<usize>) {
        if self.minibuffer.is_active() {
            self.minibuffer_pane.set_preferred_column(column);
        } else {
            self.pane_tree.set_focused_preferred_column(column);
        }
    }

    fn set_active_point_and_preferred(&mut self, point: usize, column: Option<usize>) {
        if self.minibuffer.is_active() {
            self.minibuffer_pane.set_point(point);
            self.minibuffer_pane.set_preferred_column(column);
        } else {
            self.pane_tree
                .set_focused_point_and_preferred(point, column);
        }
    }

    fn set_active_point_mark_and_preferred(
        &mut self,
        point: usize,
        mark: Option<usize>,
        column: Option<usize>,
    ) {
        if self.minibuffer.is_active() {
            self.minibuffer_pane
                .set_point_mark_and_preferred(point, mark, column);
        } else {
            self.pane_tree
                .set_focused_point_mark_and_preferred(point, mark, column);
        }
    }

    fn set_active_scroll_position(&mut self, top: usize, offset: usize) {
        if self.minibuffer.is_active() {
            self.minibuffer_pane.set_scroll_position(top, offset);
        } else {
            self.pane_tree.set_focused_scroll_position(top, offset);
        }
    }

    /// Get buffer by id.
    pub fn buffer_by_id(&self, id: usize) -> &Buffer {
        self.buffers
            .iter()
            .find(|b| b.id() == id)
            .expect("buffer must exist")
    }

    #[cfg(test)]
    pub fn buffer_text(&self) -> String {
        self.current_buffer().text().to_string()
    }

    #[cfg(test)]
    pub fn point(&self) -> usize {
        self.pane_tree.focused_pane().point()
    }

    #[cfg(test)]
    pub fn commit_undo_group(&mut self) {
        self.active_buffer_mut().commit_history();
    }

    pub fn buffer_names(&self) -> Vec<String> {
        self.buffers()
            .iter()
            .map(|b| b.name().to_string())
            .collect()
    }

    /// Completion lifecycle intent for ordinary minibuffer input.
    pub(crate) fn dismiss_minibuffer_completions(&mut self) {
        self.minibuffer.dismiss_completions();
    }

    /// Apply one Tab-completion result while keeping candidate display,
    /// prefix replacement, undo history, minibuffer point, and paging under
    /// Editor/Minibuffer ownership.
    pub(crate) fn apply_minibuffer_completion(
        &mut self,
        input: &str,
        completed: String,
        candidates: Vec<String>,
    ) {
        let had_completions = self.minibuffer.has_completions();
        self.minibuffer.set_completion_candidates(candidates);

        if completed != input {
            self.minibuffer_buffer.commit_history();
            let old_len = self.minibuffer_buffer.char_count();
            self.apply_edit(0, old_len, &completed, EditRecord::Replace);
            self.minibuffer_buffer.commit_history();
            self.minibuffer_pane.set_point(completed.chars().count());
            self.minibuffer.reset_completion_page();
        } else if had_completions && self.minibuffer.has_completions() {
            self.minibuffer.advance_completion_page();
        } else {
            self.minibuffer.reset_completion_page();
        }
    }

    /// Get the ordered region (start, end) for the focused pane.
    pub fn region(&self) -> Option<(usize, usize)> {
        let pane = self.pane_tree.focused_pane();
        pane.mark().map(|mark| {
            let start = pane.point().min(mark);
            let end = pane.point().max(mark);
            (start, end)
        })
    }

    pub fn execute(&mut self, cmd: Command) {
        if !self.minibuffer.is_active()
            && self.current_buffer().is_read_only()
            && Self::command_modifies_buffer(&cmd)
        {
            self.last_command = None;
            self.minibuffer.show_message("Buffer is read-only".to_string());
            return;
        }

        // When minibuffer is active, intercept InsertNewline (submit) and InsertTab (complete)
        if self.minibuffer.is_active() {
            match &cmd {
                Command::InsertNewline => {
                    self.submit_prompt();
                    return;
                }
                Command::IndentLine | Command::DedentLine => {
                    // Tab completion is handled in app.rs key routing
                    return;
                }
                // Freeze pane layout and focus while a prompt is active:
                // prompts that resolve their target at submit time (e.g.
                // C-x C-w writes the focused pane's buffer) must not be
                // retargeted mid-prompt. Mouse clicks are already ignored.
                Command::SplitVertical
                | Command::SplitHorizontal
                | Command::DeletePane
                | Command::DeleteOtherPanes
                | Command::CycleFocus => {
                    return;
                }
                _ => {}
            }
        }

        // Mark non-edit actions for undo grouping
        match &cmd {
            Command::InsertChar(_) | Command::InsertNewline => {}
            Command::IndentLine | Command::DedentLine => {}
            Command::DeleteBackward
            | Command::DeleteForward
            | Command::DeleteWordBackward
            | Command::KillLine => {}
            Command::Undo | Command::Redo => {}
            _ => {
                self.active_buffer_mut().mark_history_action();
            }
        }

        let mut this_command = Some(cmd.clone());

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
            Command::IndentLine => self.indent_line(),
            Command::DedentLine => self.dedent_line(),
            Command::DeleteBackward => self.delete_backward(),
            Command::DeleteForward => self.delete_forward(),
            Command::DeleteWordBackward => self.delete_word_backward(),
            Command::KillLine => {
                // A C-k that killed nothing doesn't start or extend a kill
                // chain: the next C-k must not append to the previous kill.
                if !self.kill_line() {
                    this_command = None;
                }
            }
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
            Command::DescribeBindings => self.describe_bindings(),
            Command::Cancel => self.cancel(),
            Command::Quit => self.quit(),
        }

        self.last_command = this_command;

        self.ensure_cursor_visible();
    }

    fn command_modifies_buffer(cmd: &Command) -> bool {
        matches!(
            cmd,
            Command::InsertChar(_)
                | Command::InsertNewline
                | Command::IndentLine
                | Command::DedentLine
                | Command::DeleteBackward
                | Command::DeleteForward
                | Command::DeleteWordBackward
                | Command::KillLine
                | Command::Undo
                | Command::Redo
                | Command::Cut
                | Command::Paste
        )
    }

    /// Generate a `describe-bindings` buffer from the keymap trie itself.
    fn describe_bindings(&mut self) {
        const HELP_NAME: &str = "*Help*";

        let bindings = default_keymap().bindings();
        let key_width = bindings
            .iter()
            .map(|(keys, _)| keys.chars().count())
            .max()
            .unwrap_or(3)
            .max(3);
        let mut text = String::from("Key bindings\n\n");
        text.push_str(&format!("{:<key_width$}  Command\n", "Key"));
        text.push_str(&format!("{:-<key_width$}  -------\n", ""));
        for (keys, command) in bindings {
            text.push_str(&format!("{keys:<key_width$}  {}\n", command.name()));
        }

        let buffer_id = if let Some(buffer) = self.buffers.iter().find(|b| b.name() == HELP_NAME) {
            buffer.id()
        } else {
            let id = self.next_buffer_id;
            self.next_buffer_id += 1;
            self.buffers
                .push(Buffer::new_read_only(id, HELP_NAME, &text));
            id
        };
        let buffer_len = self.buffer_by_id(buffer_id).char_count();
        self.pane_tree
            .switch_focused_buffer(buffer_id, buffer_len);
    }

    /// Scroll the focused editing pane so its point is visible. Runs after
    /// every command, after minibuffer prompt submission (which can move
    /// point outside `execute()`, e.g. goto-line), and after viewport reflow.
    ///
    /// Ordinary prompts leave the underlying pane alone while their input is
    /// edited. Isearch is the exception: its prompt is active while point in
    /// the focused pane tracks the selected match, so resize reflow must keep
    /// that point visible.
    pub(crate) fn ensure_cursor_visible(&mut self) {
        if self.minibuffer.is_active() && self.isearch.is_none() {
            return;
        }
        let pane = self.pane_tree.focused_pane();
        let point = pane.point();
        let scroll_top = pane.scroll_top();
        let scroll_row_offset = pane.scroll_row_offset();
        let vh = pane.viewport_height();
        let vw = pane.viewport_width();
        let buf = self.current_buffer();
        let (line, col) = buf.char_to_line_col(point);
        // Wrap by tab-expanded visual width, matching the renderer. The
        // cursor's visual row within its own line lets sub-line scrolling
        // bring it into view even when that line is taller than the viewport.
        let (cursor_row, _) = crate::display::visual_row_col_in_line(buf, line, col, vw);
        let (new_top, new_offset) = crate::pane::compute_scroll_position(
            scroll_top,
            scroll_row_offset,
            line,
            cursor_row,
            vh,
            vw,
            |l| crate::display::line_visual_width(buf, l),
        );
        self.pane_tree
            .set_focused_scroll_position(new_top, new_offset);
    }

    // === Pane split commands ===

    fn split_vertical(&mut self) {
        let buf_id = self.pane_tree.focused_pane().buffer_id();
        self.pane_tree.split(Direction::Vertical, buf_id);
    }

    fn split_horizontal(&mut self) {
        let buf_id = self.pane_tree.focused_pane().buffer_id();
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

    /// Char index of the next grapheme-cluster boundary after `pos`.
    /// Line endings count as one step (the rope is LF-only).
    fn next_grapheme_boundary(&self, pos: usize) -> usize {
        self.active_buffer().next_grapheme_boundary(pos)
    }

    /// Char index of the previous grapheme-cluster boundary before `pos`.
    fn prev_grapheme_boundary(&self, pos: usize) -> usize {
        self.active_buffer().prev_grapheme_boundary(pos)
    }

    fn forward_char(&mut self) {
        let pos = self.active_pane().point();
        let new_pos = self.next_grapheme_boundary(pos);
        self.set_active_point_and_preferred(new_pos, None);
    }

    fn backward_char(&mut self) {
        let pos = self.active_pane().point();
        let new_pos = self.prev_grapheme_boundary(pos);
        self.set_active_point_and_preferred(new_pos, None);
    }

    fn grapheme_is_word(&self, start: usize, end: usize) -> bool {
        self.active_buffer()
            .text()
            .slice(start..end)
            .chars()
            .any(is_word_char)
    }

    fn forward_word_position(&self, pos: usize) -> usize {
        let buffer = self.active_buffer();
        let mut pos = buffer.snap_to_grapheme_boundary(pos);
        let end = buffer.char_count();

        while pos < end {
            let next = buffer.next_grapheme_boundary(pos);
            if self.grapheme_is_word(pos, next) {
                break;
            }
            pos = next;
        }
        while pos < end {
            let next = buffer.next_grapheme_boundary(pos);
            if !self.grapheme_is_word(pos, next) {
                break;
            }
            pos = next;
        }
        pos
    }

    fn backward_word_position(&self, pos: usize) -> usize {
        let buffer = self.active_buffer();
        let mut pos = buffer.snap_to_grapheme_boundary(pos);

        while pos > 0 {
            let previous = buffer.prev_grapheme_boundary(pos);
            if self.grapheme_is_word(previous, pos) {
                break;
            }
            pos = previous;
        }
        while pos > 0 {
            let previous = buffer.prev_grapheme_boundary(pos);
            if !self.grapheme_is_word(previous, pos) {
                break;
            }
            pos = previous;
        }
        pos
    }

    fn forward_word(&mut self) {
        let pos = self.forward_word_position(self.active_pane().point());
        self.set_active_point_and_preferred(pos, None);
    }

    fn backward_word(&mut self) {
        let pos = self.backward_word_position(self.active_pane().point());
        self.set_active_point_and_preferred(pos, None);
    }

    fn delete_word_backward(&mut self) {
        let start = self.active_pane().point();
        if start == 0 {
            return;
        }

        let pos = self.backward_word_position(start);

        // Delete from pos to start
        self.apply_edit(pos, start, "", EditRecord::Delete);
        self.set_active_point(pos);
        self.set_active_preferred_column(None);
    }

    fn next_line(&mut self) {
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point());
        let target_col = pane.preferred_column().unwrap_or(col);

        if line + 1 < buf.line_count() {
            // Snap the landing point out of any grapheme cluster the raw
            // column falls into; the remembered column stays unsnapped.
            let new_point =
                buf.snap_to_grapheme_boundary(buf.line_col_to_char(line + 1, target_col));
            self.set_active_point_and_preferred(new_point, Some(target_col));
        }
    }

    fn previous_line(&mut self) {
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point());
        let target_col = pane.preferred_column().unwrap_or(col);

        if line > 0 {
            let new_point =
                buf.snap_to_grapheme_boundary(buf.line_col_to_char(line - 1, target_col));
            self.set_active_point_and_preferred(new_point, Some(target_col));
        }
    }

    fn beginning_of_line(&mut self) {
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point());
        let new_point = buf.line_col_to_char(line, 0);
        self.set_active_point_and_preferred(new_point, None);
    }

    fn end_of_line(&mut self) {
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point());
        let line_len = buf.line_len_chars(line);
        let new_point = buf.line_col_to_char(line, line_len);
        self.set_active_point_and_preferred(new_point, None);
    }

    fn buffer_beginning(&mut self) {
        self.set_active_point_and_preferred(0, None);
    }

    fn buffer_end(&mut self) {
        let len = self.active_buffer().char_count();
        self.set_active_point_and_preferred(len, None);
    }

    fn page_down(&mut self) {
        let height = self.active_pane().viewport_height();
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point());
        let target_col = pane.preferred_column().unwrap_or(col);
        let new_line = (line + height).min(buf.line_count().saturating_sub(1));
        let new_point = buf.snap_to_grapheme_boundary(buf.line_col_to_char(new_line, target_col));
        self.set_active_point_and_preferred(new_point, Some(target_col));
    }

    fn page_up(&mut self) {
        let height = self.active_pane().viewport_height();
        let buf = self.active_buffer();
        let pane = self.active_pane();
        let (line, col) = buf.char_to_line_col(pane.point());
        let target_col = pane.preferred_column().unwrap_or(col);
        let new_line = line.saturating_sub(height);
        let new_point = buf.snap_to_grapheme_boundary(buf.line_col_to_char(new_line, target_col));
        self.set_active_point_and_preferred(new_point, Some(target_col));
    }

    fn recenter_top_bottom(&mut self) {
        let pane = self.active_pane();
        let buf = self.active_buffer();
        let (cursor_line, _) = buf.char_to_line_col(pane.point());
        let height = pane.viewport_height();

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

        // Recentering is line-granular; reset any sub-line offset so the
        // chosen line starts at the top of its slot. `ensure_cursor_visible`
        // (run after every command) re-applies an offset if the cursor's
        // visual row would otherwise fall below a taller-than-viewport line.
        self.set_active_scroll_position(new_scroll_top, 0);
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
                buf.text().slice(start..end).chars().collect()
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
                EditRecord::Insert => buf.record_insert(start, text),
                EditRecord::Delete => buf.record_delete(start, &deleted),
                EditRecord::Replace => buf.record_history_replace(start, &deleted, text),
                EditRecord::NoHistory => {}
            }
            buf.replace(start, end, text);
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
            (buf.id(), delta, deleted)
        };

        if buffer_id == MINIBUFFER_BUFFER_ID {
            // The minibuffer buffer is viewed only by the minibuffer pane.
            self.minibuffer_pane.adjust_for_edit(buffer_id, delta);
        } else {
            self.pane_tree.adjust_for_edit(buffer_id, delta);
        }
        deleted
    }

    fn insert_char(&mut self, c: char) {
        let pos = self.active_pane().point();
        let s = c.to_string();
        self.apply_edit(pos, pos, &s, EditRecord::Insert);
        self.set_active_point_and_preferred(pos + 1, None);
    }

    fn insert_newline(&mut self) {
        let pos = self.active_pane().point();
        let buf = self.active_buffer();

        // Get current line's leading whitespace (spaces only)
        let (line, _) = buf.char_to_line_col(pos);
        let line_start = buf.line_col_to_char(line, 0);
        let line_len = buf.line_len_chars(line);
        let mut indent = String::new();
        for ch in buf.text().chars_at(line_start).take(line_len) {
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

        // The rope is LF-only; `LineEnding` only matters at save time.
        let insert_str = format!("\n{indent}");
        self.apply_edit(pos, pos, &insert_str, EditRecord::Insert);
        self.set_active_point_and_preferred(pos + insert_str.chars().count(), None);
    }

    fn indent_line(&mut self) {
        if self.active_region().is_some() {
            self.indent_region();
            return;
        }
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point());
        let line_start = buf.line_col_to_char(line, 0);

        // Get current leading whitespace
        let line_len = buf.line_len_chars(line);
        let ws_len = buf
            .text()
            .chars_at(line_start)
            .take(line_len)
            .take_while(|ch| *ch == ' ')
            .count();
        let old_ws: String = " ".repeat(ws_len);
        let new_ws = format!("{}{}", " ".repeat(INDENT_WIDTH), old_ws);

        let old_point = self.active_pane().point();
        self.apply_edit(
            line_start,
            line_start + ws_len,
            &new_ws,
            EditRecord::Replace,
        );
        self.active_buffer_mut().commit_history();
        self.set_active_point_and_preferred(old_point + INDENT_WIDTH, None);
    }

    fn dedent_line(&mut self) {
        if self.active_region().is_some() {
            self.dedent_region();
            return;
        }
        let buf = self.active_buffer();
        let (line, _) = buf.char_to_line_col(self.active_pane().point());
        let line_start = buf.line_col_to_char(line, 0);

        let line_len = buf.line_len_chars(line);
        let remaining_ws_len = buf
            .text()
            .chars_at(line_start)
            .take(line_len)
            .take_while(|ch| *ch == ' ')
            .count();
        let remove_count = remaining_ws_len.min(INDENT_WIDTH);

        if remove_count == 0 {
            return;
        }

        // We remove these spaces, so new whitespace is empty for those chars
        let new_ws: String = " ".repeat(remaining_ws_len - remove_count);

        let old_point = self.active_pane().point();
        self.apply_edit(
            line_start,
            line_start + remaining_ws_len,
            &new_ws,
            EditRecord::Replace,
        );
        self.active_buffer_mut().commit_history();

        // Adjust point, but don't go before line start.
        let point_col = old_point - line_start;
        let point = if point_col <= remove_count {
            line_start
        } else {
            old_point - remove_count
        };
        self.set_active_point_and_preferred(point, None);
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
        let point = pane.point();
        let mark = pane.mark().unwrap_or(point);

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
                .text()
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
        self.active_buffer_mut().commit_history();
        self.set_active_point_mark_and_preferred(new_point, Some(new_mark), None);
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
        let point = pane.point();
        let mark = pane.mark().unwrap_or(point);

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
                .text()
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
        self.active_buffer_mut().commit_history();
        self.set_active_point_mark_and_preferred(new_point, Some(new_mark), None);
    }

    fn delete_backward(&mut self) {
        let pos = self.active_pane().point();
        if pos > 0 {
            // Delete a whole grapheme cluster as a unit.
            let start = self.prev_grapheme_boundary(pos);
            self.apply_edit(start, pos, "", EditRecord::Delete);
            self.set_active_point_and_preferred(start, None);
        }
    }

    fn delete_forward(&mut self) {
        let len = self.active_buffer().char_count();
        let pos = self.active_pane().point();
        if pos < len {
            // Delete a whole grapheme cluster as a unit.
            let end = self.next_grapheme_boundary(pos);
            self.apply_edit(pos, end, "", EditRecord::Delete);
            self.set_active_preferred_column(None);
        }
    }

    /// Kill to end of line. Returns whether any text was killed: `C-k` at
    /// the very end of the buffer kills nothing, and must leave both the
    /// internal and the OS clipboard untouched (overwriting the OS clipboard
    /// with the stale previous kill would clobber whatever another program
    /// put there).
    fn kill_line(&mut self) -> bool {
        let append = self.last_command == Some(Command::KillLine);
        let buf = self.active_buffer();
        let pos = self.active_pane().point();
        let (line, col) = buf.char_to_line_col(pos);
        let line_len = buf.line_len_chars(line);

        let end = if col == line_len {
            if pos == buf.char_count() {
                // Nothing to kill.
                if !self.minibuffer.is_active() {
                    self.minibuffer.show_message("End of buffer".to_string());
                }
                return false;
            }
            // At EOL, kill the terminating LF.
            pos + crate::buffer::line_break_len_chars(buf.text().line(line))
        } else {
            buf.line_col_to_char(line, line_len)
        };
        let deleted = self.apply_edit(pos, end, "", EditRecord::Delete);
        if append {
            self.clipboard.push_str(&deleted);
        } else {
            self.clipboard = deleted;
        }
        self.active_buffer_mut().commit_history();
        self.set_os_clipboard(&self.clipboard.clone());
        self.set_active_preferred_column(None);
        true
    }

    // === Undo/Redo ===

    fn undo(&mut self) {
        if let Some(group) = self.active_buffer_mut().undo() {
            for edit in group.edits.iter().rev() {
                // Reverse the edit: replace what it inserted with what it
                // deleted. Lengths are char counts, matching positions.
                let end = edit.position + edit.inserted.chars().count();
                self.apply_edit(edit.position, end, &edit.deleted, EditRecord::NoHistory);
            }
            if let Some(first) = group.edits.first() {
                let len = self.active_buffer().char_count();
                self.set_active_point(first.position.min(len));
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
        if let Some(group) = self.active_buffer_mut().redo() {
            for edit in &group.edits {
                // Re-apply the edit: replace what it deleted with what it
                // inserted. Lengths are char counts, matching positions.
                let end = edit.position + edit.deleted.chars().count();
                self.apply_edit(edit.position, end, &edit.inserted, EditRecord::NoHistory);
            }
            if let Some(last) = group.edits.last() {
                let len = self.active_buffer().char_count();
                self.set_active_point((last.position + last.inserted.chars().count()).min(len));
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
        let point = self.active_pane().point();
        self.set_active_mark(Some(point));
        self.minibuffer.show_message("Mark set".to_string());
    }

    fn swap_point_and_mark(&mut self) {
        let pane = self.active_pane();
        if let Some(mark) = pane.mark() {
            let old_point = pane.point();
            self.set_active_point_mark_and_preferred(
                mark,
                Some(old_point),
                pane.preferred_column(),
            );
        } else {
            self.minibuffer.show_message("No mark set".to_string());
        }
    }

    /// Get region from the active pane (minibuffer pane or focused pane).
    fn active_region(&self) -> Option<(usize, usize)> {
        let pane = self.active_pane();
        pane.mark().map(|mark| {
            let start = pane.point().min(mark);
            let end = pane.point().max(mark);
            (start, end)
        })
    }

    fn cut(&mut self) {
        if let Some((start, end)) = self.active_region() {
            let text = self.apply_edit(start, end, "", EditRecord::Delete);
            self.active_buffer_mut().commit_history();
            self.clipboard = text.clone();
            self.set_os_clipboard(&text);
            let preferred_column = self.active_pane().preferred_column();
            self.set_active_point_mark_and_preferred(start, None, preferred_column);
        } else {
            self.minibuffer
                .show_message("No region selected".to_string());
        }
    }

    fn copy(&mut self) {
        if let Some((start, end)) = self.active_region() {
            let text: String = self
                .active_buffer()
                .text()
                .slice(start..end)
                .chars()
                .collect();
            self.clipboard = text.clone();
            self.set_os_clipboard(&text);
            self.set_active_mark(None);
            self.minibuffer.show_message("Region copied".to_string());
        } else {
            self.minibuffer
                .show_message("No region selected".to_string());
        }
    }

    /// Normalize text about to be pasted. The minibuffer is single-line, so
    /// every line-break form becomes a space. Buffers get every break form
    /// unified to `\n` — the rope is LF-only regardless of the buffer's
    /// save-time `LineEnding` — so pasting CRLF text cannot smuggle in raw
    /// `\r` chars, which would render invisibly.
    pub(crate) fn normalized_paste(&self, text: &str) -> String {
        let unified = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.minibuffer.is_active() {
            unified.replace('\n', " ")
        } else {
            unified
        }
    }

    fn paste(&mut self) {
        let text = self
            .get_os_clipboard()
            .unwrap_or_else(|| self.clipboard().to_string());
        if text.is_empty() {
            // Preserve C-y's existing empty-paste behavior: it resets the
            // goal column but does not create an edit or undo boundary.
            self.set_active_preferred_column(None);
            return;
        }
        self.paste_supplied_text(&text);
    }

    /// Insert text supplied by an input event as one editor-owned paste
    /// transaction. Both bracketed paste and C-y converge here after C-y has
    /// obtained clipboard text. Normalization, completion lifecycle, undo
    /// grouping, point/goal-column updates, and cursor reveal stay together.
    pub(crate) fn paste_supplied_text(&mut self, text: &str) {
        if !self.minibuffer.is_active() && self.current_buffer().is_read_only() {
            self.clear_last_command();
            self.minibuffer.show_message("Buffer is read-only".to_string());
            return;
        }

        self.clear_last_command();
        if self.minibuffer.is_active() {
            self.minibuffer.dismiss_completions();
        }

        // Preserve bracketed empty-paste semantics: it is an undo boundary
        // and dismisses completions, but does not reset the goal column.
        self.active_buffer_mut().commit_history();
        let text = self.normalized_paste(text);
        if !text.is_empty() {
            let pos = self.active_pane().point();
            self.apply_edit(pos, pos, &text, EditRecord::Insert);
            self.set_active_point(pos + text.chars().count());
            self.set_active_preferred_column(None);
        }
        self.active_buffer_mut().commit_history();
        self.ensure_cursor_visible();
    }

    fn set_os_clipboard(&mut self, text: &str) {
        self.os_clipboard.set_text(text);
    }

    fn get_os_clipboard(&mut self) -> Option<String> {
        self.os_clipboard.get_text()
    }

    fn cancel(&mut self) {
        if let Some(isearch) = self.isearch.take() {
            // Restore original position
            let (point, scroll_top, scroll_row_offset) = isearch.original_view();
            self.pane_tree
                .restore_focused_view(point, scroll_top, scroll_row_offset);
            self.minibuffer.finish();
            self.minibuffer.show_message("Quit".to_string());
        } else if self.minibuffer.is_active() {
            self.minibuffer.cancel();
        } else {
            self.pane_tree.set_focused_mark(None);
            self.minibuffer.show_message("Quit".to_string());
        }
    }
}

#[cfg(test)]
mod tests;
