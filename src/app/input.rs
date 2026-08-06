use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;

use crate::command::Command;
use crate::keymap::{default_keymap, Key, KeymapResult, KeymapState};
use crate::minibuffer::PromptKind;

use super::App;

/// All partially-consumed input state, gathered in one place:
///
/// - `keymap`: the chord-in-progress walk of the keymap trie (`C-x ...`).
/// - `esc_pending`: a bare ESC was seen; the next key gets the ALT modifier.
///
/// This state also owns its mode-line display. Rendering receives only a
/// small immutable view, so `Editor` has no pending-input mirror.
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
    /// in progress, any pending ESC, and therefore the derived indicator.
    pub(super) fn reset(&mut self) {
        self.keymap.clear();
        self.esc_pending = false;
    }

    /// Record a bare ESC: the next key will be treated as Meta-modified.
    fn set_esc_pending(&mut self) {
        self.esc_pending = true;
    }

    /// Consume the pending-ESC flag, returning whether it was set.
    fn take_esc_pending(&mut self) -> bool {
        std::mem::take(&mut self.esc_pending)
    }

    fn pending_display(&self) -> String {
        let mut display = self.keymap.pending_display();
        if self.esc_pending {
            display.push_str("ESC ");
        }
        display
    }

    pub(super) fn render_view(&self) -> String {
        self.pending_display()
    }

    /// Feed a key to the chord trie. Its result and pending walk are the only
    /// sources for the derived mode-line display.
    fn process_key(&mut self, key: KeyEvent) -> KeymapResult {
        self.keymap.process_key(key)
    }
}

impl<B: Backend> App<B>
where
    B::Error: Send + Sync + 'static,
{
    pub(super) fn handle_key(&mut self, key: KeyEvent) {
        // C-g always cancels: reset all pending input state, then Cancel.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.input.reset();
            self.editor.execute(Command::Cancel);
            return;
        }

        // Handle Esc-as-Meta prefix: bare Esc sets a flag so the next key gets ALT
        if key.code == KeyCode::Esc
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            self.input.set_esc_pending();
            return;
        }

        // If Esc was pending, add ALT modifier to this key
        let key = if self.input.take_esc_pending() {
            KeyEvent::new(key.code, key.modifiers | KeyModifiers::ALT)
        } else {
            key
        };

        // If isearch is active, route keys to isearch handler
        if self.editor.isearch().is_some() {
            self.handle_isearch_key(key);
            return;
        }

        // If minibuffer is active, intercept Enter/Tab then route through keymap
        if self.editor.minibuffer().is_active() {
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
                    self.editor.dismiss_minibuffer_completions();
                    // Fall through to normal keymap processing
                }
            }
        }

        let pending_before = self.input.pending_display();
        match self.input.process_key(key) {
            KeymapResult::Matched(cmd) => {
                self.editor.execute(cmd);
            }
            KeymapResult::Pending => {}
            KeymapResult::NotFound => {
                if !pending_before.is_empty() {
                    // A dead-end chord (e.g. C-x j) must not self-insert.
                    let chord = format!("{pending_before}{}", Key::from_event(key).display());
                    self.editor
                        .set_pending_display_message(format!("{chord} is undefined"));
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
                self.editor.isearch_cycle(SearchDirection::Forward);
            }
            // C-r during isearch: cycle backward
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                self.editor.isearch_cycle(SearchDirection::Backward);
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
                self.editor.isearch_input_char(c);
            }
            // Any other key: accept search, then process the key normally
            _ => {
                self.editor.isearch_accept();
                self.handle_key(key);
            }
        }
    }

    fn handle_minibuffer_tab(&mut self) {
        use crate::minibuffer::{complete_buffer_with_candidates, complete_path_with_candidates};

        let kind = self.editor.minibuffer().prompt().map(|p| p.kind());
        let input = self.editor.minibuffer_text();
        let (completed, candidates) = match kind {
            Some(PromptKind::FindFile) | Some(PromptKind::WriteFile) => {
                complete_path_with_candidates(&input, self.editor.cwd())
            }
            Some(PromptKind::SwitchBuffer) => {
                let names = self.editor.buffer_names();
                complete_buffer_with_candidates(&input, &names)
            }
            _ => return,
        };

        self.editor
            .apply_minibuffer_completion(&input, completed, candidates);
    }
}
