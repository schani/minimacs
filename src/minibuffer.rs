use std::path::{Path, PathBuf};

/// The state of the minibuffer.
#[derive(Debug)]
pub enum MinibufferState {
    /// Idle — shows timed messages.
    Idle,
    /// Active prompt waiting for user input.
    Prompt(Prompt),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptKind {
    FindFile,
    SwitchBuffer,
    WriteFile,
    GotoLine,
    ISearch,
    /// "Save buffer X? (y/n/q)"
    SaveConfirm { buffer_name: String },
}

#[derive(Debug)]
pub struct Prompt {
    pub kind: PromptKind,
    pub label: String,    // e.g., "Find file: "
    pub input: String,    // current user input
    pub cursor: usize,    // cursor position within input
}

impl Prompt {
    pub fn new(kind: PromptKind, label: &str) -> Self {
        Self {
            kind,
            label: label.to_string(),
            input: String::new(),
            cursor: 0,
        }
    }

    pub fn new_with_input(kind: PromptKind, label: &str, input: &str) -> Self {
        let cursor = input.len();
        Self {
            kind,
            label: label.to_string(),
            input: input.to_string(),
            cursor,
        }
    }

    /// Display text: label + input
    pub fn display(&self) -> String {
        format!("{}{}", self.label, self.input)
    }

    /// Insert a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Delete the character before the cursor.
    pub fn delete_backward(&mut self) {
        if self.cursor > 0 {
            // Find the previous char boundary
            let prev = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev);
            self.cursor = prev;
        }
    }

    /// Move cursor forward.
    pub fn forward_char(&mut self) {
        if self.cursor < self.input.len() {
            self.cursor += self.input[self.cursor..].chars().next().map_or(0, |c| c.len_utf8());
        }
    }

    /// Move cursor backward.
    pub fn backward_char(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.input[..self.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    /// Move cursor to beginning.
    pub fn beginning(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end.
    pub fn end(&mut self) {
        self.cursor = self.input.len();
    }

    /// Tab completion for file paths.
    pub fn complete_path(&mut self) {
        let path = if self.input.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(&self.input)
        };

        let (dir, prefix) = if path.is_dir() {
            (path, String::new())
        } else {
            let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            let prefix = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (dir, prefix)
        };

        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };

        let mut matches: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&prefix)
            })
            .map(|e| e.path())
            .collect();
        matches.sort();

        if matches.len() == 1 {
            let mut completed = matches[0].to_string_lossy().into_owned();
            if matches[0].is_dir() {
                completed.push('/');
            }
            self.input = completed;
            self.cursor = self.input.len();
        } else if matches.len() > 1 {
            // Find common prefix
            let names: Vec<String> = matches
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
            if let Some(common) = common_prefix(&names) {
                self.input = common;
                self.cursor = self.input.len();
            }
        }
    }

    /// Tab completion for buffer names.
    pub fn complete_buffer(&mut self, buffer_names: &[String]) {
        let prefix = &self.input;
        let matches: Vec<&String> = buffer_names
            .iter()
            .filter(|name| name.starts_with(prefix.as_str()))
            .collect();

        if matches.len() == 1 {
            self.input = matches[0].clone();
            self.cursor = self.input.len();
        } else if matches.len() > 1 {
            let names: Vec<String> = matches.into_iter().cloned().collect();
            if let Some(common) = common_prefix(&names) {
                self.input = common;
                self.cursor = self.input.len();
            }
        }
    }
}

/// Find the common prefix of a list of strings.
fn common_prefix(strings: &[String]) -> Option<String> {
    if strings.is_empty() {
        return None;
    }
    let first = &strings[0];
    let mut len = first.len();
    for s in &strings[1..] {
        len = first
            .chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .count();
        // Convert char count back to byte length
        len = first.char_indices().nth(len).map_or(first.len(), |(i, _)| i);
    }
    if len > 0 {
        Some(first[..len].to_string())
    } else {
        None
    }
}

pub struct Minibuffer {
    pub state: MinibufferState,
    pub message: Option<String>,
}

impl Minibuffer {
    pub fn new() -> Self {
        Self {
            state: MinibufferState::Idle,
            message: None,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, MinibufferState::Prompt(_))
    }

    pub fn prompt(&self) -> Option<&Prompt> {
        match &self.state {
            MinibufferState::Prompt(p) => Some(p),
            _ => None,
        }
    }

