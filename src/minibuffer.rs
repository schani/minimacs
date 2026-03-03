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
    pub label: String, // e.g., "Find file: "
}

impl Prompt {
    pub fn new(kind: PromptKind, label: &str) -> Self {
        Self {
            kind,
            label: label.to_string(),
        }
    }
}

/// Tab completion for file paths. Returns the completed path string.
pub fn complete_path(input: &str) -> String {
    let path = if input.is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(input)
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
        return input.to_string();
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
        completed
    } else if matches.len() > 1 {
        let names: Vec<String> = matches
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        common_prefix(&names).unwrap_or_else(|| input.to_string())
    } else {
        input.to_string()
    }
}

/// Tab completion for buffer names. Returns the completed name string.
pub fn complete_buffer(input: &str, buffer_names: &[String]) -> String {
    let matches: Vec<&String> = buffer_names
        .iter()
        .filter(|name| name.starts_with(input))
        .collect();

    if matches.len() == 1 {
        matches[0].clone()
    } else if matches.len() > 1 {
        let names: Vec<String> = matches.into_iter().cloned().collect();
        common_prefix(&names).unwrap_or_else(|| input.to_string())
    } else {
        input.to_string()
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = complete_buffer("t", &["test.txt".into(), "todo.md".into(), "other.rs".into()]);
        assert_eq!(result, "t"); // "test.txt" and "todo.md" share only "t"
    }

    #[test]
    fn buffer_name_single_completion() {
        let result = complete_buffer("te", &["test.txt".into(), "todo.md".into(), "other.rs".into()]);
        assert_eq!(result, "test.txt"); // only "test.txt" matches
    }

    #[test]
    fn path_completion_in_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("beta.txt"), "").unwrap();

        let input = format!("{}/a", dir.path().display());
        let result = complete_path(&input);
        assert!(result.ends_with("alpha.txt"));
    }

    #[test]
    fn path_completion_multiple_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let input = format!("{}/foo", dir.path().display());
        let result = complete_path(&input);
        assert!(result.contains("foob"));
    }

    #[test]
    fn path_completion_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();

        let input = format!("{}/sub", dir.path().display());
        let result = complete_path(&input);
        assert!(result.ends_with('/'), "Dir completion should end with /: {}", result);
    }

    #[test]
    fn minibuffer_prompt_idle() {
        let mb = Minibuffer::new();
        assert!(mb.prompt().is_none());
    }

    #[test]
    fn minibuffer_prompt_mut_idle() {
        let mut mb = Minibuffer::new();
        assert!(mb.prompt_mut().is_none());
    }

    #[test]
    fn minibuffer_finish() {
        let mut mb = Minibuffer::new();
        mb.start_prompt(PromptKind::FindFile, "Find file: ");
        assert!(mb.is_active());
        mb.finish();
        assert!(!mb.is_active());
    }

    #[test]
    fn minibuffer_show_message() {
        let mut mb = Minibuffer::new();
        mb.show_message("hello".to_string());
        assert_eq!(mb.message, Some("hello".to_string()));
    }

    #[test]
    fn common_prefix_empty_list() {
        assert_eq!(common_prefix(&[]), None);
    }
}
