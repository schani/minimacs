use std::path::{Path, PathBuf};

use ratatui::layout::Direction;

use crate::buffer::Buffer;
use crate::command::Command;
use crate::indent::INDENT_WIDTH;
use crate::minibuffer::{normalize_path_string, Minibuffer, PromptKind};
use crate::pane::{Pane, PaneTree};

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
    /// Char positions of all matches, recomputed once per query change.
    /// Navigation and rendering read this instead of rescanning the buffer.
    pub matches: Vec<usize>,
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

    pub fn open_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        // Check if file is already open
        let existing_buffer_id = self.buffers.iter().find_map(|buf| {
            let bp = buf.path.as_ref()?;
            let buf_canonical = std::fs::canonicalize(bp).unwrap_or_else(|_| bp.clone());
            (buf_canonical == canonical).then_some(buf.id)
        });
        if let Some(buffer_id) = existing_buffer_id {
            let name = self.buffer_by_id(buffer_id).name.clone();
            self.switch_focused_pane_to_buffer(buffer_id);
            self.minibuffer
                .show_message(format!("Switched to buffer {name}"));
            return Ok(());
        }

        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        let mut buf = match Buffer::from_file(id, &canonical) {
            Ok(buf) => buf,
            Err(_) if !canonical.exists() => {
                // File doesn't exist yet — create a new empty buffer with the path
                Buffer::new_for_path(id, &canonical)
            }
            Err(e) => return Err(e),
        };
        buf.name = self.unique_buffer_name(&buf.name, buf.path.as_deref());
        let name = buf.name.clone();
        let msg = if buf.path.as_ref().is_some_and(|p| p.exists()) {
            format!("Opened {name}")
        } else {
            format!("(New file) {name}")
        };
        self.buffers.push(buf);
        self.switch_focused_pane_to_buffer(id);
        self.minibuffer.show_message(msg);
        Ok(())
    }

    /// Disambiguate a buffer name against the existing buffers, emacs-style:
    /// `mod.rs` collides → `mod.rs<lib>` (trailing path components), falling
    /// back to `mod.rs<2>` when paths can't tell them apart.
    fn unique_buffer_name(&self, base: &str, path: Option<&Path>) -> String {
        self.unique_buffer_name_excluding(base, path, None)
    }

    /// Like [`unique_buffer_name`], but ignores the buffer with id `exclude`
    /// (used when renaming an existing buffer).
    fn unique_buffer_name_excluding(
        &self,
        base: &str,
        path: Option<&Path>,
        exclude: Option<usize>,
    ) -> String {
        let taken = |name: &str| {
            self.buffers
                .iter()
                .any(|b| b.name == name && Some(b.id) != exclude)
        };
        if !taken(base) {
            return base.to_string();
        }
        if let Some(parent) = path.and_then(|p| p.parent()) {
            let components: Vec<String> = parent
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            for n in 1..=components.len() {
                let qualifier = components[components.len() - n..].join("/");
                let candidate = format!("{base}<{qualifier}>");
                if !taken(&candidate) {
                    return candidate;
                }
            }
        }
        let mut i = 2;
        loop {
            let candidate = format!("{base}<{i}>");
            if !taken(&candidate) {
                return candidate;
            }
            i += 1;
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

        // After any command, ensure cursor is visible (skip for minibuffer)
        if !self.minibuffer.is_active() {
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
        let dir = self
            .current_buffer()
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(&self.cwd);
        let initial = format!("{}/", dir.display());
        self.start_minibuffer_prompt_with_input(PromptKind::FindFile, "Find file: ", &initial);
    }

    fn switch_buffer_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::SwitchBuffer, "Switch to buffer: ");
    }

    fn write_file_prompt(&mut self) {
        let dir = self
            .current_buffer()
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(&self.cwd);
        let initial = format!("{}/", dir.display());
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
                    self.minibuffer.show_message(format!("{e}"));
                }
            }
            PromptKind::SwitchBuffer => {
                self.minibuffer.finish();
                self.switch_to_buffer(&input);
            }
            PromptKind::WriteFile => {
                self.minibuffer.finish();
                let path = PathBuf::from(normalize_path_string(&input));
                let buffer_id = self.current_buffer().id;
                let own_path = self.current_buffer().path.as_deref() == Some(path.as_path());
                if path.exists() && !own_path {
                    self.start_minibuffer_prompt(
                        PromptKind::OverwriteConfirm {
                            buffer_id,
                            path: path.clone(),
                        },
                        &format!("{} exists; overwrite? (y/n) ", path.display()),
                    );
                    return;
                }
                self.write_buffer_to_path(buffer_id, path);
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
            PromptKind::KillConfirm { buffer_id } => {
                self.minibuffer.finish();
                match input.as_str() {
                    "y" | "Y" => self.do_kill_buffer(buffer_id),
                    "n" | "N" => {}
                    _ => self
                        .minibuffer
                        .show_message("Please answer y or n".to_string()),
                }
            }
            PromptKind::OverwriteConfirm { buffer_id, path } => {
                self.minibuffer.finish();
                match input.as_str() {
                    "y" | "Y" => self.write_buffer_to_path(buffer_id, path),
                    "n" | "N" => self.minibuffer.show_message("Cancelled".to_string()),
                    _ => self
                        .minibuffer
                        .show_message("Please answer y or n".to_string()),
                }
            }
            PromptKind::SaveAnywayConfirm { buffer_id } => {
                self.minibuffer.finish();
                match input.as_str() {
                    "y" | "Y" => {
                        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buffer_id) {
                            let name = buf.name.clone();
                            match buf.save() {
                                Ok(()) => {
                                    self.minibuffer.show_message(format!("Wrote {name}"));
                                }
                                Err(e) => {
                                    self.minibuffer
                                        .show_message(format!("Error saving: {e}"));
                                }
                            }
                        }
                    }
                    "n" | "N" => {
                        self.minibuffer.show_message("Save cancelled".to_string());
                    }
                    _ => self
                        .minibuffer
                        .show_message("Please answer y or n".to_string()),
                }
            }
            PromptKind::QuitSaveConfirm { buffer_id } => {
                self.minibuffer.finish();
                match input.as_str() {
                    "y" | "Y" => match self.buffers.iter_mut().find(|b| b.id == buffer_id) {
                        Some(buf) if buf.path.is_some() => {
                            let name = buf.name.clone();
                            match buf.save() {
                                Ok(()) => {
                                    self.quit_pending.retain(|&id| id != buffer_id);
                                    self.continue_quit();
                                }
                                Err(e) => {
                                    self.quit_pending.clear();
                                    self.minibuffer
                                        .show_message(format!("Could not save {name}: {e}"));
                                }
                            }
                        }
                        Some(buf) => {
                            let name = buf.name.clone();
                            self.quit_pending.clear();
                            self.minibuffer.show_message(format!(
                                "Buffer {name} has no file; save it with C-x C-w first"
                            ));
                        }
                        None => {
                            // Buffer disappeared in the meantime; skip it.
                            self.quit_pending.retain(|&id| id != buffer_id);
                            self.continue_quit();
                        }
                    },
                    "n" | "N" => {
                        self.quit_pending.retain(|&id| id != buffer_id);
                        self.continue_quit();
                    }
                    "q" | "Q" => {
                        self.quit_pending.clear();
                        self.minibuffer.show_message("Quit".to_string());
                    }
                    _ => {
                        // Re-ask for the same buffer.
                        self.continue_quit();
                    }
                }
            }
        }
    }

    /// Write a buffer to `path` (the C-x C-w flow). Buffer identity (path,
    /// name, syntax) is only updated after the save succeeds.
    fn write_buffer_to_path(&mut self, buffer_id: usize, path: PathBuf) {
        let result = {
            let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buffer_id) else {
                return;
            };
            buf.save_as(&path).map(|()| buf.redetect_syntax())
        };
        match result {
            Ok(()) => {
                let base = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let name = self.unique_buffer_name_excluding(&base, Some(&path), Some(buffer_id));
                if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buffer_id) {
                    buf.name = name.clone();
                }
                self.minibuffer.show_message(format!("Wrote {name}"));
            }
            Err(e) => {
                self.minibuffer.show_message(format!("Error saving: {e}"));
            }
        }
    }

    fn switch_focused_pane_to_buffer(&mut self, buffer_id: usize) {
        let buffer_len = self.buffer_by_id(buffer_id).char_count();
        self.pane_tree
            .focused_pane_mut()
            .switch_buffer(buffer_id, buffer_len);
    }

    fn switch_to_buffer(&mut self, name: &str) {
        let buffer_id = if name.is_empty() {
            self.pane_tree.focused_pane().alternate_buffer_id()
        } else {
            self.buffers.iter().find(|b| b.name == name).map(|b| b.id)
        };

        if let Some(buffer_id) = buffer_id {
            self.switch_focused_pane_to_buffer(buffer_id);
        } else if !name.is_empty() {
            self.minibuffer
                .show_message(format!("No buffer named '{name}'"));
        }
    }

    fn kill_buffer(&mut self) {
        if self.minibuffer.is_active() {
            return;
        }
        let buffer_id = self.pane_tree.focused_pane().buffer_id;
        let is_modified = self.current_buffer().modified;
        let name = self.current_buffer().name.clone();

        if is_modified {
            self.start_minibuffer_prompt(
                PromptKind::KillConfirm { buffer_id },
                &format!("Buffer {name} modified; kill anyway? (y/n) "),
            );
            return;
        }

        self.do_kill_buffer(buffer_id);
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
        let new_buffer_len = self.buffer_by_id(new_id).char_count();

        // Update all panes that referenced the killed buffer.
        self.pane_tree.for_each_pane_mut(&mut |pane| {
            pane.forget_buffer(buffer_id);
            if pane.buffer_id == buffer_id {
                pane.restore_buffer_state(new_id, new_buffer_len);
            }
        });
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
        let (buffer_id, start, removed, inserted, deleted) = {
            let buf = self.active_buffer_mut();
            let len = buf.char_count();
            let start = start.min(len);
            let end = end.min(len).max(start);
            let deleted: String = if end > start {
                buf.text.slice(start..end).chars().collect()
            } else {
                String::new()
            };
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
            (buf.id, start, end - start, text.chars().count(), deleted)
        };

        if buffer_id == usize::MAX {
            // The minibuffer buffer is viewed only by the minibuffer pane.
            self.minibuffer_pane
                .adjust_for_edit(buffer_id, start, removed, inserted);
        } else {
            self.pane_tree.for_each_pane_mut(&mut |pane| {
                pane.adjust_for_edit(buffer_id, start, removed, inserted);
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

    // === File operations ===

    fn save(&mut self) {
        let has_path = self.current_buffer().path.is_some();
        if !has_path {
            self.write_file_prompt();
            return;
        }
        if self.current_buffer().externally_modified() {
            let buffer_id = self.current_buffer().id;
            let name = self.current_buffer().name.clone();
            self.start_minibuffer_prompt(
                PromptKind::SaveAnywayConfirm { buffer_id },
                &format!("{name} changed on disk; save anyway? (y/n) "),
            );
            return;
        }
        match self.current_buffer_mut().save() {
            Ok(()) => {
                let name = self.current_buffer().name.clone();
                self.minibuffer.show_message(format!("Wrote {name}"));
            }
            Err(e) => {
                self.minibuffer.show_message(format!("Error saving: {e}"));
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
            matches: Vec::new(),
        });
        let label = match direction {
            SearchDirection::Forward => "I-search: ",
            SearchDirection::Backward => "I-search backward: ",
        };
        self.start_minibuffer_prompt(PromptKind::ISearch, label);
    }

    /// Find the char positions of all occurrences of `query` in the buffer.
    /// One O(buffer) scan; called only when the search query changes.
    fn compute_matches_for_query(buf: &Buffer, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return Vec::new();
        }
        let text: String = buf.text.chars().collect();
        let mut matches = Vec::new();
        let mut search_start = 0usize; // byte offset
        let mut char_offset = 0usize;
        while let Some(byte_pos) = text[search_start..].find(query) {
            let match_char =
                char_offset + text[search_start..search_start + byte_pos].chars().count();
            matches.push(match_char);
            // Advance past this match start by one char (overlaps allowed).
            let next_byte = search_start
                + byte_pos
                + text[search_start + byte_pos..]
                    .chars()
                    .next()
                    .map_or(1, |c| c.len_utf8());
            char_offset = match_char + 1;
            search_start = next_byte;
        }
        matches
    }

    /// Move point to an isearch match and scroll it into view.
    fn isearch_goto_match(&mut self, char_pos: usize) {
        self.pane_tree.focused_pane_mut().point = char_pos;
        let pane = self.pane_tree.focused_pane();
        let scroll_top = pane.scroll_top;
        let vh = pane.viewport_height;
        let vw = pane.viewport_width;
        let buf = self.current_buffer();
        let (line, _) = buf.char_to_line_col(char_pos);
        let new_top = crate::pane::compute_scroll_top(scroll_top, line, vh, vw, |l| {
            crate::render::line_visual_width(buf, l)
        });
        self.pane_tree.focused_pane_mut().scroll_top = new_top;
        if let Some(ref mut isearch) = self.isearch {
            isearch.current_match = Some(char_pos);
        }
    }

    /// Called when isearch input changes — rescan the buffer once, cache all
    /// match positions, and jump to the first match from the original point.
    pub fn isearch_update(&mut self) {
        let (query, direction, original_point) = match &self.isearch {
            Some(s) => (s.query.clone(), s.direction, s.original_point),
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
                isearch.matches.clear();
            }
            return;
        }

        let matches = Self::compute_matches_for_query(self.current_buffer(), &query);
        let query_len = query.chars().count();
        let found = match direction {
            SearchDirection::Forward => {
                matches.iter().copied().find(|&p| p >= original_point)
            }
            SearchDirection::Backward => matches
                .iter()
                .copied()
                .rev()
                .find(|&p| p + query_len <= original_point),
        };
        if let Some(ref mut isearch) = self.isearch {
            isearch.matches = matches;
        }

        if let Some(char_pos) = found {
            self.isearch_goto_match(char_pos);
        } else {
            self.minibuffer.show_message("Failing I-search".to_string());
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = None;
            }
        }
    }

    /// Cycle to next/previous match during isearch, using the cached matches.
    pub fn isearch_next(&mut self) {
        let (query, found) = match &self.isearch {
            Some(s) if !s.query.is_empty() => {
                let current_point = self.pane_tree.focused_pane().point;
                let query_len = s.query.chars().count();
                let found = match s.direction {
                    SearchDirection::Forward => {
                        s.matches.iter().copied().find(|&p| p > current_point)
                    }
                    SearchDirection::Backward => s
                        .matches
                        .iter()
                        .copied()
                        .rev()
                        .find(|&p| p + query_len <= current_point),
                };
                (s.query.clone(), found)
            }
            _ => return,
        };

        if let Some(char_pos) = found {
            self.isearch_goto_match(char_pos);
        } else {
            self.minibuffer
                .show_message(format!("Failing I-search: {query}"));
        }
    }

    /// Accept the current isearch position.
    pub fn isearch_accept(&mut self) {
        self.isearch = None;
        self.minibuffer.finish();
    }

    /// All match positions for rendering (char offset, query char length).
    /// Reads the cache built by `isearch_update` — no buffer scan per frame.
    pub fn isearch_matches(&self) -> Vec<(usize, usize)> {
        let isearch = match &self.isearch {
            Some(s) if !s.query.is_empty() => s,
            _ => return Vec::new(),
        };
        let query_char_len = isearch.query.chars().count();
        isearch
            .matches
            .iter()
            .map(|&pos| (pos, query_char_len))
            .collect()
    }

    fn quit(&mut self) {
        if self.minibuffer.is_active() {
            return;
        }
        self.quit_pending = self
            .buffers
            .iter()
            .filter(|b| b.modified)
            .map(|b| b.id)
            .collect();
        self.continue_quit();
    }

    /// Prompt for the next still-modified buffer awaiting a quit-time save
    /// decision, or quit once none remain.
    fn continue_quit(&mut self) {
        while let Some(&id) = self.quit_pending.first() {
            match self.buffers.iter().find(|b| b.id == id) {
                Some(buf) if buf.modified => {
                    let name = buf.name.clone();
                    self.start_minibuffer_prompt(
                        PromptKind::QuitSaveConfirm { buffer_id: id },
                        &format!("Save buffer {name}? (y/n/q) "),
                    );
                    return;
                }
                _ => {
                    self.quit_pending.remove(0);
                }
            }
        }
        self.should_quit = true;
    }
}

#[cfg(test)]
mod tests;
