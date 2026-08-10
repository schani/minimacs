use std::path::{Path, PathBuf};

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
        self.set_minibuffer_input("");
    }

    /// Replace the minibuffer input and reset its edit state (history, mark,
    /// scroll), leaving any active prompt untouched.
    fn set_minibuffer_input(&mut self, input: &str) {
        self.minibuffer_buffer.reset_transient_text(input);
        self.minibuffer_pane.set_point(input.chars().count());
        self.minibuffer_pane.set_mark(None);
        self.minibuffer_pane.set_scroll_position(0, 0);
        self.minibuffer_pane.set_preferred_column(None);
    }

    /// Start a minibuffer prompt with initial text. No-op if already active.
    fn start_minibuffer_prompt_with_input(&mut self, kind: PromptKind, label: &str, input: &str) {
        if self.minibuffer.is_active() {
            return;
        }
        self.minibuffer.start_prompt(kind, label);
        self.set_minibuffer_input(input);
    }

    /// Read the current minibuffer text.
    pub fn minibuffer_text(&self) -> String {
        self.minibuffer_buffer.text().to_string()
    }

    /// Turn path-prompt input into the path to act on: normalize it (tilde
    /// expansion, `.`/`..` resolution — leading `..` is preserved), then
    /// resolve a relative result against the editor's `cwd`, so which file
    /// is opened or written never depends on the process working directory.
    /// Empty input stays empty (rejecting it is the caller's concern).
    fn path_from_input(&self, input: &str) -> PathBuf {
        let normalized = normalize_path_string(input);
        if normalized.is_empty() || Path::new(&normalized).is_absolute() {
            return PathBuf::from(normalized);
        }
        PathBuf::from(normalize_path_string(&format!(
            "{}/{normalized}",
            self.cwd.display()
        )))
    }

    /// The shared "non-empty normalized path" validation both path prompts
    /// (find-file, write-file) go through on submit: `None` when the input
    /// is blank or normalizes to the empty path (`.`, `a/..`) — the caller
    /// re-asks via [`Editor::reask_path_prompt`].
    fn validated_path_from_input(&self, input: &str) -> Option<PathBuf> {
        if input.trim().is_empty() {
            return None;
        }
        let path = self.path_from_input(input);
        if path.as_os_str().is_empty() {
            return None;
        }
        Some(path)
    }

    /// The initial input of a path prompt: the current buffer's directory
    /// (or the editor's `cwd`), with one trailing platform path separator.
    fn default_path_prompt_input(&self) -> String {
        let dir = self
            .current_buffer()
            .path()
            .as_ref()
            .and_then(|p| p.parent())
            .unwrap_or(&self.cwd);
        let mut input = dir.display().to_string();
        if !input.ends_with(std::path::MAIN_SEPARATOR) {
            input.push(std::path::MAIN_SEPARATOR);
        }
        input
    }

    /// Re-ask an active path prompt whose input didn't validate: flag the
    /// requirement in the live prompt label (like failing isearch — queued
    /// messages are invisible while a prompt is active) and restore the
    /// default directory prefill.
    fn reask_path_prompt(&mut self, label: &str) {
        self.minibuffer.set_prompt_label(label);
        let input = self.default_path_prompt_input();
        self.set_minibuffer_input(&input);
    }

    pub(super) fn find_file_prompt(&mut self) {
        let initial = self.default_path_prompt_input();
        self.start_minibuffer_prompt_with_input(PromptKind::FindFile, "Find file: ", &initial);
    }

    pub(super) fn switch_buffer_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::SwitchBuffer, "Switch to buffer: ");
    }

    pub(super) fn write_file_prompt(&mut self) {
        let initial = self.default_path_prompt_input();
        self.start_minibuffer_prompt_with_input(PromptKind::WriteFile, "Write file: ", &initial);
    }

    pub(super) fn goto_line_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::GotoLine, "Goto line: ");
    }

    pub(super) fn execute_extended_command_prompt(&mut self) {
        self.start_minibuffer_prompt(PromptKind::ExecuteExtendedCommand, "M-x ");
    }

    pub fn submit_prompt(&mut self) {
        // Enter is intercepted before the keymap, so it never reaches
        // `execute()` and `last_command` would survive the prompt — a C-k
        // in the prompt followed by a C-k in the buffer must not append.
        // C-g cancellation needs no reset here: it runs `Command::Cancel`
        // through `execute()`, which updates `last_command` itself.
        self.clear_last_command();
        self.dispatch_prompt();
        // Prompt handlers may move point in the focused pane (e.g. goto-line)
        // without going through `execute()`, so scroll it into view here.
        self.ensure_cursor_visible();
    }

    fn dispatch_prompt(&mut self) {
        let kind = match self.minibuffer.prompt() {
            Some(p) => p.kind(),
            None => return,
        };
        let input = self.minibuffer_text();

        match kind {
            PromptKind::FindFile => {
                let Some(path) = self.validated_path_from_input(&input) else {
                    self.reask_path_prompt("Find file (path required): ");
                    return;
                };
                self.minibuffer.finish();
                if let Err(e) = self.open_file(&path) {
                    self.minibuffer.show_message(format!("{e}"));
                }
            }
            PromptKind::SwitchBuffer => {
                self.minibuffer.finish();
                self.switch_to_buffer(&input);
            }
            PromptKind::ExecuteExtendedCommand => {
                let Some(command) = crate::command::Command::from_name_or_unique_prefix(&input)
                else {
                    self.minibuffer.dismiss_completions();
                    self.minibuffer.set_prompt_label("M-x (command required) ");
                    return;
                };
                // Finish first so commands that open a prompt can do so,
                // including execute-extended-command itself.
                self.minibuffer.finish();
                self.execute(command);
            }
            PromptKind::WriteFile => {
                let Some(path) = self.validated_path_from_input(&input) else {
                    self.reask_path_prompt("Write file (path required): ");
                    return;
                };
                self.minibuffer.finish();
                let buffer_id = self.current_buffer().id();
                let own_path = self.current_buffer().path() == Some(path.as_path());
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
                // Writing to the buffer's own path is a save — the
                // external-modification guard applies (mtime tracking only
                // covers the buffer's own file).
                if own_path && self.external_modification_guard(buffer_id, false) {
                    return;
                }
                self.write_buffer_reporting(buffer_id, WriteTarget::Path(path));
            }
            PromptKind::GotoLine => {
                self.minibuffer.finish();
                match input.parse::<usize>() {
                    Ok(line_num) if line_num > 0 => {
                        let target_line = line_num - 1;
                        let char_pos = self.current_buffer().line_col_to_char(target_line, 0);
                        self.pane_tree.set_focused_point(char_pos);
                    }
                    _ => self
                        .minibuffer
                        .show_message("Invalid line number".to_string()),
                }
            }
            PromptKind::ISearch => {
                // Enter during isearch accepts the position
                self.isearch_accept();
            }
            // Confirmation prompts: an unrecognized answer visibly clears
            // the input and re-asks. Most retain the prompt object;
            // QuitSaveConfirm finishes and rebuilds it through continue_quit.
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
            PromptKind::SaveAnywayConfirm {
                buffer_id,
                resume_quit,
            } => match input.as_str() {
                "y" | "Y" => {
                    self.minibuffer.finish();
                    if resume_quit {
                        self.quit_save_and_continue(buffer_id);
                    } else {
                        self.write_buffer_reporting(buffer_id, WriteTarget::BufferPath);
                    }
                }
                "n" | "N" => {
                    self.minibuffer.finish();
                    if resume_quit {
                        // Cancel the whole quit, consistent with a failed
                        // quit-time save.
                        self.quit_pending.clear();
                    }
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
                            .find(|b| b.id() == buffer_id)
                            .map(|b| (b.path().is_some(), b.name().to_string()));
                        match buf_state {
                            Some((true, _)) => {
                                // The quit-time save honors the external-
                                // modification guard too; the confirm
                                // handler resumes the quit on "y" and
                                // cancels it on "n".
                                if self.external_modification_guard(buffer_id, true) {
                                    return;
                                }
                                self.quit_save_and_continue(buffer_id);
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
            .filter(|b| b.is_modified())
            .map(|b| b.id())
            .collect();
        self.continue_quit();
    }

    /// Save a quit-pending buffer to its own path and resume the quit
    /// sequence: on success drop it from `quit_pending` and continue with
    /// the next buffer; on failure cancel the whole quit with a message.
    fn quit_save_and_continue(&mut self, buffer_id: usize) {
        let name = match self.buffers.iter().find(|b| b.id() == buffer_id) {
            Some(buf) => buf.name().to_string(),
            None => {
                // Buffer disappeared in the meantime; skip it.
                self.quit_pending.retain(|&id| id != buffer_id);
                self.continue_quit();
                return;
            }
        };
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

    /// Prompt for the next still-modified buffer awaiting a quit-time save
    /// decision, or quit once none remain.
    fn continue_quit(&mut self) {
        while let Some(&id) = self.quit_pending.first() {
            match self.buffers.iter().find(|b| b.id() == id) {
                Some(buf) if buf.is_modified() => {
                    let name = buf.name().to_string();
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
