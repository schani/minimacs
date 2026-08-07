use super::*;

impl Editor {
    /// Helper: set minibuffer text directly (for tests).
    fn set_minibuffer_text(&mut self, text: &str) {
        self.minibuffer_buffer.reset_transient_text(text);
        self.minibuffer_pane.set_point(text.chars().count());
    }
}

/// Drive the same query-edit transitions as production isearch input.
fn drive_isearch_query(editor: &mut Editor, query: &str) {
    while editor
        .isearch
        .as_ref()
        .is_some_and(|state| !state.query().is_empty())
    {
        editor.isearch_backspace();
    }
    for ch in query.chars() {
        editor.isearch_input_char(ch);
    }
}

mod core_commands;
mod editing;
mod files_and_buffers;
mod prompts_and_panes;
mod save_flows;
mod search_and_words;
mod unicode_and_multi_pane;
