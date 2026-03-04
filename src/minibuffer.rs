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

/// Normalize a path string by removing `.` components and resolving `..` components.
/// Preserves trailing `/` if present.
pub fn normalize_path_string(input: &str) -> String {
    use std::path::Component;

    let has_trailing_slash = input.ends_with('/');
    let path = Path::new(input);
    let mut components: Vec<&std::ffi::OsStr> = Vec::new();
    let mut has_root = false;

    for component in path.components() {
        match component {
            Component::RootDir => {
                has_root = true;
            }
            Component::CurDir => {
                // Skip `.`
            }
            Component::ParentDir => {
                // Pop last component if possible
                if !components.is_empty() {
                    components.pop();
                }
            }
            Component::Normal(name) => {
                components.push(name);
            }
            Component::Prefix(_) => {}
        }
    }

    let result = if has_root {
        let mut p = PathBuf::from("/");
        for c in &components {
            p.push(c);
        }
        p
    } else {
        let mut p = PathBuf::new();
        for c in &components {
            p.push(c);
        }
        p
    };

    let mut s = result.to_string_lossy().into_owned();
    if has_trailing_slash && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Tab completion for file paths. Returns the completed path string and display candidates.
///
/// The first element is the completed prefix (same as `complete_path`).
/// The second element is a list of display candidates (basenames, with trailing `/` for dirs).
/// Empty if there is a unique match or no match.
pub fn complete_path_with_candidates(input: &str) -> (String, Vec<String>) {
    let normalized = if input.is_empty() {
        input.to_string()
    } else {
        normalize_path_string(input)
    };
    let input = &normalized;

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
        return (input.to_string(), Vec::new());
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
        (completed, Vec::new())
    } else if matches.len() > 1 {
        // Full paths for prefix computation
        let full_names: Vec<String> = matches
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let completed = common_prefix(&full_names).unwrap_or_else(|| input.to_string());

        // Basenames for display
        let candidates: Vec<String> = matches
            .iter()
            .map(|p| {
                let mut name = p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if p.is_dir() {
                    name.push('/');
                }
                name
            })
            .collect();

        (completed, candidates)
    } else {
        (input.to_string(), Vec::new())
    }
}

/// Tab completion for file paths. Returns the completed path string.
#[cfg(test)]
pub fn complete_path(input: &str) -> String {
    complete_path_with_candidates(input).0
}

/// Tab completion for buffer names. Returns the completed name string and sorted display candidates.
///
/// The first element is the completed prefix (same as `complete_buffer`).
/// The second element is a sorted list of matching buffer names. Empty if unique/no match.
pub fn complete_buffer_with_candidates(input: &str, buffer_names: &[String]) -> (String, Vec<String>) {
    let matches: Vec<&String> = buffer_names
        .iter()
        .filter(|name| name.starts_with(input))
        .collect();

    if matches.len() == 1 {
        (matches[0].clone(), Vec::new())
    } else if matches.len() > 1 {
        let names: Vec<String> = matches.into_iter().cloned().collect();
        let completed = common_prefix(&names).unwrap_or_else(|| input.to_string());
        let mut candidates = names;
        candidates.sort();
        (completed, candidates)
    } else {
        (input.to_string(), Vec::new())
    }
}

/// Tab completion for buffer names. Returns the completed name string.
#[cfg(test)]
pub fn complete_buffer(input: &str, buffer_names: &[String]) -> String {
    complete_buffer_with_candidates(input, buffer_names).0
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
    pub completions: Option<Vec<String>>,
    pub completion_page: usize,
}

impl Minibuffer {
    pub fn new() -> Self {
        Self {
            state: MinibufferState::Idle,
            message: None,
            completions: None,
            completion_page: 0,
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
        self.completions = None;
        self.completion_page = 0;
    }

    pub fn cancel(&mut self) {
        self.state = MinibufferState::Idle;
        self.message = Some("Quit".to_string());
        self.completions = None;
        self.completion_page = 0;
    }

    pub fn finish(&mut self) {
        self.state = MinibufferState::Idle;
        self.completions = None;
        self.completion_page = 0;
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

    // === _with_candidates tests ===

    #[test]
    fn path_candidates_multiple_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let input = format!("{}/foo", dir.path().display());
        let (completed, candidates) = complete_path_with_candidates(&input);
        assert!(completed.contains("foob"));
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains(&"foobar.txt".to_string()));
        assert!(candidates.contains(&"foobaz.txt".to_string()));
    }