    pub fn prompt_mut(&mut self) -> Option<&mut Prompt> {
        match &mut self.state {
            MinibufferState::Prompt(p) => Some(p),
            _ => None,
        }
    }

    pub fn start_prompt(&mut self, kind: PromptKind, label: &str) {
        self.state = MinibufferState::Prompt(Prompt::new(kind, label));
        self.message = None;
    }

    pub fn start_prompt_with_input(&mut self, kind: PromptKind, label: &str, input: &str) {
        self.state = MinibufferState::Prompt(Prompt::new_with_input(kind, label, input));
        self.message = None;
    }

    pub fn cancel(&mut self) {
        self.state = MinibufferState::Idle;
        self.message = Some("Quit".to_string());
    }

    pub fn finish(&mut self) {
        self.state = MinibufferState::Idle;
    }

    pub fn show_message(&mut self, msg: String) {
        self.message = Some(msg);
    }

    /// Get the display text for the minibuffer line.
    pub fn display_text(&self) -> String {
        match &self.state {
            MinibufferState::Prompt(p) => p.display(),
            MinibufferState::Idle => self.message.as_deref().unwrap_or("").to_string(),
        }
    }

    /// Get cursor position within the minibuffer line (for the prompt cursor).
    pub fn cursor_position(&self) -> Option<usize> {
        match &self.state {
            MinibufferState::Prompt(p) => Some(p.label.len() + p.cursor),
            MinibufferState::Idle => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_insert_and_delete() {
        let mut prompt = Prompt::new(PromptKind::FindFile, "Find file: ");
        prompt.insert_char('h');
        prompt.insert_char('i');
        assert_eq!(prompt.input, "hi");
        assert_eq!(prompt.cursor, 2);
        prompt.delete_backward();
        assert_eq!(prompt.input, "h");
        assert_eq!(prompt.cursor, 1);
    }

    #[test]
    fn prompt_navigation() {
        let mut prompt = Prompt::new(PromptKind::FindFile, "Find file: ");
        prompt.insert_char('a');
        prompt.insert_char('b');
        prompt.insert_char('c');
        prompt.backward_char();
        assert_eq!(prompt.cursor, 2);
        prompt.beginning();
        assert_eq!(prompt.cursor, 0);
        prompt.end();
        assert_eq!(prompt.cursor, 3);
    }

    #[test]
    fn prompt_display() {
        let prompt = Prompt::new_with_input(PromptKind::FindFile, "Find file: ", "test.txt");
        assert_eq!(prompt.display(), "Find file: test.txt");
    }

    #[test]
    fn minibuffer_state_transitions() {
        let mut mb = Minibuffer::new();
        assert!(!mb.is_active());

        mb.start_prompt(PromptKind::FindFile, "Find file: ");
        assert!(mb.is_active());

        mb.cancel();
        assert!(!mb.is_active());
        assert_eq!(mb.message, Some("Quit".to_string()));
    }

    #[test]
    fn common_prefix_works() {
        assert_eq!(
            common_prefix(&["foo".into(), "foobar".into(), "foobaz".into()]),
            Some("foo".into())
        );
        assert_eq!(
            common_prefix(&["abc".into(), "xyz".into()]),
            None
        );
    }

    #[test]
    fn buffer_name_completion() {
        let mut prompt = Prompt::new(PromptKind::SwitchBuffer, "Switch to buffer: ");
        prompt.insert_char('t');
        prompt.complete_buffer(&["test.txt".into(), "todo.md".into(), "other.rs".into()]);
        assert_eq!(prompt.input, "t"); // "test.txt" and "todo.md" share only "t"
    }

    #[test]
    fn buffer_name_single_completion() {
        let mut prompt = Prompt::new(PromptKind::SwitchBuffer, "Switch to buffer: ");
        prompt.insert_char('t');
        prompt.insert_char('e');
        prompt.complete_buffer(&["test.txt".into(), "todo.md".into(), "other.rs".into()]);
        assert_eq!(prompt.input, "test.txt"); // only "test.txt" matches
    }

    #[test]
    fn path_completion_in_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("beta.txt"), "").unwrap();

        let mut prompt = Prompt::new(PromptKind::FindFile, "Find file: ");
        prompt.input = format!("{}/a", dir.path().display());
        prompt.cursor = prompt.input.len();
        prompt.complete_path();
        assert!(prompt.input.ends_with("alpha.txt"));
    }
}
