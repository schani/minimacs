use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;

use crate::command::Command;
use crate::editor::{EditRecord, Editor};
use crate::keymap::{default_keymap, Key, KeymapResult, KeymapState};
use crate::minibuffer::PromptKind;

use super::App;

/// All partially-consumed input state, gathered in one place:
///
/// - `keymap`: the chord-in-progress walk of the keymap trie (`C-x ...`).
/// - `esc_pending`: a bare ESC was seen; the next key gets the ALT modifier.
///
/// `Editor::pending_keys` — the mode-line display of the pending input — is
/// a mirror of this state (it lives on `Editor` because rendering only sees
/// `&Editor`). Every mutation goes through these methods so the mirror stays
/// in sync, and `reset` is the single point that clears everything at once.
pub(super) struct InputState {
    keymap: KeymapState,
    esc_pending: bool,
}

impl InputState {
    pub(super) fn new() -> Self {
        Self {
            keymap: KeymapState::new(default_keymap()),
            esc_pending: false,
        }
    }

    /// The single reset point for all pending input state: cancels any chord
    /// in progress, any pending ESC, and the mode-line indicator.
    pub(super) fn reset(&mut self, editor: &mut Editor) {
        self.keymap.clear();
        self.esc_pending = false;
        editor.pending_keys.clear();
    }

    /// Record a bare ESC: the next key will be treated as Meta-modified.
    fn set_esc_pending(&mut self, editor: &mut Editor) {
        self.esc_pending = true;
        editor.pending_keys = format!("{}ESC ", self.keymap.pending_display());
    }

    /// Consume the pending-ESC flag, returning whether it was set.
    fn take_esc_pending(&mut self) -> bool {
        std::mem::take(&mut self.esc_pending)
    }

    fn pending_display(&self) -> String {
        self.keymap.pending_display()
    }

    /// Feed a key to the chord trie, keeping the mode-line display in sync:
    /// `Pending` shows the accumulated prefix, anything else clears it.
    fn process_key(&mut self, editor: &mut Editor, key: KeyEvent) -> KeymapResult {
        let result = self.keymap.process_key(key);
        editor.pending_keys = match result {
            KeymapResult::Pending => self.keymap.pending_display(),
            _ => String::new(),
        };
        result
    }
}

