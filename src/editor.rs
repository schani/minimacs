use std::path::{Path, PathBuf};

use ratatui::layout::Direction;

use crate::buffer::Buffer;
use crate::command::Command;
use crate::minibuffer::{Minibuffer, PromptKind, normalize_path_string};
use crate::pane::{Pane, PaneTree};

const INDENT_WIDTH: usize = 4;

/// Position for recenter-top-bottom cycling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecenterPosition {
    Center,
    Top,
    Bottom,
}

/// Direction of incremental search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// State for incremental search.
#[derive(Debug)]
pub struct ISearchState {
    pub query: String,
    pub direction: SearchDirection,
    /// Position before search started (to restore on C-g).
    pub original_point: usize,
    pub original_scroll_top: usize,
    /// Current match position (char offset of match start).
    pub current_match: Option<usize>,
}

#[allow(dead_code)]
pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub next_buffer_id: usize,
    pub pane_tree: PaneTree,
    pub clipboard: String,
    pub cwd: PathBuf,
    pub should_quit: bool,
    pub pending_keys: String,
    pub minibuffer: Minibuffer,
    pub minibuffer_buffer: Buffer,
    pub minibuffer_pane: Pane,
    pub isearch: Option<ISearchState>,
    /// Tracks the previously executed command (for consecutive-command detection).
    last_command: Option<Command>,
    /// Tracks last recenter position for C-l cycling.
    last_recenter_position: Option<RecenterPosition>,
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
            pending_keys: String::new(),
            minibuffer: Minibuffer::new(),
            minibuffer_buffer: Buffer::from_str(usize::MAX, "*minibuffer*", ""),
            minibuffer_pane: mb_pane,
            isearch: None,
            last_command: None,
            last_recenter_position: None,
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
            pending_keys: String::new(),
            minibuffer: Minibuffer::new(),
            minibuffer_buffer: Buffer::from_str(usize::MAX, "*minibuffer*", ""),
            minibuffer_pane: mb_pane,
            isearch: None,
            last_command: None,
            last_recenter_position: None,
        }
    }

    pub fn open_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        // Check if file is already open
        for buf in &self.buffers {
            if let Some(ref bp) = buf.path {
                let buf_canonical = std::fs::canonicalize(bp).unwrap_or_else(|_| bp.clone());
                if buf_canonical == canonical {
                    let pane = self.pane_tree.focused_pane_mut();
                    pane.buffer_id = buf.id;
                    pane.point = 0;
                    pane.scroll_top = 0;
                    self.minibuffer
                        .show_message(format!("Switched to buffer {}", buf.name));
                    return Ok(());
                }
            }
        }

        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        let buf = match Buffer::from_file(id, &canonical) {
            Ok(buf) => buf,
            Err(_) if !canonical.exists() => {
                // File doesn't exist yet — create a new empty buffer with the path
                Buffer::new_for_path(id, &canonical)
            }
            Err(e) => return Err(e),
        };
        let name = buf.name.clone();
        let msg = if buf.path.as_ref().is_some_and(|p| p.exists()) {
            format!("Opened {}", name)
        } else {
            format!("(New file) {}", name)
        };
        self.buffers.push(buf);
        let pane = self.pane_tree.focused_pane_mut();
        pane.buffer_id = id;
        pane.point = 0;
        pane.scroll_top = 0;
        self.minibuffer.show_message(msg);
        Ok(())
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
            Command::DeleteBackward | Command::DeleteForward | Command::DeleteWordBackward
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

        // After any command, ensure cursor is visible (skip for minibuffer)
        if !self.minibuffer.is_active() {
            let point = self.pane_tree.focused_pane().point;
            let (line, _) = self.current_buffer().char_to_line_col(point);
            self.pane_tree.focused_pane_mut().ensure_visible(line);
        }
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

    // === Minibuffer prompt lifecycle ===

    /// Start a minibuffer prompt. No-op if already active (prompt guard).
    fn start_minibuffer_prompt(&mut self, kind: PromptKind, label: &str) {
        if self.minibuffer.is_active() {
            return;
        }
        self.minibuffer.start_prompt(kind, label);
        self.minibuffer_buffer.text = ropey::Rope::new();
        self.minibuffer_buffer.modified = false;
        self.minibuffer_buffer.history = crate::history::History::new();
        self.minibuffer_pane.point = 0;
        self.minibuffer_pane.mark = None;
        self.minibuffer_pane.scroll_top = 0;
        self.minibuffer_pane.preferred_column = None;
    }

    /// Start a minibuffer prompt with initial text. No-op if already active.
    fn start_minibuffer_prompt_with_input(&mut self, kind: PromptKind, label: &str, input: &str) {
        if self.minibuffer.is_active() {
            return;
        }
        self.minibuffer.start_prompt(kind, label);
        self.minibuffer_buffer.text = ropey::Rope::from_str(input);
        self.minibuffer_buffer.modified = false;
        self.minibuffer_buffer.history = crate::history::History::new();
        self.minibuffer_pane.point = input.chars().count();
        self.minibuffer_pane.mark = None;
        self.minibuffer_pane.scroll_top = 0;
        self.minibuffer_pane.preferred_column = None;
    }

    /// Read the current minibuffer text.
    pub fn minibuffer_text(&self) -> String {
        self.minibuffer_buffer.text.to_string()
    }

    fn find_file_prompt(&mut self) {
        let initial = format!("{}/", self.cwd.display());
        self.start_minibuffer_prompt_with_input(PromptKind::FindFile, "Find file: ", &initial);
    }

    fn switch_buffer_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::SwitchBuffer, "Switch to buffer: ");
    }

    fn write_file_prompt(&mut self) {
        let initial = format!("{}/", self.cwd.display());
        self.start_minibuffer_prompt_with_input(PromptKind::WriteFile, "Write file: ", &initial);
    }

    fn goto_line_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::GotoLine, "Goto line: ");
    }

    pub fn submit_prompt(&mut self) {
        let kind = match self.minibuffer.prompt() {
            Some(p) => p.kind.clone(),
            None => return,
        };
        let input = self.minibuffer_text();

        match kind {
            PromptKind::FindFile => {
                self.minibuffer.finish();
                let path = PathBuf::from(normalize_path_string(&input));
                if let Err(e) = self.open_file(&path) {
                    self.minibuffer.show_message(format!("{}", e));
                }
            }
            PromptKind::SwitchBuffer => {
                self.minibuffer.finish();
                self.switch_to_buffer(&input);
            }
            PromptKind::WriteFile => {
                self.minibuffer.finish();
                let path = PathBuf::from(normalize_path_string(&input));
                self.current_buffer_mut().path = Some(path.clone());
                self.current_buffer_mut().name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| input.clone());
                self.save();
            }
            PromptKind::GotoLine => {
                self.minibuffer.finish();
                if let Ok(line_num) = input.parse::<usize>() {
                    if line_num > 0 {
                        let target_line = line_num - 1;
                        let char_pos = self.current_buffer().line_col_to_char(target_line, 0);
                        self.pane_tree.focused_pane_mut().point = char_pos;
                    }
                } else {
                    self.minibuffer
                        .show_message("Invalid line number".to_string());
                }
            }
            PromptKind::ISearch => {
                // Enter during isearch accepts the position
                self.isearch_accept();
            }
            PromptKind::SaveConfirm { buffer_name } => {
                self.minibuffer.finish();
                match input.as_str() {
                    "y" | "Y" => {
                        if let Some(buf) =
                            self.buffers.iter_mut().find(|b| b.name == buffer_name)
                        {
                            if buf.path.is_some() {
                                let _ = buf.save();
                            }
                        }
                        self.should_quit = true;
                    }
                    "n" | "N" => {
                        self.should_quit = true;
                    }
                    "q" | "Q" => {
                        self.minibuffer.show_message("Quit".to_string());
                    }
                    _ => {
                        self.minibuffer
                            .show_message("Please answer y, n, or q".to_string());
                    }
                }
            }
        }
    }

    fn switch_to_buffer(&mut self, name: &str) {
        if let Some(buf) = self.buffers.iter().find(|b| b.name == name) {
            let id = buf.id;
            let pane = self.pane_tree.focused_pane_mut();
            pane.buffer_id = id;
            pane.point = 0;
            pane.scroll_top = 0;
        } else {
            self.minibuffer
                .show_message(format!("No buffer named '{}'", name));
        }
    }

    fn kill_buffer(&mut self) {
        if self.minibuffer.is_active() {
            return;
        }
        let is_modified = self.current_buffer().modified;
        let name = self.current_buffer().name.clone();

        if is_modified {
            self.start_minibuffer_prompt(
                PromptKind::SaveConfirm {
                    buffer_name: name.clone(),
                },
                &format!("Buffer {} modified; kill anyway? (y/n) ", name),
            );
            return;
        }

        let current_id = self.pane_tree.focused_pane().buffer_id;
        self.do_kill_buffer(current_id);
    }

    fn do_kill_buffer(&mut self, buffer_id: usize) {
        self.buffers.retain(|b| b.id != buffer_id);

        let new_id = if self.buffers.is_empty() {
            let buf = Buffer::new_scratch(self.next_buffer_id);
            self.next_buffer_id += 1;
            let id = buf.id;
            self.buffers.push(buf);
            id
        } else {
            self.buffers[0].id
        };

        // Update all panes that referenced the killed buffer.
        self.pane_tree.for_each_pane_mut(&mut |pane| {
            if pane.buffer_id == buffer_id {
                pane.buffer_id = new_id;
                pane.point = 0;
                pane.scroll_top = 0;
            }
        });
    }

    // === Movement commands ===

    fn forward_char(&mut self) {
        let len = self.active_buffer().char_count();
        let pane = self.active_pane_mut();
        if pane.point < len {
            pane.point += 1;
        }
        pane.preferred_column = None;
    }

    fn backward_char(&mut self) {
        let pane = self.active_pane_mut();
        if pane.point > 0 {
            pane.point -= 1;
        }
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
        let deleted: String = self
            .active_buffer()
            .text
            .slice(pos..start)
            .chars()
            .collect();
        self.active_buffer_mut()
            .history
            .record_delete(pos, &deleted);
        self.active_buffer_mut().remove(pos, start);
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
        let position = match if is_consecutive { self.last_recenter_position } else { None } {
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

    fn insert_char(&mut self, c: char) {
        let pos = self.active_pane().point;
        let s = c.to_string();
        self.active_buffer_mut().history.record_insert(pos, &s);
        self.active_buffer_mut().insert(pos, &s);
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

        let insert_str = format!("{}{}", le, indent);
        self.active_buffer_mut()
            .history
            .record_insert(pos, &insert_str);
        self.active_buffer_mut().insert(pos, &insert_str);
        let pane = self.active_pane_mut();
        pane.point = pos + insert_str.len();
        pane.preferred_column = None;
    }

    fn insert_tab(&mut self) {
        let pos = self.active_pane().point;
        let spaces = " ".repeat(INDENT_WIDTH);
        self.active_buffer_mut()
            .history
            .record_insert(pos, &spaces);
        self.active_buffer_mut().insert(pos, &spaces);
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

        self.active_buffer_mut()
            .history
            .record_replace(line_start, &old_ws, &new_ws);
        self.active_buffer_mut().remove(line_start, line_start + ws_len);
        self.active_buffer_mut().insert(line_start, &new_ws);
        self.active_buffer_mut().history.commit();
        let pane = self.active_pane_mut();
        pane.point += INDENT_WIDTH;
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

        self.active_buffer_mut()
            .history
            .record_replace(line_start, &" ".repeat(remaining_ws_len), &new_ws);
        self.active_buffer_mut()
            .remove(line_start, line_start + remaining_ws_len);
        self.active_buffer_mut().insert(line_start, &new_ws);
        self.active_buffer_mut().history.commit();

        let pane = self.active_pane_mut();
        // Adjust point, but don't go before line start
        let point_col = pane.point - line_start;
        if point_col <= remove_count {
            pane.point = line_start;
        } else {
            pane.point -= remove_count;
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

        let old_text: String = buf.text.slice(span_start..span_end).chars().collect();
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
            let line_text: String = buf.text.slice(line_start..next_line_start).chars().collect();

            new_text.push_str(&indent_str);
            new_text.push_str(&line_text);

            // Adjust point and mark if they're on this line
            if point >= line_start
                && (point < next_line_start
                    || (line_idx == last_line && point == next_line_start))
            {
                new_point = point + INDENT_WIDTH * (line_idx - first_line + 1);
            }
            if mark >= line_start
                && (mark < next_line_start
                    || (line_idx == last_line && mark == next_line_start))
            {
                new_mark = mark + INDENT_WIDTH * (line_idx - first_line + 1);
            }

        }

        self.active_buffer_mut()
            .history
            .record_replace(span_start, &old_text, &new_text);
        self.active_buffer_mut().remove(span_start, span_end);
        self.active_buffer_mut().insert(span_start, &new_text);
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

        let old_text: String = buf.text.slice(span_start..span_end).chars().collect();

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
            let line_text: String = buf.text.slice(line_start..next_line_start).chars().collect();

            // Count leading spaces to remove (up to INDENT_WIDTH)
            let remove_count = line_text.chars().take(INDENT_WIDTH).take_while(|&c| c == ' ').count();

            new_text.push_str(&line_text[remove_count..]);
            cumulative_delta += remove_count;

            // Adjust point
            if point >= line_start && (point < next_line_start || (line_idx == last_line && point == next_line_start)) {
                let col = point - line_start;
                if col <= remove_count {
                    new_point = line_start - (cumulative_delta - remove_count);
                } else {
                    new_point = point - cumulative_delta;
                }
            }
            // Adjust mark
            if mark >= line_start && (mark < next_line_start || (line_idx == last_line && mark == next_line_start)) {
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

        self.active_buffer_mut()
            .history
            .record_replace(span_start, &old_text, &new_text);
        self.active_buffer_mut().remove(span_start, span_end);
        self.active_buffer_mut().insert(span_start, &new_text);
        self.active_buffer_mut().history.commit();

        let pane = self.active_pane_mut();
        pane.point = new_point;
        pane.mark = Some(new_mark);
        pane.preferred_column = None;
    }

    fn delete_backward(&mut self) {
        let pos = self.active_pane().point;
        if pos > 0 {
            let deleted: String = self
                .active_buffer()
                .text
                .slice(pos - 1..pos)
                .chars()
                .collect();
            self.active_buffer_mut()
                .history
                .record_delete(pos - 1, &deleted);
            self.active_buffer_mut().remove(pos - 1, pos);
            let pane = self.active_pane_mut();
            pane.point = pos - 1;
            pane.preferred_column = None;
        }
    }

    fn delete_forward(&mut self) {
        let len = self.active_buffer().char_count();
        let pos = self.active_pane().point;
        if pos < len {
            let deleted: String = self
                .active_buffer()
                .text
                .slice(pos..pos + 1)
                .chars()
                .collect();
            self.active_buffer_mut()
                .history
                .record_delete(pos, &deleted);
            self.active_buffer_mut().remove(pos, pos + 1);
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
                let deleted: String = buf.text.slice(pos..pos + 1).chars().collect();
                self.active_buffer_mut()
                    .history
                    .record_delete(pos, &deleted);
                self.active_buffer_mut().remove(pos, pos + 1);
                if append {
                    self.clipboard.push_str(&deleted);
                } else {
                    self.clipboard = deleted;
                }
            }
        } else {
            let end = buf.line_col_to_char(line, line_len);
            let deleted: String = buf.text.slice(pos..end).chars().collect();
            self.active_buffer_mut()
                .history
                .record_delete(pos, &deleted);
            self.active_buffer_mut().remove(pos, end);
            if append {
                self.clipboard.push_str(&deleted);
            } else {
                self.clipboard = deleted;
            }
        }
        self.active_buffer_mut().history.commit();
        self.active_pane_mut().preferred_column = None;
    }

    // === Undo/Redo ===

    fn undo(&mut self) {
        if let Some(group) = self.active_buffer_mut().history.undo() {
            for edit in group.edits.iter().rev() {
                if !edit.inserted.is_empty() {
                    let end = edit.position + edit.inserted.len();
                    self.active_buffer_mut().remove(edit.position, end);
                }
                if !edit.deleted.is_empty() {
                    self.active_buffer_mut()
                        .insert(edit.position, &edit.deleted);
                }
            }
            if let Some(first) = group.edits.first() {
                self.active_pane_mut().point = first.position;
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
                if !edit.deleted.is_empty() {
                    let end = edit.position + edit.deleted.len();
                    self.active_buffer_mut().remove(edit.position, end);
                }
                if !edit.inserted.is_empty() {
                    self.active_buffer_mut()
                        .insert(edit.position, &edit.inserted);
                }
            }
            if let Some(last) = group.edits.last() {
                self.active_pane_mut().point =
                    last.position + last.inserted.len();
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

    // === File operations ===

    fn save(&mut self) {
        let has_path = self.current_buffer().path.is_some();
        if !has_path {
            self.write_file_prompt();
            return;
        }
        match self.current_buffer_mut().save() {
            Ok(()) => {
                let name = self.current_buffer().name.clone();
                self.minibuffer.show_message(format!("Wrote {}", name));
            }
            Err(e) => {
                self.minibuffer
                    .show_message(format!("Error saving: {}", e));
            }
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
            let text: String = self
                .active_buffer()
                .text
                .slice(start..end)
                .chars()
                .collect();
            self.active_buffer_mut()
                .history
                .record_delete(start, &text);
            self.active_buffer_mut().remove(start, end);
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
            self.active_buffer_mut()
                .history
                .record_insert(pos, &text);
            self.active_buffer_mut().insert(pos, &text);
            self.active_buffer_mut().history.commit();
            self.active_pane_mut().point = pos + text.len();
        }
        self.active_pane_mut().preferred_column = None;
    }

    fn set_os_clipboard(&self, _text: &str) {
        #[cfg(feature = "clipboard")]
        {
            if let Ok(mut clip) = arboard::Clipboard::new() {
                let _ = clip.set_text(_text.to_string());
            }
        }
    }

    fn get_os_clipboard(&self) -> Option<String> {
        #[cfg(feature = "clipboard")]
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

    // === Incremental Search ===

    fn isearch_start(&mut self, direction: SearchDirection) {
        if self.minibuffer.is_active() {
            return;
        }
        let pane = self.pane_tree.focused_pane();
        self.isearch = Some(ISearchState {
            query: String::new(),
            direction,
            original_point: pane.point,
            original_scroll_top: pane.scroll_top,
            current_match: None,
        });
        let label = match direction {
            SearchDirection::Forward => "I-search: ",
            SearchDirection::Backward => "I-search backward: ",
        };
        self.start_minibuffer_prompt(PromptKind::ISearch, label);
    }

    /// Called when isearch input changes — find next match from current position.
    pub fn isearch_update(&mut self) {
        let (query, direction) = match &self.isearch {
            Some(s) => (s.query.clone(), s.direction),
            None => return,
        };
        if query.is_empty() {
            // Restore to original position
            if let Some(ref isearch) = self.isearch {
                let pane = self.pane_tree.focused_pane_mut();
                pane.point = isearch.original_point;
                pane.scroll_top = isearch.original_scroll_top;
            }
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = None;
            }
            return;
        }
        let buf = self.current_buffer();
        let text: String = buf.text.chars().collect();
        let original_char = match &self.isearch {
            Some(s) => s.original_point,
            None => 0,
        };
        // Convert char offset to byte offset for string searching
        let search_from_byte: usize = text.chars().take(original_char).map(|c| c.len_utf8()).sum();

        let found = match direction {
            SearchDirection::Forward => {
                text[search_from_byte..].find(&query).map(|byte_i| {
                    text[..search_from_byte + byte_i].chars().count()
                })
            }
            SearchDirection::Backward => {
                text[..search_from_byte].rfind(&query).map(|byte_i| {
                    text[..byte_i].chars().count()
                })
            }
        };

        if let Some(char_pos) = found {
            let pane = self.pane_tree.focused_pane_mut();
            pane.point = char_pos;
            let (line, _) = self.current_buffer().char_to_line_col(char_pos);
            self.pane_tree.focused_pane_mut().ensure_visible(line);
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = Some(char_pos);
            }
        } else {
            self.minibuffer.show_message("Failing I-search".to_string());
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = None;
            }
        }
    }

    /// Cycle to next/previous match during isearch.
    pub fn isearch_next(&mut self) {
        let (query, direction) = match &self.isearch {
            Some(s) => (s.query.clone(), s.direction),
            None => return,
        };
        if query.is_empty() {
            return;
        }
        let buf = self.current_buffer();
        let text: String = buf.text.chars().collect();
        let current_point = self.pane_tree.focused_pane().point;

        let found = match direction {
            SearchDirection::Forward => {
                // Search from current_point + 1
                let start = (current_point + 1).min(text.chars().count());
                // Convert char offset to byte offset for string search
                let byte_start: usize = text.chars().take(start).map(|c| c.len_utf8()).sum();
                text[byte_start..].find(&query).map(|byte_i| {
                    start + text[byte_start..byte_start + byte_i].chars().count()
                })
            }
            SearchDirection::Backward => {
                let byte_end: usize = text.chars().take(current_point).map(|c| c.len_utf8()).sum();
                text[..byte_end].rfind(&query).map(|byte_i| {
                    text[..byte_i].chars().count()
                })
            }
        };

        if let Some(char_pos) = found {
            let pane = self.pane_tree.focused_pane_mut();
            pane.point = char_pos;
            let (line, _) = self.current_buffer().char_to_line_col(char_pos);
            self.pane_tree.focused_pane_mut().ensure_visible(line);
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = Some(char_pos);
            }
        } else {
            self.minibuffer.show_message(format!("Failing I-search: {}", query));
        }
    }

    /// Accept the current isearch position.
    pub fn isearch_accept(&mut self) {
        self.isearch = None;
        self.minibuffer.finish();
    }

    /// Get all visible match positions for rendering (char offset, query_len).
    pub fn isearch_matches(&self) -> Vec<(usize, usize)> {
        let isearch = match &self.isearch {
            Some(s) if !s.query.is_empty() => s,
            _ => return Vec::new(),
        };
        let buf = self.current_buffer();
        let text: String = buf.text.chars().collect();
        let query = &isearch.query;
        let query_char_len = query.chars().count();

        let mut matches = Vec::new();
        let mut search_start = 0usize; // byte offset
        let mut char_offset = 0usize;

        while let Some(byte_pos) = text[search_start..].find(query) {
            let match_char = char_offset
                + text[search_start..search_start + byte_pos].chars().count();
            matches.push((match_char, query_char_len));
            // Advance past this match by one char
            let next_byte = search_start + byte_pos
                + text[search_start + byte_pos..].chars().next().map_or(1, |c| c.len_utf8());
            char_offset = match_char + 1;
            search_start = next_byte;
        }

        matches
    }

    fn quit(&mut self) {
        if self.minibuffer.is_active() {
            return;
        }
        for buf in &self.buffers {
            if buf.modified {
                let name = buf.name.clone();
                self.start_minibuffer_prompt(
                    PromptKind::SaveConfirm {
                        buffer_name: name.clone(),
                    },
                    &format!("Save buffer {}? (y/n/q) ", name),
                );
                return;
            }
        }
        self.should_quit = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_char_moves_one() {
        let mut editor = Editor::new_with_text("hello");
        assert_eq!(editor.point(), 0);
        editor.execute(Command::ForwardChar);
        assert_eq!(editor.point(), 1);
    }

    #[test]
    fn forward_char_stops_at_end() {
        let mut editor = Editor::new_with_text("hi");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::ForwardChar);
        assert_eq!(editor.point(), 2);
    }

    #[test]
    fn backward_char_moves_one() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 3;
        editor.execute(Command::BackwardChar);
        assert_eq!(editor.point(), 2);
    }

    #[test]
    fn backward_char_stops_at_start() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::BackwardChar);
        assert_eq!(editor.point(), 0);
    }

    #[test]
    fn next_line_basic() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::NextLine);
        assert_eq!(editor.point(), 8);
    }

    #[test]
    fn next_line_clamps_to_shorter_line() {
        let mut editor = Editor::new_with_text("hello\nhi");
        editor.pane_tree.focused_pane_mut().point = 4;
        editor.execute(Command::NextLine);
        assert_eq!(editor.point(), 8);
    }

    #[test]
    fn next_line_preserves_preferred_column() {
        let mut editor = Editor::new_with_text("hello\nhi\nworld");
        editor.pane_tree.focused_pane_mut().point = 4;
        editor.execute(Command::NextLine);
        editor.execute(Command::NextLine);
        assert_eq!(editor.point(), 13);
    }

    #[test]
    fn previous_line_basic() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 8;
        editor.execute(Command::PreviousLine);
        assert_eq!(editor.point(), 2);
    }

    #[test]
    fn beginning_of_line() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 8;
        editor.execute(Command::BeginningOfLine);
        assert_eq!(editor.point(), 6);
    }

    #[test]
    fn end_of_line() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 6;
        editor.execute(Command::EndOfLine);
        assert_eq!(editor.point(), 11);
    }

    #[test]
    fn insert_char_basic() {
        let mut editor = Editor::new_with_text("hllo");
        editor.pane_tree.focused_pane_mut().point = 1;
        editor.execute(Command::InsertChar('e'));
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.point(), 2);
    }

    #[test]
    fn insert_newline() {
        let mut editor = Editor::new_with_text("helloworld");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::InsertNewline);
        assert_eq!(editor.buffer_text(), "hello\nworld");
        assert_eq!(editor.point(), 6);
    }

    #[test]
    fn insert_tab_inserts_four_spaces() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertTab);
        assert_eq!(editor.buffer_text(), "    ");
        assert_eq!(editor.point(), 4);
    }

    #[test]
    fn delete_backward_basic() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 3;
        editor.execute(Command::DeleteBackward);
        assert_eq!(editor.buffer_text(), "helo");
        assert_eq!(editor.point(), 2);
    }

    #[test]
    fn delete_backward_at_start_does_nothing() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::DeleteBackward);
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.point(), 0);
    }

    #[test]
    fn delete_forward_basic() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::DeleteForward);
        assert_eq!(editor.buffer_text(), "helo");
        assert_eq!(editor.point(), 2);
    }

    #[test]
    fn delete_forward_at_end_does_nothing() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::DeleteForward);
        assert_eq!(editor.buffer_text(), "hello");
    }

    #[test]
    fn scroll_follows_cursor_down() {
        let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk");
        editor.pane_tree.focused_pane_mut().viewport_height = 3;
        editor.pane_tree.focused_pane_mut().scroll_top = 0;
        for _ in 0..8 {
            editor.execute(Command::NextLine);
        }
        let (line, _) = editor
            .current_buffer()
            .char_to_line_col(editor.point());
        assert_eq!(line, 8);
        let pane = editor.pane_tree.focused_pane();
        assert!(pane.scroll_top <= line);
        assert!(line < pane.scroll_top + pane.viewport_height);
    }

    #[test]
    fn undo_reverses_insert() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertChar('a'));
        editor.execute(Command::InsertChar('b'));
        editor.commit_undo_group();
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertChar('x'));
        editor.commit_undo_group();
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "");
        editor.execute(Command::Redo);
        assert_eq!(editor.buffer_text(), "x");
    }

    #[test]
    fn undo_reverses_delete() {
        let mut editor = Editor::new_with_text("abc");
        editor.pane_tree.focused_pane_mut().point = 3;
        editor.execute(Command::DeleteBackward);
        editor.commit_undo_group();
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "abc");
    }

    #[test]
    fn kill_line_from_middle() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::KillLine);
        assert_eq!(editor.buffer_text(), "he\nworld");
        assert_eq!(editor.clipboard, "llo");
    }

    #[test]
    fn kill_line_at_eol() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::KillLine);
        assert_eq!(editor.buffer_text(), "helloworld");
        assert_eq!(editor.clipboard, "\n");
    }

    #[test]
    fn buffer_beginning_and_end() {
        let mut editor = Editor::new_with_text("hello\nworld");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::BufferBeginning);
        assert_eq!(editor.point(), 0);
        editor.execute(Command::BufferEnd);
        assert_eq!(editor.point(), 11);
    }

    #[test]
    fn page_down_and_up() {
        let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        editor.pane_tree.focused_pane_mut().viewport_height = 3;
        editor.execute(Command::PageDown);
        let (line, _) = editor
            .current_buffer()
            .char_to_line_col(editor.point());
        assert_eq!(line, 3);
        editor.execute(Command::PageUp);
        let (line, _) = editor
            .current_buffer()
            .char_to_line_col(editor.point());
        assert_eq!(line, 0);
    }

    // === Recenter tests ===

    #[test]
    fn recenter_centers_cursor_line() {
        // 10 lines, viewport height 5, cursor on line 5
        let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        editor.pane_tree.focused_pane_mut().viewport_height = 5;
        editor.pane_tree.focused_pane_mut().viewport_width = 40;
        // Move cursor to line 5 (the "f" line)
        for _ in 0..5 {
            editor.execute(Command::NextLine);
        }
        editor.execute(Command::RecenterTopBottom);
        // Center: scroll_top = 5 - 5/2 = 3
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
    }

    #[test]
    fn recenter_cycles_center_top_bottom() {
        let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        editor.pane_tree.focused_pane_mut().viewport_height = 5;
        editor.pane_tree.focused_pane_mut().viewport_width = 40;
        // Move cursor to line 5
        for _ in 0..5 {
            editor.execute(Command::NextLine);
        }
        // First C-l: center (scroll_top = 3)
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
        // Second C-l: top (scroll_top = 5)
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 5);
        // Third C-l: bottom (scroll_top = 5 - 4 = 1)
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 1);
        // Fourth C-l: center again (scroll_top = 3)
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
    }

    #[test]
    fn recenter_resets_on_other_command() {
        let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
        editor.pane_tree.focused_pane_mut().viewport_height = 5;
        editor.pane_tree.focused_pane_mut().viewport_width = 40;
        for _ in 0..5 {
            editor.execute(Command::NextLine);
        }
        // First C-l: center
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
        // Any other command resets the cycle
        editor.execute(Command::ForwardChar);
        // Next C-l should be center again, not top
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
    }

    #[test]
    fn recenter_at_beginning_of_buffer() {
        let mut editor = Editor::new_with_text("a\nb\nc\nd\ne");
        editor.pane_tree.focused_pane_mut().viewport_height = 5;
        editor.pane_tree.focused_pane_mut().viewport_width = 40;
        // Cursor at line 0
        editor.execute(Command::RecenterTopBottom);
        // Center: 0.saturating_sub(2) = 0
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 0);
        // Top: scroll_top = 0
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 0);
        // Bottom: 0.saturating_sub(4) = 0
        editor.execute(Command::RecenterTopBottom);
        assert_eq!(editor.pane_tree.focused_pane().scroll_top, 0);
    }

    #[test]
    fn save_buffer_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        editor.execute(Command::InsertChar('X'));
        editor.execute(Command::Save);

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "Xoriginal");
    }

    #[test]
    fn save_buffer_without_path_prompts() {
        let mut editor = Editor::new();
        editor.execute(Command::Save);
        assert!(editor.minibuffer.is_active());
    }

    #[test]
    fn find_file_opens_prompt() {
        let mut editor = Editor::new();
        editor.execute(Command::FindFile);
        assert!(editor.minibuffer.is_active());
    }

    #[test]
    fn switch_buffer_opens_prompt() {
        let mut editor = Editor::new();
        editor.execute(Command::SwitchBuffer);
        assert!(editor.minibuffer.is_active());
    }

    #[test]
    fn open_same_file_twice_switches() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "content").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        let first_id = editor.pane_tree.focused_pane().buffer_id;

        editor.open_file(&file).unwrap();
        assert_eq!(editor.pane_tree.focused_pane().buffer_id, first_id);
        assert_eq!(editor.buffers.len(), 2);
    }

    #[test]
    fn kill_last_buffer_creates_scratch() {
        let mut editor = Editor::new();
        assert_eq!(editor.buffers.len(), 1);
        editor.do_kill_buffer(0);
        assert_eq!(editor.buffers.len(), 1);
        assert_eq!(editor.current_buffer().name, "*scratch*");
    }

    #[test]
    fn quit_with_unmodified_buffers() {
        let mut editor = Editor::new();
        editor.execute(Command::Quit);
        assert!(editor.should_quit);
    }

    #[test]
    fn quit_with_modified_buffer_prompts() {
        let mut editor = Editor::new();
        editor.execute(Command::InsertChar('x'));
        editor.execute(Command::Quit);
        assert!(!editor.should_quit);
        assert!(editor.minibuffer.is_active());
    }

    impl Editor {
        /// Helper: set minibuffer text directly (for tests).
        fn set_minibuffer_text(&mut self, text: &str) {
            self.minibuffer_buffer.text = ropey::Rope::from_str(text);
            self.minibuffer_pane.point = text.chars().count();
        }
    }

    #[test]
    fn goto_line_via_prompt() {
        let mut editor = Editor::new_with_text("line1\nline2\nline3\nline4");
        editor.execute(Command::GotoLine);
        editor.set_minibuffer_text("3");
        editor.submit_prompt();
        let (line, _) = editor
            .current_buffer()
            .char_to_line_col(editor.point());
        assert_eq!(line, 2);
    }

    #[test]
    fn find_file_submit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let mut editor = Editor::new();
        editor.execute(Command::FindFile);
        editor.set_minibuffer_text(&file.to_string_lossy());
        editor.submit_prompt();

        assert_eq!(editor.buffer_text(), "hello");
        assert!(!editor.minibuffer.is_active());
    }

    #[test]
    fn find_file_submit_normalizes_dot() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let mut editor = Editor::new();
        editor.execute(Command::FindFile);
        // Use /./  in path — should still open the file
        let input = format!("{}/./test.txt", dir.path().display());
        editor.set_minibuffer_text(&input);
        editor.submit_prompt();

        assert_eq!(editor.buffer_text(), "hello");
    }

    #[test]
    fn find_file_submit_normalizes_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let mut editor = Editor::new();
        editor.execute(Command::FindFile);
        // Use /../ in path — should still open the file
        let input = format!("{}/sub/../test.txt", dir.path().display());
        editor.set_minibuffer_text(&input);
        editor.submit_prompt();

        assert_eq!(editor.buffer_text(), "hello");
    }

    #[test]
    fn switch_buffer_submit() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "file content").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        editor.execute(Command::SwitchBuffer);
        editor.set_minibuffer_text("*scratch*");
        editor.submit_prompt();

        assert_eq!(editor.current_buffer().name, "*scratch*");
    }

    // === Region/Clipboard tests ===

    #[test]
    fn set_mark() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::SetMark);
        assert_eq!(editor.pane_tree.focused_pane().mark, Some(2));
    }

    #[test]
    fn swap_point_and_mark() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 1;
        editor.execute(Command::SetMark);
        editor.pane_tree.focused_pane_mut().point = 4;
        editor.execute(Command::SwapPointAndMark);
        assert_eq!(editor.point(), 1);
        assert_eq!(editor.pane_tree.focused_pane().mark, Some(4));
    }

    #[test]
    fn region_returns_ordered_range() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 4;
        editor.pane_tree.focused_pane_mut().mark = Some(1);
        let (start, end) = editor.region().unwrap();
        assert_eq!(start, 1);
        assert_eq!(end, 4);
    }

    #[test]
    fn cut_removes_region() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::SetMark);
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::Cut);
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.clipboard, " world");
        assert_eq!(editor.point(), 5);
        assert_eq!(editor.pane_tree.focused_pane().mark, None);
    }

    #[test]
    fn copy_preserves_text() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 0;
        editor.execute(Command::SetMark);
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::Copy);
        assert_eq!(editor.buffer_text(), "hello world");
        assert_eq!(editor.clipboard, "hello");
        assert_eq!(editor.pane_tree.focused_pane().mark, None);
    }

    #[test]
    fn paste_inserts_clipboard() {
        let mut editor = Editor::new_with_text("hello");
        editor.clipboard = " world".to_string();
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::Paste);
        assert_eq!(editor.buffer_text(), "hello world");
        assert_eq!(editor.point(), 11);
    }

    #[test]
    fn cancel_deactivates_mark() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::SetMark);
        assert!(editor.pane_tree.focused_pane().mark.is_some());
        editor.execute(Command::Cancel);
        assert_eq!(editor.pane_tree.focused_pane().mark, None);
    }

    #[test]
    fn cut_then_paste() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 6;
        editor.execute(Command::SetMark);
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::Cut);
        assert_eq!(editor.buffer_text(), "hello ");
        editor.pane_tree.focused_pane_mut().point = 0;
        editor.execute(Command::Paste);
        assert_eq!(editor.buffer_text(), "worldhello ");
    }

    #[test]
    fn cut_undo() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::SetMark);
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::Cut);
        assert_eq!(editor.buffer_text(), "hello");
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "hello world");
    }

    // === Pane split tests ===

    #[test]
    fn split_vertical_creates_two_panes() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::SplitVertical);
        assert_eq!(editor.pane_tree.pane_count(), 2);
    }

    #[test]
    fn split_horizontal_creates_two_panes() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::SplitHorizontal);
        assert_eq!(editor.pane_tree.pane_count(), 2);
    }

    #[test]
    fn cycle_focus_between_panes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "other").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        editor.execute(Command::SplitVertical);
        // Both panes show same buffer
        let first_bid = editor.pane_tree.focused_pane().buffer_id;
        editor.execute(Command::CycleFocus);
        let second_bid = editor.pane_tree.focused_pane().buffer_id;
        assert_eq!(first_bid, second_bid);
    }

    #[test]
    fn delete_pane_works() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::SplitVertical);
        assert_eq!(editor.pane_tree.pane_count(), 2);
        editor.execute(Command::DeletePane);
        assert_eq!(editor.pane_tree.pane_count(), 1);
    }

    #[test]
    fn delete_only_pane_shows_message() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::DeletePane);
        assert_eq!(editor.pane_tree.pane_count(), 1);
    }

    #[test]
    fn delete_other_panes() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::SplitVertical);
        editor.execute(Command::SplitHorizontal);
        assert_eq!(editor.pane_tree.pane_count(), 3);
        editor.execute(Command::DeleteOtherPanes);
        assert_eq!(editor.pane_tree.pane_count(), 1);
    }

    #[test]
    fn isearch_forward_basic() {
        let mut editor = Editor::new_with_text("hello world hello");
        editor.execute(Command::ISearchForward);
        assert!(editor.isearch.is_some());
        assert!(editor.minibuffer.is_active());

        // Type "world" into search
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "world".to_string();
        }
        editor.isearch_update();
        // Should find "world" at char position 6
        assert_eq!(editor.point(), 6);
    }

    #[test]
    fn isearch_backward_basic() {
        let mut editor = Editor::new_with_text("hello world hello");
        // Start at end
        editor.pane_tree.focused_pane_mut().point = 17;
        editor.execute(Command::ISearchBackward);
        assert!(editor.isearch.is_some());

        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "hello".to_string();
        }
        editor.isearch_update();
        // Should find "hello" at position 12 (second occurrence, backward from 17)
        assert_eq!(editor.point(), 12);
    }

    #[test]
    fn isearch_cancel_restores_position() {
        let mut editor = Editor::new_with_text("hello world");
        assert_eq!(editor.point(), 0);
        editor.execute(Command::ISearchForward);

        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "world".to_string();
        }
        editor.isearch_update();
        assert_eq!(editor.point(), 6); // Found at "world"

        // Cancel should restore
        editor.execute(Command::Cancel);
        assert_eq!(editor.point(), 0);
        assert!(editor.isearch.is_none());
    }

    #[test]
    fn isearch_accept_keeps_position() {
        let mut editor = Editor::new_with_text("hello world");
        editor.execute(Command::ISearchForward);

        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "world".to_string();
        }
        editor.isearch_update();
        assert_eq!(editor.point(), 6);

        editor.isearch_accept();
        assert_eq!(editor.point(), 6); // Position kept
        assert!(editor.isearch.is_none());
    }

    #[test]
    fn isearch_next_cycles() {
        let mut editor = Editor::new_with_text("aaa bbb aaa bbb aaa");
        editor.execute(Command::ISearchForward);

        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "aaa".to_string();
        }
        editor.isearch_update();
        assert_eq!(editor.point(), 0); // First "aaa"

        editor.isearch_next();
        assert_eq!(editor.point(), 8); // Second "aaa"

        editor.isearch_next();
        assert_eq!(editor.point(), 16); // Third "aaa"
    }

    #[test]
    fn isearch_matches_returns_all() {
        let mut editor = Editor::new_with_text("abcabcabc");
        editor.execute(Command::ISearchForward);

        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "abc".to_string();
        }
        editor.isearch_update();

        let matches = editor.isearch_matches();
        assert_eq!(matches.len(), 3);
        assert_eq!(matches[0], (0, 3));
        assert_eq!(matches[1], (3, 3));
        assert_eq!(matches[2], (6, 3));
    }

    // === Word movement tests ===

    #[test]
    fn forward_word_basic() {
        let mut editor = Editor::new_with_text("hello world");
        editor.execute(Command::ForwardWord);
        assert_eq!(editor.point(), 5); // End of "hello"
    }

    #[test]
    fn forward_word_skips_non_word() {
        let mut editor = Editor::new_with_text("hello   world");
        editor.execute(Command::ForwardWord);
        assert_eq!(editor.point(), 5); // End of "hello"
        editor.execute(Command::ForwardWord);
        assert_eq!(editor.point(), 13); // End of "world"
    }

    #[test]
    fn forward_word_at_end() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::ForwardWord);
        assert_eq!(editor.point(), 5); // Stays at end
    }

    #[test]
    fn forward_word_with_underscore() {
        let mut editor = Editor::new_with_text("foo_bar baz");
        editor.execute(Command::ForwardWord);
        assert_eq!(editor.point(), 7); // End of "foo_bar"
    }

    #[test]
    fn backward_word_basic() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::BackwardWord);
        assert_eq!(editor.point(), 6); // Start of "world"
    }

    #[test]
    fn backward_word_skips_non_word() {
        let mut editor = Editor::new_with_text("hello   world");
        editor.pane_tree.focused_pane_mut().point = 13;
        editor.execute(Command::BackwardWord);
        assert_eq!(editor.point(), 8); // Start of "world"
        editor.execute(Command::BackwardWord);
        assert_eq!(editor.point(), 0); // Start of "hello"
    }

    #[test]
    fn backward_word_at_start() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::BackwardWord);
        assert_eq!(editor.point(), 0); // Stays at start
    }

    #[test]
    fn backward_word_with_underscore() {
        let mut editor = Editor::new_with_text("foo_bar baz");
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::BackwardWord);
        assert_eq!(editor.point(), 8); // Start of "baz"
        editor.execute(Command::BackwardWord);
        assert_eq!(editor.point(), 0); // Start of "foo_bar"
    }

    // === Delete word backward tests ===

    #[test]
    fn delete_word_backward_basic() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::DeleteWordBackward);
        assert_eq!(editor.buffer_text(), "hello ");
        assert_eq!(editor.point(), 6);
    }

    #[test]
    fn delete_word_backward_with_spaces() {
        let mut editor = Editor::new_with_text("hello   world");
        editor.pane_tree.focused_pane_mut().point = 13;
        editor.execute(Command::DeleteWordBackward);
        assert_eq!(editor.buffer_text(), "hello   ");
        assert_eq!(editor.point(), 8);
    }

    #[test]
    fn delete_word_backward_at_start() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::DeleteWordBackward);
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.point(), 0);
    }

    // === Non-existent file tests ===

    #[test]
    fn open_nonexistent_file_creates_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new_file.txt");
        assert!(!file.exists());

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();

        assert_eq!(editor.current_buffer().name, "new_file.txt");
        assert_eq!(editor.buffer_text(), "");
        assert_eq!(
            editor.current_buffer().path.as_ref().unwrap().file_name(),
            Some(std::ffi::OsStr::new("new_file.txt"))
        );
    }

    #[test]
    fn open_nonexistent_file_save_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("new_file.txt");
        assert!(!file.exists());

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        editor.execute(Command::InsertChar('h'));
        editor.execute(Command::InsertChar('i'));
        editor.execute(Command::Save);

        assert!(file.exists());
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hi");
    }

    #[test]
    fn delete_word_backward_undo() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 11;
        editor.execute(Command::DeleteWordBackward);
        assert_eq!(editor.buffer_text(), "hello ");
        editor.commit_undo_group();
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "hello world");
    }

    #[test]
    fn undo_with_nothing_to_undo() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::Undo);
        assert_eq!(
            editor.minibuffer.message,
            Some("No further undo information".to_string())
        );
    }

    #[test]
    fn redo_with_nothing_to_redo() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::Redo);
        assert_eq!(
            editor.minibuffer.message,
            Some("No further redo information".to_string())
        );
    }

    #[test]
    fn swap_point_and_mark_no_mark() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::SwapPointAndMark);
        assert_eq!(
            editor.minibuffer.message,
            Some("No mark set".to_string())
        );
    }

    #[test]
    fn cut_no_region() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::Cut);
        assert_eq!(
            editor.minibuffer.message,
            Some("No region selected".to_string())
        );
    }

    #[test]
    fn copy_no_region() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::Copy);
        assert_eq!(
            editor.minibuffer.message,
            Some("No region selected".to_string())
        );
    }

    #[test]
    fn cancel_active_minibuffer() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::FindFile);
        assert!(editor.minibuffer.is_active());
        editor.execute(Command::Cancel);
        assert!(!editor.minibuffer.is_active());
    }

    #[test]
    fn write_file_prompt() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::WriteFile);
        assert!(editor.minibuffer.is_active());
        let prompt = editor.minibuffer.prompt().unwrap();
        assert_eq!(prompt.kind, PromptKind::WriteFile);
    }

    #[test]
    fn kill_buffer_unmodified() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::KillBuffer);
        // Killing the only buffer creates a new scratch buffer
        assert_eq!(editor.current_buffer().name, "*scratch*");
        assert_eq!(editor.buffer_text(), "");
    }

    #[test]
    fn kill_buffer_modified_prompts() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertChar('x'));
        assert!(editor.current_buffer().modified);
        editor.execute(Command::KillBuffer);
        assert!(editor.minibuffer.is_active());
        let prompt = editor.minibuffer.prompt().unwrap();
        assert!(matches!(prompt.kind, PromptKind::SaveConfirm { .. }));
    }

    #[test]
    fn kill_buffer_with_others_remaining() {
        let dir = tempfile::tempdir().unwrap();
        let file1 = dir.path().join("a.txt");
        let file2 = dir.path().join("b.txt");
        std::fs::write(&file1, "aaa").unwrap();
        std::fs::write(&file2, "bbb").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file1).unwrap();
        editor.open_file(&file2).unwrap();
        assert_eq!(editor.buffers.len(), 3); // scratch + a.txt + b.txt
        let current_id = editor.pane_tree.focused_pane().buffer_id;
        editor.execute(Command::KillBuffer);
        // Buffer was killed, switched to first remaining
        assert!(editor.buffers.iter().all(|b| b.id != current_id));
    }

    #[test]
    fn goto_line_invalid_input() {
        let mut editor = Editor::new_with_text("line1\nline2\nline3");
        editor.execute(Command::GotoLine);
        editor.set_minibuffer_text("abc");
        editor.submit_prompt();
        assert_eq!(
            editor.minibuffer.message,
            Some("Invalid line number".to_string())
        );
    }

    #[test]
    fn switch_to_nonexistent_buffer() {
        let mut editor = Editor::new_with_text("hello");
        editor.execute(Command::SwitchBuffer);
        editor.set_minibuffer_text("nonexistent");
        editor.submit_prompt();
        assert_eq!(
            editor.minibuffer.message,
            Some("No buffer named 'nonexistent'".to_string())
        );
    }

    #[test]
    fn submit_write_file_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("output.txt");

        let mut editor = Editor::new_with_text("content");
        editor.execute(Command::WriteFile);
        editor.set_minibuffer_text(&file.to_string_lossy());
        editor.submit_prompt();

        assert!(file.exists());
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "content");
    }

    #[test]
    fn save_confirm_yes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        editor.execute(Command::InsertChar('X'));
        // Simulate quit which triggers save confirm
        editor.execute(Command::Quit);
        assert!(editor.minibuffer.is_active());
        editor.set_minibuffer_text("y");
        editor.submit_prompt();
        assert!(editor.should_quit);
        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "Xoriginal");
    }

    #[test]
    fn save_confirm_no() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertChar('X'));
        editor.execute(Command::Quit);
        assert!(editor.minibuffer.is_active());
        editor.set_minibuffer_text("n");
        editor.submit_prompt();
        assert!(editor.should_quit);
    }

    #[test]
    fn save_confirm_quit() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertChar('X'));
        editor.execute(Command::Quit);
        assert!(editor.minibuffer.is_active());
        editor.set_minibuffer_text("q");
        editor.submit_prompt();
        assert!(!editor.should_quit);
    }

    #[test]
    fn save_confirm_invalid() {
        let mut editor = Editor::new_with_text("");
        editor.execute(Command::InsertChar('X'));
        editor.execute(Command::Quit);
        assert!(editor.minibuffer.is_active());
        editor.set_minibuffer_text("x");
        editor.submit_prompt();
        assert!(!editor.should_quit);
        assert_eq!(
            editor.minibuffer.message,
            Some("Please answer y, n, or q".to_string())
        );
    }

    #[test]
    fn isearch_no_match() {
        let mut editor = Editor::new_with_text("hello world");
        editor.execute(Command::ISearchForward);
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "xyz".to_string();
        }
        editor.isearch_update();
        assert_eq!(
            editor.minibuffer.message,
            Some("Failing I-search".to_string())
        );
    }

    #[test]
    fn isearch_next_no_more_matches() {
        let mut editor = Editor::new_with_text("hello world");
        editor.execute(Command::ISearchForward);
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "hello".to_string();
        }
        editor.isearch_update();
        // Try to cycle — no more matches
        editor.isearch_next();
        assert!(editor.minibuffer.message.as_ref().unwrap().contains("Failing"));
    }

    #[test]
    fn isearch_backward_finds_match() {
        let mut editor = Editor::new_with_text("hello world hello");
        // Move to end
        editor.pane_tree.focused_pane_mut().point = 17;
        editor.execute(Command::ISearchBackward);
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "hello".to_string();
        }
        editor.isearch_update();
        // rfind from position 17 finds the last "hello" before cursor = position 12
        assert_eq!(editor.point(), 12);
    }

    #[test]
    fn isearch_next_backward() {
        let mut editor = Editor::new_with_text("ab ab ab");
        editor.pane_tree.focused_pane_mut().point = 8;
        editor.execute(Command::ISearchBackward);
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "ab".to_string();
        }
        editor.isearch_update();
        // First backward match from end
        let first = editor.point();
        editor.isearch_next();
        // Should find an earlier match
        assert!(editor.point() < first || editor.minibuffer.message.as_ref().unwrap().contains("Failing"));
    }

    #[test]
    fn isearch_empty_query_restores() {
        let mut editor = Editor::new_with_text("hello world");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::ISearchForward);
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = "world".to_string();
        }
        editor.isearch_update();
        assert_eq!(editor.point(), 6);
        // Clear query → should restore
        if let Some(ref mut isearch) = editor.isearch {
            isearch.query = String::new();
        }
        editor.isearch_update();
        assert_eq!(editor.point(), 5);
    }

    #[test]
    fn buffer_names_list() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        let names = editor.buffer_names();
        assert!(names.contains(&"*scratch*".to_string()));
        assert!(names.contains(&"test.txt".to_string()));
    }

    #[test]
    fn undo_restores_unmodified_state() {
        let mut editor = Editor::new_with_text("hello");
        assert!(!editor.current_buffer().modified);
        editor.execute(Command::InsertChar('X'));
        assert!(editor.current_buffer().modified);
        editor.commit_undo_group();
        editor.execute(Command::Undo);
        assert!(
            !editor.current_buffer().modified,
            "Buffer should be unmodified after undoing to original state"
        );
    }

    #[test]
    fn undo_redo_preserves_modified_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();
        assert!(!editor.current_buffer().modified);

        // Make an edit and save
        editor.execute(Command::InsertChar('X'));
        editor.execute(Command::Save);
        assert!(!editor.current_buffer().modified);

        // Make another edit
        editor.execute(Command::InsertChar('Y'));
        assert!(editor.current_buffer().modified);

        // Undo back to saved state
        editor.commit_undo_group();
        editor.execute(Command::Undo);
        assert!(
            !editor.current_buffer().modified,
            "Should be unmodified after undoing to last save point"
        );
    }

    // === Indentation tests ===

    #[test]
    fn insert_newline_copies_indentation() {
        let mut editor = Editor::new_with_text("    hello");
        // Place cursor at end of "hello"
        editor.pane_tree.focused_pane_mut().point = 9;
        editor.execute(Command::InsertNewline);
        assert_eq!(editor.buffer_text(), "    hello\n    ");
        assert_eq!(editor.point(), 14); // after the 4 spaces on new line
    }

    #[test]
    fn insert_newline_no_indent_at_column_zero() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 5;
        editor.execute(Command::InsertNewline);
        assert_eq!(editor.buffer_text(), "hello\n");
        assert_eq!(editor.point(), 6);
    }

    #[test]
    fn insert_newline_mid_line_preserves_indent() {
        let mut editor = Editor::new_with_text("    helloworld");
        editor.pane_tree.focused_pane_mut().point = 9; // between "hello" and "world"
        editor.execute(Command::InsertNewline);
        assert_eq!(editor.buffer_text(), "    hello\n    world");
        assert_eq!(editor.point(), 14);
    }

    #[test]
    fn indent_line_single() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::IndentLine);
        assert_eq!(editor.buffer_text(), "    hello");
        assert_eq!(editor.point(), 6); // 2 + 4
    }

    #[test]
    fn indent_line_with_region() {
        let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
        // Select all three lines
        let pane = editor.pane_tree.focused_pane_mut();
        pane.point = 0;
        pane.mark = Some(11); // end of "ccc"
        editor.execute(Command::IndentLine);
        assert_eq!(editor.buffer_text(), "    aaa\n    bbb\n    ccc");
    }

    #[test]
    fn dedent_line_single() {
        let mut editor = Editor::new_with_text("    hello");
        editor.pane_tree.focused_pane_mut().point = 6; // on 'l'
        editor.execute(Command::DedentLine);
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.point(), 2); // 6 - 4
    }

    #[test]
    fn dedent_line_partial_spaces() {
        let mut editor = Editor::new_with_text("  hello");
        editor.pane_tree.focused_pane_mut().point = 4;
        editor.execute(Command::DedentLine);
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.point(), 2); // 4 - 2
    }

    #[test]
    fn dedent_line_no_leading_spaces() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::DedentLine);
        assert_eq!(editor.buffer_text(), "hello");
        assert_eq!(editor.point(), 2); // unchanged
    }

    #[test]
    fn dedent_line_with_region() {
        let mut editor = Editor::new_with_text("    aaa\n    bbb\n    ccc");
        let pane = editor.pane_tree.focused_pane_mut();
        pane.point = 0;
        pane.mark = Some(23); // end of text
        editor.execute(Command::DedentLine);
        assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
    }

    #[test]
    fn region_end_at_col0_excludes_last_line() {
        let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
        // Region covers first two lines, but end is at start of "ccc"
        let pane = editor.pane_tree.focused_pane_mut();
        pane.point = 0;
        pane.mark = Some(8); // start of "ccc" line (col 0)
        editor.execute(Command::IndentLine);
        assert_eq!(editor.buffer_text(), "    aaa\n    bbb\nccc");
    }

    #[test]
    fn undo_reverses_indent() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().point = 2;
        editor.execute(Command::IndentLine);
        assert_eq!(editor.buffer_text(), "    hello");
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "hello");
    }

    #[test]
    fn undo_reverses_region_indent() {
        let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
        let pane = editor.pane_tree.focused_pane_mut();
        pane.point = 0;
        pane.mark = Some(11);
        editor.execute(Command::IndentLine);
        assert_eq!(editor.buffer_text(), "    aaa\n    bbb\n    ccc");
        editor.execute(Command::Undo);
        assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
    }

    #[test]
    fn preferred_column_cleared_after_indent() {
        let mut editor = Editor::new_with_text("hello");
        editor.pane_tree.focused_pane_mut().preferred_column = Some(5);
        editor.execute(Command::IndentLine);
        assert_eq!(editor.pane_tree.focused_pane().preferred_column, None);
    }

    #[test]
    fn preferred_column_cleared_after_dedent() {
        let mut editor = Editor::new_with_text("    hello");
        editor.pane_tree.focused_pane_mut().preferred_column = Some(5);
        editor.pane_tree.focused_pane_mut().point = 6;
        editor.execute(Command::DedentLine);
        assert_eq!(editor.pane_tree.focused_pane().preferred_column, None);
    }

    // === Consecutive kill-line accumulation tests ===

    #[test]
    fn kill_line_consecutive_accumulates() {
        // "hello\nworld\n" — three C-k's from the start should accumulate
        let mut editor = Editor::new_with_text("hello\nworld\n");
        // C-k 1: kills "hello" (rest of line), clipboard = "hello"
        editor.execute(Command::KillLine);
        assert_eq!(editor.clipboard, "hello");
        // C-k 2: kills "\n" (at EOL), clipboard = "hello\n"
        editor.execute(Command::KillLine);
        assert_eq!(editor.clipboard, "hello\n");
        // C-k 3: kills "world" (rest of line), clipboard = "hello\nworld"
        editor.execute(Command::KillLine);
        assert_eq!(editor.clipboard, "hello\nworld");
    }

    #[test]
    fn kill_line_non_consecutive_resets() {
        let mut editor = Editor::new_with_text("hello\nworld");
        // First C-k: kills "hello"
        editor.execute(Command::KillLine);
        assert_eq!(editor.clipboard, "hello");
        // Non-kill command breaks the chain
        editor.execute(Command::ForwardChar);
        // After killing "hello", buffer is "\nworld", point is at 0.
        // ForwardChar moves to pos 1 ('w'). Kill_line kills "world".
        // Since last_command was ForwardChar, clipboard is replaced.
        editor.execute(Command::KillLine);
        assert_eq!(editor.clipboard, "world");
    }

    #[test]
    fn kill_line_accumulate_then_paste() {
        let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
        // Kill first two segments: "aaa" then "\n"
        editor.execute(Command::KillLine);
        editor.execute(Command::KillLine);
        assert_eq!(editor.clipboard, "aaa\n");
        // Paste should insert the accumulated text
        editor.execute(Command::Paste);
        assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
    }
}
