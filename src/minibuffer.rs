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
    /// "Buffer X modified; kill anyway? (y/n)"
    KillConfirm {
        buffer_id: usize,
    },
    /// "X changed on disk; save anyway? (y/n)" — the external-modification
    /// guard, asked by every flow that writes a buffer to its own path.
    /// `resume_quit` marks a save that is part of the quit sequence: "y"
    /// saves and continues the quit, "n" cancels the whole quit.
    SaveAnywayConfirm {
        buffer_id: usize,
        resume_quit: bool,
    },
    /// "X exists; overwrite? (y/n)" — C-x C-w to an existing file.
    OverwriteConfirm {
        buffer_id: usize,
        path: PathBuf,
    },
    /// "Save buffer X? (y/n/q)" — asked once per modified buffer when quitting.
    QuitSaveConfirm {
        buffer_id: usize,
    },
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

/// Expand a leading `~` or `~/...` to the given home directory.
/// `~user` forms and mid-path tildes are left untouched.
fn expand_tilde_with(input: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return input.to_string();
    };
    if input == "~" || input == "~/" {
        let mut s = home.to_string();
        if input.ends_with('/') && !s.ends_with('/') {
            s.push('/');
        }
        s
    } else if let Some(rest) = input.strip_prefix("~/") {
        format!("{}/{rest}", home.trim_end_matches('/'))
    } else {
        input.to_string()
    }
}

/// Normalize a path string: expand a leading `~`, remove `.` components, and
/// resolve `..` components lexically. On a rootless (relative) path, `..`
/// components that climb above the starting point are preserved (`a/../../b`
/// becomes `../b`) so the caller can resolve them against a base directory;
/// on a rooted path `..` clamps at `/`. Preserves trailing `/` if present.
pub fn normalize_path_string(input: &str) -> String {
    use std::path::Component;

    let home = std::env::var("HOME").ok();
    let input = &expand_tilde_with(input, home.as_deref());

    let has_trailing_slash = input.ends_with('/');
    let path = Path::new(input);
    let mut components: Vec<&std::ffi::OsStr> = Vec::new();
    let mut leading_parents = 0usize;
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
                if !components.is_empty() {
                    components.pop();
                } else if !has_root {
                    // A rootless path climbing above its starting point:
                    // keep the `..` (rooted paths clamp at `/` instead).
                    leading_parents += 1;
                }
            }
            Component::Normal(name) => {
                components.push(name);
            }
            Component::Prefix(_) => {}
        }
    }

    let mut result = if has_root {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for _ in 0..leading_parents {
        result.push("..");
    }
    for c in &components {
        result.push(c);
    }

    let mut s = result.to_string_lossy().into_owned();
    if has_trailing_slash && !s.is_empty() && !s.ends_with('/') {
        s.push('/');
    }
    s
}