    #[test]
    fn path_candidates_single_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("unique.txt"), "").unwrap();

        let input = format!("{}/uni", dir.path().display());
        let (completed, candidates) = complete_path_with_candidates(&input);
        assert!(completed.ends_with("unique.txt"));
        assert!(candidates.is_empty());
    }

    #[test]
    fn path_candidates_no_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();

        let input = format!("{}/zzz", dir.path().display());
        let (completed, candidates) = complete_path_with_candidates(&input);
        assert_eq!(completed, input);
        assert!(candidates.is_empty());
    }

    #[test]
    fn path_candidates_show_basenames_with_dir_slash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("subfile.txt"), "").unwrap();

        let input = format!("{}/sub", dir.path().display());
        let (_, candidates) = complete_path_with_candidates(&input);
        assert_eq!(candidates.len(), 2);
        // Dir candidate should have trailing /
        assert!(candidates.iter().any(|c| c == "subdir/"));
        // File candidate should not
        assert!(candidates.iter().any(|c| c == "subfile.txt"));
    }

    #[test]
    fn buffer_candidates_multiple_matches_sorted() {
        let names = vec!["test.txt".into(), "todo.md".into(), "other.rs".into()];
        let (completed, candidates) = complete_buffer_with_candidates("t", &names);
        assert_eq!(completed, "t");
        assert_eq!(candidates, vec!["test.txt", "todo.md"]);
    }

    #[test]
    fn buffer_candidates_single_match() {
        let names = vec!["test.txt".into(), "todo.md".into()];
        let (completed, candidates) = complete_buffer_with_candidates("te", &names);
        assert_eq!(completed, "test.txt");
        assert!(candidates.is_empty());
    }

    #[test]
    fn buffer_candidates_no_match() {
        let names = vec!["test.txt".into(), "todo.md".into()];
        let (completed, candidates) = complete_buffer_with_candidates("zzz", &names);
        assert_eq!(completed, "zzz");
        assert!(candidates.is_empty());
    }

    // === normalize_path_string tests ===

    #[test]
    fn normalize_removes_dot_component() {
        assert_eq!(normalize_path_string("/home/user/./foo"), "/home/user/foo");
    }

    #[test]
    fn normalize_resolves_dotdot_component() {
        assert_eq!(normalize_path_string("/home/user/../other"), "/home/other");
    }

    #[test]
    fn normalize_preserves_trailing_slash() {
        assert_eq!(normalize_path_string("/home/user/./"), "/home/user/");
        assert_eq!(normalize_path_string("/home/user/../"), "/home/");
    }

    #[test]
    fn normalize_no_change_for_clean_path() {
        assert_eq!(normalize_path_string("/home/user/foo"), "/home/user/foo");
    }

    #[test]
    fn normalize_dotdot_at_root() {
        assert_eq!(normalize_path_string("/../foo"), "/foo");
    }

    #[test]
    fn normalize_multiple_dots() {
        assert_eq!(
            normalize_path_string("/a/b/./c/../d"),
            "/a/b/d"
        );
    }

    #[test]
    fn normalize_preserves_trailing_slash_after_dotdot() {
        assert_eq!(normalize_path_string("/a/b/../"), "/a/");
    }

    #[test]
    fn path_completion_normalizes_dot_in_path() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();

        // Input has /./  which should be normalized
        let input = format!("{}/./{}", dir.path().display(), "a");
        let result = complete_path(&input);
        assert!(result.ends_with("alpha.txt"));
        assert!(!result.contains("/./"), "Result should not contain /./: {}", result);
    }

    #[test]
    fn path_completion_normalizes_dotdot_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();

        // Input navigates into sub then back up with ..
        let input = format!("{}/sub/../a", dir.path().display());
        let result = complete_path(&input);
        assert!(result.ends_with("alpha.txt"));
        assert!(!result.contains("/../"), "Result should not contain /../: {}", result);
    }
}
