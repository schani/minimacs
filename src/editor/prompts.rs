use std::path::PathBuf;

use crate::minibuffer::{normalize_path_string, PromptKind};

use super::fileops::WriteTarget;
use super::Editor;

impl Editor {
    // === Minibuffer prompt lifecycle ===

    /// Start a minibuffer prompt. No-op if already active (prompt guard).
    pub(super) fn start_minibuffer_prompt(&mut self, kind: PromptKind, label: &str) {
        if self.minibuffer.is_active() {
            return;
        }
        self.minibuffer.start_prompt(kind, label);
        self.reset_minibuffer_input();
    }

    /// Clear the minibuffer input and its edit state, leaving any active
    /// prompt untouched — used both when a prompt starts and to re-ask a
    /// confirmation after an unrecognized answer.
    fn reset_minibuffer_input(&mut self) {
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

    pub(super) fn find_file_prompt(&mut self) {
        let dir = self
            .current_buffer()
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(&self.cwd);
        let initial = format!("{}/", dir.display());
        self.start_minibuffer_prompt_with_input(PromptKind::FindFile, "Find file: ", &initial);
    }

    pub(super) fn switch_buffer_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::SwitchBuffer, "Switch to buffer: ");
    }

    pub(super) fn write_file_prompt(&mut self) {
        let dir = self
            .current_buffer()
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(&self.cwd);
        let initial = format!("{}/", dir.display());
        self.start_minibuffer_prompt_with_input(PromptKind::WriteFile, "Write file: ", &initial);
    }

    pub(super) fn goto_line_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::GotoLine, "Goto line: ");
    }

    pub fn submit_prompt(&mut self) {
        self.dispatch_prompt();
        // Prompt handlers may move point in the focused pane (e.g. goto-line)
        // without going through `execute()`, so scroll it into view here.
        self.ensure_cursor_visible();
    }

    fn dispatch_prompt(&mut self) {
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
                self.write_buffer_reporting(buffer_id, WriteTarget::Path(path));
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
            // Confirmation prompts: an unrecognized answer clears the input
            // and re-asks (the prompt state stays alive); only a recognized
            // answer finishes the prompt.
            PromptKind::KillConfirm { buffer_id } => match input.as_str() {
                "y" | "Y" => {
                    self.minibuffer.finish();
                    self.do_kill_buffer(buffer_id);
                }
                "n" | "N" => self.minibuffer.finish(),
                _ => self.reset_minibuffer_input(),
            },
            PromptKind::OverwriteConfirm { buffer_id, path } => match input.as_str() {
                "y" | "Y" => {
                    self.minibuffer.finish();
                    self.write_buffer_reporting(buffer_id, WriteTarget::Path(path));
                }
                "n" | "N" => {
                    self.minibuffer.finish();
                    self.minibuffer.show_message("Cancelled".to_string());
                }
                _ => self.reset_minibuffer_input(),
            },
            PromptKind::SaveAnywayConfirm { buffer_id } => match input.as_str() {
                "y" | "Y" => {
                    self.minibuffer.finish();
                    self.write_buffer_reporting(buffer_id, WriteTarget::BufferPath);
                }
                "n" | "N" => {
                    self.minibuffer.finish();
                    self.minibuffer.show_message("Save cancelled".to_string());
                }
                _ => self.reset_minibuffer_input(),
            },
            PromptKind::QuitSaveConfirm { buffer_id } => {
                self.minibuffer.finish();
                match input.as_str() {
                    "y" | "Y" => {
                        let buf_state = self
                            .buffers
                            .iter()
                            .find(|b| b.id == buffer_id)
                            .map(|b| (b.path.is_some(), b.name.clone()));
                        match buf_state {
                            Some((true, name)) => {
                                match self.write_buffer(buffer_id, WriteTarget::BufferPath) {
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
                            Some((false, name)) => {
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
                        }
                    }
                    "n" | "N" => {
                        self.quit_pending.retain(|&id| id != buffer_id);
                        self.continue_quit();
                    }
                    "q" | "Q" => {
                        self.quit_pending.clear();
                        self.minibuffer.show_message("Quit".to_string());
                    }
                    "a" | "A" => {
                        // Abort: quit immediately, discard all unsaved
                        // changes, and exit non-zero so callers like git
                        // abandon the operation (vim's :cq).
                        self.quit_pending.clear();
                        self.quit_abort = true;
                        self.should_quit = true;
                    }
                    _ => {
                        // Re-ask for the same buffer.
                        self.continue_quit();
                    }
                }
            }
        }
    }

    pub(super) fn quit(&mut self) {
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
                        &format!("Save buffer {name}? (y/n/q, a aborts) "),
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