/// Tab completion for file paths. Returns the completed path string and display candidates.
///
/// The first element is the completed prefix (same as `complete_path`).
/// The second element is a list of display candidates (basenames, with trailing `/` for dirs).
/// Empty if there is a unique match or no match.
///
/// Relative input (including a leading `..`) is looked up on disk against
/// `base` — the editor's cwd, the same base prompt submission resolves
/// against — but the returned strings stay in the form the user typed
/// (relative stays relative).
pub fn complete_path_with_candidates(input: &str, base: &Path) -> (String, Vec<String>) {
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

    let resolve = |p: &Path| -> PathBuf {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base.join(p)
        }
    };

    let (dir, prefix) = if resolve(&path).is_dir() {
        (path, String::new())
    } else {
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let prefix = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (dir, prefix)
    };

    let Ok(entries) = std::fs::read_dir(resolve(&dir)) else {
        return (input.to_string(), Vec::new());
    };

    // (display path, is_dir) — display paths stay relative for relative
    // input; is_dir is checked on the resolved path.
    let mut matches: Vec<(PathBuf, bool)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| (dir.join(e.file_name()), e.path().is_dir()))
        .collect();
    matches.sort();

    if matches.len() == 1 {
        let (p, is_dir) = &matches[0];
        let mut completed = p.to_string_lossy().into_owned();
        if *is_dir {
            completed.push('/');
        }
        (completed, Vec::new())
    } else if matches.len() > 1 {
        // Full paths for prefix computation
        let full_names: Vec<String> = matches
            .iter()
            .map(|(p, _)| p.to_string_lossy().into_owned())
            .collect();
        let completed = common_prefix(&full_names).unwrap_or_else(|| input.to_string());

        // Basenames for display
        let candidates: Vec<String> = matches
            .iter()
            .map(|(p, is_dir)| {
                let mut name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if *is_dir {
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
    complete_path_with_candidates(input, Path::new(".")).0
}

/// Tab completion for buffer names. Returns the completed name string and sorted display candidates.
///
/// The first element is the completed prefix (same as `complete_buffer`).
/// The second element is a sorted list of matching buffer names. Empty if unique/no match.
pub fn complete_buffer_with_candidates(
    input: &str,
    buffer_names: &[String],
) -> (String, Vec<String>) {
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
/// Longest common prefix of all strings, regardless of their order.
fn common_prefix(strings: &[String]) -> Option<String> {
    let first = strings.first()?;
    let mut prefix_chars = first.chars().count();
    for s in &strings[1..] {
        let shared = first
            .chars()
            .zip(s.chars())
            .take_while(|(a, b)| a == b)
            .count();
        prefix_chars = prefix_chars.min(shared);
    }
    if prefix_chars > 0 {
        Some(first.chars().take(prefix_chars).collect())
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

    /// End a prompt. Clears any message queued while the prompt was active
    /// (it was never rendered and would reappear stale); handlers that want
    /// a result message show it after calling `finish`.
    pub fn finish(&mut self) {
        self.state = MinibufferState::Idle;
        self.message = None;
        self.completions = None;
        self.completion_page = 0;
    }

    /// Dismiss the completion list and reset paging after input changes.
    pub fn dismiss_completions(&mut self) {
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
        assert_eq!(common_prefix(&["abc".into(), "xyz".into()]), None);
    }

    #[test]
    fn buffer_name_completion() {
        let result = complete_buffer(
            "t",
            &["test.txt".into(), "todo.md".into(), "other.rs".into()],
        );
        assert_eq!(result, "t"); // "test.txt" and "todo.md" share only "t"
    }

    #[test]
    fn buffer_name_single_completion() {
        let result = complete_buffer(
            "te",
            &["test.txt".into(), "todo.md".into(), "other.rs".into()],
        );
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
        assert!(
            result.ends_with('/'),
            "Dir completion should end with /: {}",
            result
        );
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
    fn finish_clears_stale_message() {
        // A message queued while a prompt is active (it isn't rendered then)
        // must not reappear after the prompt finishes.
        let mut mb = Minibuffer::new();
        mb.start_prompt(PromptKind::FindFile, "Find file: ");
        mb.show_message("Mark set".to_string());
        mb.finish();
        assert_eq!(mb.message, None);
    }

    #[test]
    fn start_prompt_clears_message() {
        let mut mb = Minibuffer::new();
        mb.show_message("old".to_string());
        mb.start_prompt(PromptKind::FindFile, "Find file: ");
        assert_eq!(mb.message, None);
    }

    #[test]
    fn minibuffer_show_message() {
        let mut mb = Minibuffer::new();
        mb.show_message("hello".to_string());
        assert_eq!(mb.message, Some("hello".to_string()));
    }

    #[test]
    fn common_prefix_unsorted_input() {
        // Previously only the first and last-compared strings determined the
        // result; "a" in the middle must shorten the prefix to "a".
        assert_eq!(
            common_prefix(&["abc".into(), "a".into(), "abd".into()]),
            Some("a".to_string())
        );
    }

    #[test]
    fn common_prefix_multibyte_divergence() {
        // "é" and "ü" share a leading UTF-8 byte; the prefix must be cut at
        // a char boundary, not a byte boundary.
        assert_eq!(common_prefix(&["éa".into(), "üb".into()]), None);
        assert_eq!(
            common_prefix(&["éa".into(), "éb".into()]),
            Some("é".to_string())
        );
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
        let (completed, candidates) = complete_path_with_candidates(&input, Path::new("."));
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
        let (completed, candidates) = complete_path_with_candidates(&input, Path::new("."));
        assert!(completed.ends_with("unique.txt"));
        assert!(candidates.is_empty());
    }

    #[test]
    fn path_candidates_no_match() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();

        let input = format!("{}/zzz", dir.path().display());
        let (completed, candidates) = complete_path_with_candidates(&input, Path::new("."));
        assert_eq!(completed, input);
        assert!(candidates.is_empty());
    }

    #[test]
    fn path_candidates_show_basenames_with_dir_slash() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        std::fs::write(dir.path().join("subfile.txt"), "").unwrap();

        let input = format!("{}/sub", dir.path().display());
        let (_, candidates) = complete_path_with_candidates(&input, Path::new("."));
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
    fn tilde_expands_to_home() {
        assert_eq!(
            expand_tilde_with("~/foo/bar.txt", Some("/home/u")),
            "/home/u/foo/bar.txt"
        );
        assert_eq!(expand_tilde_with("~", Some("/home/u")), "/home/u");
        assert_eq!(expand_tilde_with("~/", Some("/home/u/")), "/home/u/");
    }

    #[test]
    fn tilde_not_expanded_mid_path_or_for_named_user() {
        assert_eq!(expand_tilde_with("/a/~/b", Some("/home/u")), "/a/~/b");
        assert_eq!(
            expand_tilde_with("~other/foo", Some("/home/u")),
            "~other/foo"
        );
        assert_eq!(expand_tilde_with("~/foo", None), "~/foo");
    }

    #[test]
    fn normalize_expands_tilde_and_resolves_dotdot() {
        let Some(home) = std::env::var_os("HOME") else {
            return; // nothing to test without a home dir
        };
        let home = home.to_string_lossy().into_owned();
        let home = home.trim_end_matches('/');
        assert_eq!(normalize_path_string("~/a/../b"), format!("{home}/b"));
    }

    #[test]
    fn normalize_dotdot_at_root() {
        assert_eq!(normalize_path_string("/../foo"), "/foo");
    }

    #[test]
    fn normalize_preserves_leading_dotdot_on_relative_path() {
        assert_eq!(normalize_path_string("../foo"), "../foo");
        assert_eq!(normalize_path_string("../../a/b"), "../../a/b");
    }

    #[test]
    fn normalize_dotdot_escaping_relative_path_stays() {
        assert_eq!(normalize_path_string("a/../../b"), "../b");
    }

    #[test]
    fn normalize_preserves_trailing_slash_on_leading_dotdot() {
        assert_eq!(normalize_path_string("../"), "../");
    }

    #[test]
    fn normalize_multiple_dots() {
        assert_eq!(normalize_path_string("/a/b/./c/../d"), "/a/b/d");
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
        assert!(
            !result.contains("/./"),
            "Result should not contain /./: {}",
            result
        );
    }

    #[test]
    fn path_completion_leading_dotdot_completes_from_parent_of_base() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();

        // Relative input with a leading `..` is looked up against the base
        // directory's parent, and the completion stays relative.
        let (completed, candidates) = complete_path_with_candidates("../al", &sub);
        assert_eq!(completed, "../alpha.txt");
        assert!(candidates.is_empty());
    }

    #[test]
    fn path_completion_leading_dotdot_lists_parent_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let (completed, candidates) = complete_path_with_candidates("../foo", &sub);
        assert_eq!(completed, "../fooba");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.contains(&"foobar.txt".to_string()));
        assert!(candidates.contains(&"foobaz.txt".to_string()));
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
        assert!(
            !result.contains("/../"),
            "Result should not contain /../: {}",
            result
        );
    }
}