impl<B: Backend> App<B>
where
    B::Error: Send + Sync + 'static,
{
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        // C-g always cancels: reset all pending input state, then Cancel.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.input.reset(&mut self.editor);
            self.editor.execute(Command::Cancel);
            return;
        }

        // Handle Esc-as-Meta prefix: bare Esc sets a flag so the next key gets ALT
        if key.code == KeyCode::Esc
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            self.input.set_esc_pending(&mut self.editor);
            return;
        }

        // If Esc was pending, add ALT modifier to this key
        let key = if self.input.take_esc_pending() {
            KeyEvent::new(key.code, key.modifiers | KeyModifiers::ALT)
        } else {
            key
        };

        // If isearch is active, route keys to isearch handler
        if self.editor.isearch.is_some() {
            self.handle_isearch_key(key);
            return;
        }

        // If minibuffer is active, intercept Enter/Tab then route through keymap
        if self.editor.minibuffer.is_active() {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    self.editor.submit_prompt();
                    return;
                }
                (KeyModifiers::NONE, KeyCode::Tab) => {
                    self.handle_minibuffer_tab();
                    return;
                }
                _ => {
                    self.editor.minibuffer.completions = None;
                    self.editor.minibuffer.completion_page = 0;
                    // Fall through to normal keymap processing
                }
            }
        }

        let pending_before = self.input.pending_display();
        match self.input.process_key(&mut self.editor, key) {
            KeymapResult::Matched(cmd) => {
                self.editor.execute(cmd);
            }
            KeymapResult::Pending => {}
            KeymapResult::NotFound => {
                if !pending_before.is_empty() {
                    // A dead-end chord (e.g. C-x j) must not self-insert.
                    let chord = format!("{pending_before}{}", Key::from_event(key).display());
                    self.editor
                        .minibuffer
                        .show_message(format!("{chord} is undefined"));
                    return;
                }
                // Self-insert fallback for printable chars
                if let KeyCode::Char(c) = key.code {
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT)
                    {
                        self.editor.execute(Command::InsertChar(c));
                    }
                }
            }
        }
    }

    fn handle_isearch_key(&mut self, key: KeyEvent) {
        use crate::editor::SearchDirection;

        match (key.modifiers, key.code) {
            // C-s during isearch: cycle forward
            (KeyModifiers::CONTROL, KeyCode::Char('s')) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.direction = SearchDirection::Forward;
                }
                self.editor.isearch_next();
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.label = "I-search: ".to_string();
                }
            }
            // C-r during isearch: cycle backward
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.direction = SearchDirection::Backward;
                }
                self.editor.isearch_next();
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.label = "I-search backward: ".to_string();
                }
            }
            // Enter: accept search position
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.editor.isearch_accept();
            }
            // Backspace: delete one grapheme cluster from query
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                self.editor.isearch_backspace();
            }
            // Printable char: add to query and search
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.query.push(c);
                    // Sync minibuffer buffer to query
                    let query = isearch.query.clone();
                    self.editor.minibuffer_buffer.text = ropey::Rope::from_str(&query);
                    self.editor.minibuffer_pane.point = query.chars().count();
                }
                self.editor.isearch_update();
            }
            // Any other key: accept search, then process the key normally
            _ => {
                self.editor.isearch_accept();
                self.handle_key(key);
            }
        }
    }

    fn handle_minibuffer_tab(&mut self) {
        use crate::minibuffer::{complete_path_with_candidates, complete_buffer_with_candidates};

        let had_completions = self.editor.minibuffer.completions.is_some();
        let kind = self.editor.minibuffer.prompt().map(|p| p.kind.clone());
        let input = self.editor.minibuffer_text();
        let (completed, candidates) = match kind {
            Some(PromptKind::FindFile) | Some(PromptKind::WriteFile) => {
                complete_path_with_candidates(&input, &self.editor.cwd)
            }
            Some(PromptKind::SwitchBuffer) => {
                let names = self.editor.buffer_names();
                complete_buffer_with_candidates(&input, &names)
            }
            _ => return,
        };

        // Always update completions list
        self.editor.minibuffer.completions = if candidates.is_empty() {
            None
        } else {
            Some(candidates)
        };

        // Only replace buffer text if the completion advanced the prefix
        if completed != input {
            self.editor.minibuffer_buffer.history.commit();
            let old_len = self.editor.minibuffer_buffer.char_count();
            self.editor
                .apply_edit(0, old_len, &completed, EditRecord::Replace);
            self.editor.minibuffer_buffer.history.commit();
            self.editor.minibuffer_pane.point = completed.chars().count();
            self.editor.minibuffer.completion_page = 0;
        } else if had_completions && self.editor.minibuffer.completions.is_some() {
            // Completions were already showing and prefix didn't change: advance page
            self.editor.minibuffer.completion_page += 1;
        } else {
            self.editor.minibuffer.completion_page = 0;
        }
    }

    pub(super) fn handle_paste(&mut self, text: &str) {
        self.editor.clear_last_command();
        if self.editor.minibuffer.is_active() {
            self.editor.minibuffer.completions = None;
            self.editor.minibuffer.completion_page = 0;
        }
        let text = self.editor.normalized_paste(text);
        // Insert pasted text as a single undo group
        self.editor.active_buffer_mut().history.commit();
        let point = self.editor.active_pane().point;
        self.editor.apply_edit(point, point, &text, EditRecord::Insert);
        self.editor.active_pane_mut().point = point + text.chars().count();
        self.editor.active_buffer_mut().history.commit();
    }
}
