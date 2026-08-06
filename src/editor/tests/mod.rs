use super::*;

impl Editor {
    /// Helper: set minibuffer text directly (for tests).
    fn set_minibuffer_text(&mut self, text: &str) {
        self.minibuffer_buffer.reset_transient_text(text);
        self.minibuffer_pane.set_point(text.chars().count());
    }
}

mod core_commands;
mod editing;
mod files_and_buffers;
mod prompts_and_panes;
mod save_flows;
mod search_and_words;
mod unicode_and_multi_pane;
