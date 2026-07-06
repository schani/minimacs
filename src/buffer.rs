use anyhow::{bail, Result};
use ropey::{Rope, RopeSlice};
use std::fs;
use std::path::{Path, PathBuf};

use crate::history::History;
use crate::syntax::{self, SyntaxState};

pub type BufferId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(&self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

/// True for chars ropey treats as line breaks (with its default
/// `unicode_lines` feature): LF, VT, FF, CR, NEL, LS, PS.
fn is_line_break_char(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\u{0b}' | '\u{0c}' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

/// Char length of the line break terminating a ropey line slice: 0 (the
/// final line, or an empty buffer), 2 for CRLF, 1 for every other break
/// in ropey's set. Every consumer that strips a line's break — line
/// lengths, rendering, mouse mapping, kill-line — must use this so it
/// agrees with where ropey actually breaks lines.
pub(crate) fn line_break_len_chars(line: RopeSlice) -> usize {
    let len = line.len_chars();
    if len == 0 {
        return 0;
    }
    let last = line.char(len - 1);
    if !is_line_break_char(last) {
        return 0;
    }
    if last == '\n' && len >= 2 && line.char(len - 2) == '\r' {
        2
    } else {
        1
    }
}

pub struct Buffer {
    pub id: BufferId,
    pub text: Rope,
    pub path: Option<PathBuf>,
    pub name: String,
    pub modified: bool,
    pub line_ending: LineEnding,
    pub history: History,
    pub syntax: Option<SyntaxState>,
    pub edit_generation: usize,
    /// Modification time of the file when we last loaded or saved it.
    /// Used to detect external changes before clobbering them on save.
    disk_mtime: Option<std::time::SystemTime>,
}

impl Buffer {
    pub fn new_scratch(id: BufferId) -> Self {
        Self {
            id,
            text: Rope::new(),
            path: None,
            name: "*scratch*".to_string(),
            modified: false,
            line_ending: LineEnding::Lf,
            history: History::new(),
            syntax: None,
            edit_generation: 0,
            disk_mtime: None,
        }
    }

    /// Create a new empty buffer with a file path (for files that don't exist yet).
    pub fn new_for_path(id: BufferId, path: &Path) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let syntax_state = syntax::detect_language(path).and_then(SyntaxState::new);
        Self {
            id,
            text: Rope::new(),
            path: Some(path.to_path_buf()),
            name,
            modified: false,
            line_ending: LineEnding::Lf,
            history: History::new(),
            syntax: syntax_state,
            edit_generation: 0,
            disk_mtime: None,
        }
    }

    pub fn from_str(id: BufferId, name: &str, content: &str) -> Self {
        Self {
            id,
            text: Rope::from_str(content),
            path: None,
            name: name.to_string(),
            modified: false,
            line_ending: LineEnding::Lf,
            history: History::new(),
            syntax: None,
            edit_generation: 0,
            disk_mtime: None,
        }
    }

    pub fn from_file(id: BufferId, path: &Path) -> Result<Self> {
        let bytes = fs::read(path)?;

        // Detect binary files (contains null bytes)
        if bytes.contains(&0) {
            bail!("File appears to be binary: {}", path.display());
        }

        // Check for non-UTF-8
        let content = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(_) => bail!("File is not valid UTF-8: {}", path.display()),
        };

        // Detect line ending
        let line_ending = if content.contains("\r\n") {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        // Detect language for syntax highlighting
        let syntax_state = syntax::detect_language(path)
            .and_then(SyntaxState::new);

        Ok(Self {
            id,
            text: Rope::from_str(content),
            path: Some(path.to_path_buf()),
            name,
            modified: false,
            line_ending,
            history: History::new(),
            syntax: syntax_state,
            edit_generation: 0,
            disk_mtime: fs::metadata(path).and_then(|m| m.modified()).ok(),
        })
    }

    /// Save atomically: write to a temp file in the same directory, fsync,
    /// then rename over the target. A crash or full disk mid-write can never
    /// destroy the existing file.
    pub fn save(&mut self) -> Result<()> {
        let path = match &self.path {
            Some(p) => p.clone(),
            None => bail!("Buffer has no file path"),
        };
        self.save_as(&path)
    }

    /// Save to `path` (atomically, like [`save`]). The buffer's path and
    /// clean state are only updated after the write succeeds, so a failed
    /// save never changes the buffer's identity.
    ///
    /// `path` is the buffer's *logical* identity; the bytes land on the
    /// *physical* file behind any symlinks (see [`resolve_write_target`]),
    /// so saving through a symlink rewrites the target instead of replacing
    /// the link, without renaming the buffer to the resolved path.
    pub fn save_as(&mut self, path: &Path) -> Result<()> {
        use std::io::Write;

        let content: String = self.text.to_string();
        let physical = resolve_write_target(path)?;

        if has_other_hard_links(&physical) {
            // The target has other hard links; the rename below would
            // replace the inode and make the other names diverge. Write
            // in place to keep the inode, trading away crash-atomicity
            // for this case (a crash mid-write can leave the file
            // truncated) — preserving the hard links is the point, the
            // same tradeoff emacs makes with backup-by-copying.
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&physical)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        } else {
            let dir = match physical.parent() {
                Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                _ => PathBuf::from("."),
            };
            let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
            tmp.write_all(content.as_bytes())?;
            // Keep the target's permissions; a fresh temp file defaults to 0600.
            if let Ok(meta) = fs::metadata(&physical) {
                tmp.as_file().set_permissions(meta.permissions())?;
            }
            tmp.as_file().sync_all()?;
            tmp.persist(&physical)?;
        }

        self.path = Some(path.to_path_buf());
        // Stat the file that was actually written. (Statting the logical
        // path would also work — fs::metadata follows symlinks — but be
        // explicit.)
        self.disk_mtime = fs::metadata(&physical).and_then(|m| m.modified()).ok();
        self.history.mark_clean();
        self.modified = false;
        Ok(())
    }

    /// Re-detect the syntax highlighting language from the current path
    /// (after `C-x C-w` to a different extension).
    pub fn redetect_syntax(&mut self) {
        self.syntax = self
            .path
            .as_deref()
            .and_then(syntax::detect_language)
            .and_then(SyntaxState::new);
    }

    /// True if the file on disk has changed since we last loaded or saved it
    /// (i.e. saving now would clobber someone else's changes).
    pub fn externally_modified(&self) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let current = fs::metadata(path).and_then(|m| m.modified()).ok();
        match (self.disk_mtime, current) {
            (Some(known), Some(now)) => known != now,
            // We never saw the file on disk, and it still isn't there.
            (None, None) => false,
            // Created or deleted behind our back.
            _ => true,
        }
    }

    /// Total number of lines in the buffer.
    pub fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    /// Total number of chars in the buffer.
    pub fn char_count(&self) -> usize {
        self.text.len_chars()
    }

    /// Convert a char offset to (line, col) where both are 0-based.
    pub fn char_to_line_col(&self, char_idx: usize) -> (usize, usize) {
        let char_idx = char_idx.min(self.text.len_chars());
        let line = self.text.char_to_line(char_idx);
        let line_start = self.text.line_to_char(line);
        (line, char_idx - line_start)
    }

    /// Convert (line, col) to char offset, clamping col to line length.
    pub fn line_col_to_char(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.line_count().saturating_sub(1));
        let line_start = self.text.line_to_char(line);
        let line_len = self.line_len_chars(line);
        line_start + col.min(line_len)
    }

    /// Snap a char position to the nearest grapheme-cluster boundary at or
    /// before it (emacs behavior). Identity for positions already on a
    /// boundary; positions inside a line break (mid-CRLF) snap back to the
    /// end of the line's text. Positions computed by column arithmetic
    /// (line movement's column clamping, mouse-click mapping) must be
    /// snapped so point never rests mid-cluster, where a backspace or
    /// insert would split the cluster.
    pub fn snap_to_grapheme_boundary(&self, pos: usize) -> usize {
        use unicode_segmentation::UnicodeSegmentation;
        let pos = pos.min(self.char_count());
        let (line, col) = self.char_to_line_col(pos);
        let line_len = self.line_len_chars(line);
        let line_start = self.line_col_to_char(line, 0);
        if col >= line_len {
            // At the line's end (a boundary) or inside its line break
            // (mid-CRLF): both resolve to the end of the line's text.
            return line_start + line_len;
        }
        let line_text: String = self
            .text
            .slice(line_start..line_start + line_len)
            .chars()
            .collect();
        let mut start = 0;
        for g in line_text.graphemes(true) {
            let g_len = g.chars().count();
            if col < start + g_len {
                return line_start + start;
            }
            start += g_len;
        }
        line_start + line_len
    }

    /// Length of line in chars, excluding its terminating line break.
    pub fn line_len_chars(&self, line_idx: usize) -> usize {
        let line = self.text.line(line_idx);
        line.len_chars() - line_break_len_chars(line)
    }

    /// Insert a string at the given char offset.
    pub fn insert(&mut self, char_idx: usize, text: &str) {
        self.text.insert(char_idx, text);
        self.modified = true;
        self.edit_generation += 1;
    }

    /// Remove chars in range [start..end).
    pub fn remove(&mut self, start: usize, end: usize) {
        if start < end {
            self.text.remove(start..end);
            self.modified = true;
            self.edit_generation += 1;
        }
    }

    /// Update the modified flag based on undo history clean state.
    pub fn update_modified(&mut self) {
        self.modified = !self.history.is_clean();
    }
}

/// Resolve the physical file a write to `path` should land on, following
/// any chain of symlinks — including dangling ones. Saving through a
/// symlink must rewrite the link's target, not replace the link with a
/// regular file (emacs behavior); for a dangling link `foo -> missing`
/// the result is `missing`, which the write then creates.
fn resolve_write_target(path: &Path) -> Result<PathBuf> {
    // Easy case: the file exists — canonicalize resolves the whole
    // symlink chain (and `..` / directory symlinks along the way).
    if let Ok(resolved) = fs::canonicalize(path) {
        return Ok(resolved);
    }
    // The file doesn't exist yet, or the path is a dangling symlink:
    // follow links manually. A relative link target is relative to the
    // link's directory (`join` on an absolute target replaces the base).
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match fs::read_link(&current) {
            Ok(target) => {
                current = match current.parent() {
                    Some(dir) if !dir.as_os_str().is_empty() => dir.join(&target),
                    _ => target,
                };
            }
            Err(_) => {
                // Not a symlink: this is the write target. Canonicalize
                // its parent (the file itself doesn't exist) so `..` and
                // directory symlinks resolve; if even the parent doesn't
                // exist, return as-is and let the write report the error.
                if let (Some(parent), Some(name)) = (current.parent(), current.file_name()) {
                    let parent = if parent.as_os_str().is_empty() {
                        Path::new(".")
                    } else {
                        parent
                    };
                    if let Ok(parent) = fs::canonicalize(parent) {
                        return Ok(parent.join(name));
                    }
                }
                return Ok(current);
            }
        }
    }
    bail!("too many levels of symbolic links: {}", path.display());
}

/// True if `path` names a file with other hard links (nlink > 1); the
/// atomic rename-based save would split those apart.
#[cfg(unix)]
fn has_other_hard_links(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).is_ok_and(|m| m.nlink() > 1)
}

/// Non-unix platforms have no visible link count; always use the atomic
/// rename-based save there.
#[cfg(not(unix))]
fn has_other_hard_links(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_scratch_buffer() {
        let buf = Buffer::new_scratch(0);
        assert_eq!(buf.name, "*scratch*");
        assert_eq!(buf.char_count(), 0);
        assert!(!buf.modified);
    }

    #[test]
    fn from_str_basic() {
        let buf = Buffer::from_str(0, "test", "hello\nworld");
        assert_eq!(buf.char_count(), 11);
        assert_eq!(buf.line_count(), 2);
    }

    #[test]
    fn char_to_line_col_basic() {
        let buf = Buffer::from_str(0, "test", "hello\nworld");
        assert_eq!(buf.char_to_line_col(0), (0, 0));
        assert_eq!(buf.char_to_line_col(5), (0, 5));
        assert_eq!(buf.char_to_line_col(6), (1, 0));
        assert_eq!(buf.char_to_line_col(11), (1, 5));
    }

    #[test]
    fn line_col_to_char_basic() {
        let buf = Buffer::from_str(0, "test", "hello\nworld");
        assert_eq!(buf.line_col_to_char(0, 0), 0);
        assert_eq!(buf.line_col_to_char(1, 0), 6);
        assert_eq!(buf.line_col_to_char(1, 5), 11);
    }

    #[test]
    fn line_col_to_char_clamps_col() {
        let buf = Buffer::from_str(0, "test", "hi\nworld");
        // Line 0 is "hi" (2 chars), col 10 should clamp to 2
        assert_eq!(buf.line_col_to_char(0, 10), 2);
    }

    #[test]
    fn line_len_chars_excludes_newline() {
        let buf = Buffer::from_str(0, "test", "hello\nworld\n");
        assert_eq!(buf.line_len_chars(0), 5); // "hello"
        assert_eq!(buf.line_len_chars(1), 5); // "world"
        assert_eq!(buf.line_len_chars(2), 0); // empty last line
    }

    /// Every line break ropey recognizes (with its default `unicode_lines`
    /// feature): LF, CRLF, lone CR, VT, FF, NEL, LS, PS.
    const ROPEY_LINE_BREAKS: [&str; 8] = [
        "\n", "\r\n", "\r", "\u{0b}", "\u{0c}", "\u{85}", "\u{2028}", "\u{2029}",
    ];

    #[test]
    fn line_len_chars_excludes_every_ropey_line_break() {
        for br in ROPEY_LINE_BREAKS {
            let buf = Buffer::from_str(0, "test", &format!("ab{br}cd{br}"));
            assert_eq!(buf.line_count(), 3, "ropey must break on {br:?}");
            assert_eq!(buf.line_len_chars(0), 2, "line 0 with break {br:?}");
            assert_eq!(buf.line_len_chars(1), 2, "line 1 with break {br:?}");
            assert_eq!(buf.line_len_chars(2), 0, "line 2 with break {br:?}");
        }
    }

    #[test]
    fn line_break_len_chars_matches_ropey_breaks() {
        for br in ROPEY_LINE_BREAKS {
            let buf = Buffer::from_str(0, "test", &format!("ab{br}cd"));
            let expected = br.chars().count();
            assert_eq!(
                line_break_len_chars(buf.text.line(0)),
                expected,
                "break {br:?}"
            );
            assert_eq!(line_break_len_chars(buf.text.line(1)), 0, "final line");
        }
        let empty = Buffer::from_str(0, "test", "");
        assert_eq!(line_break_len_chars(empty.text.line(0)), 0);
    }

    #[test]
    fn line_col_to_char_clamps_before_line_break() {
        // Form feed is a line break: col clamps to 1, before the FF.
        let buf = Buffer::from_str(0, "test", "a\u{0c}b\n");
        assert_eq!(buf.line_col_to_char(0, 10), 1);
    }

    #[test]
    fn snap_to_grapheme_boundary_is_identity_on_boundaries() {
        // "ae\u{301}b": clusters are a(0..1), e+acute(1..3), b(3..4).
        let buf = Buffer::from_str(0, "test", "ae\u{301}b\ncd");
        for pos in [0, 1, 3, 4, 5, 6, 7] {
            assert_eq!(buf.snap_to_grapheme_boundary(pos), pos, "pos {pos}");
        }
    }

    #[test]
    fn snap_to_grapheme_boundary_snaps_mid_cluster_to_start() {
        let buf = Buffer::from_str(0, "test", "ae\u{301}b");
        assert_eq!(buf.snap_to_grapheme_boundary(2), 1);
        // Family emoji: man ZWJ woman ZWJ girl is one cluster (chars 1..6).
        let buf = Buffer::from_str(0, "test", "x\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}z");
        for pos in 2..6 {
            assert_eq!(buf.snap_to_grapheme_boundary(pos), 1, "pos {pos}");
        }
        assert_eq!(buf.snap_to_grapheme_boundary(6), 6);
    }

    #[test]
    fn snap_to_grapheme_boundary_snaps_mid_crlf_before_break() {
        // Position 2 sits between \r and \n; snap back to the line's end.
        let buf = Buffer::from_str(0, "test", "a\r\nb");
        assert_eq!(buf.snap_to_grapheme_boundary(2), 1);
        assert_eq!(buf.snap_to_grapheme_boundary(1), 1);
        assert_eq!(buf.snap_to_grapheme_boundary(3), 3);
    }

    #[test]
    fn snap_to_grapheme_boundary_clamps_past_end() {
        let buf = Buffer::from_str(0, "test", "ab");
        assert_eq!(buf.snap_to_grapheme_boundary(10), 2);
    }

    #[test]
    fn insert_and_remove() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        buf.insert(5, " world");
        assert_eq!(buf.text.to_string(), "hello world");
        assert!(buf.modified);

        buf.remove(5, 11);
        assert_eq!(buf.text.to_string(), "hello");
    }

    #[test]
    fn edit_generation_increments_on_insert_and_remove() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        assert_eq!(buf.edit_generation, 0);

        buf.insert(5, " world");
        assert_eq!(buf.edit_generation, 1);

        buf.insert(11, "!");
        assert_eq!(buf.edit_generation, 2);

        buf.remove(11, 12);
        assert_eq!(buf.edit_generation, 3);

        // No-op remove (start == end) should NOT increment
        buf.remove(5, 5);
        assert_eq!(buf.edit_generation, 3);
    }

    #[test]
    fn from_file_and_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "hello\nworld\n").unwrap();

        let mut buf = Buffer::from_file(0, &file).unwrap();
        assert_eq!(buf.name, "test.txt");
        assert_eq!(buf.line_count(), 3);
        assert!(!buf.modified);

        buf.insert(5, " there");
        assert!(buf.modified);
        buf.save().unwrap();
        assert!(!buf.modified);

        let content = fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hello there\nworld\n");
    }

    #[test]
    fn externally_modified_detection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "hello").unwrap();

        let mut buf = Buffer::from_file(0, &file).unwrap();
        assert!(!buf.externally_modified());

        // Simulate another program changing the file (force a distinct mtime).
        fs::write(&file, "changed elsewhere").unwrap();
        let f = fs::OpenOptions::new().write(true).open(&file).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
            .unwrap();
        drop(f);
        assert!(buf.externally_modified());

        // Saving takes ownership of the file again.
        buf.insert(0, "x");
        buf.save().unwrap();
        assert!(!buf.externally_modified());
    }

    #[test]
    fn pathless_buffer_is_never_externally_modified() {
        let buf = Buffer::new_scratch(0);
        assert!(!buf.externally_modified());
    }

    #[test]
    fn save_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "hello").unwrap();

        let mut buf = Buffer::from_file(0, &file).unwrap();
        buf.insert(0, "x");
        buf.save().unwrap();

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["test.txt"], "no temp files may remain");
    }

    #[cfg(unix)]
    #[test]
    fn save_preserves_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("script.sh");
        fs::write(&file, "echo hi").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();

        let mut buf = Buffer::from_file(0, &file).unwrap();
        buf.insert(0, "#!/bin/sh\n");
        buf.save().unwrap();

        let mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "save must not clobber file permissions");
    }

    #[cfg(unix)]
    #[test]
    fn save_through_symlink_writes_target_and_keeps_link() {
        let dir = tempfile::tempdir().unwrap();
        let real_dir = dir.path().join("real");
        fs::create_dir(&real_dir).unwrap();
        let target = real_dir.join("file.txt");
        fs::write(&target, "old").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut buf = Buffer::from_file(0, &link).unwrap();
        buf.insert(0, "new ");
        buf.save().unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "saving through a symlink must not replace the link"
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "new old");
        assert_eq!(
            buf.path.as_deref(),
            Some(link.as_path()),
            "the buffer's logical path must stay the link, not the resolved target"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_through_dangling_symlink_creates_target() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link.txt");
        // Relative link target: must resolve against the link's directory.
        std::os::unix::fs::symlink("missing.txt", &link).unwrap();

        let mut buf = Buffer::new_for_path(0, &link);
        buf.insert(0, "hello");
        buf.save().unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink(),
            "saving through a dangling symlink must not replace the link"
        );
        let target = dir.path().join("missing.txt");
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello");
        assert_eq!(fs::read_to_string(&link).unwrap(), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn save_through_symlink_loop_errors() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::os::unix::fs::symlink("b.txt", &a).unwrap();
        std::os::unix::fs::symlink("a.txt", &b).unwrap();

        let mut buf = Buffer::new_for_path(0, &a);
        buf.insert(0, "x");
        assert!(buf.save().is_err(), "a symlink loop must fail the save");
        assert!(
            fs::symlink_metadata(&a).unwrap().file_type().is_symlink(),
            "a failed save must leave the links alone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_hard_links_on_same_inode() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "old").unwrap();
        fs::hard_link(&a, &b).unwrap();

        let mut buf = Buffer::from_file(0, &a).unwrap();
        buf.insert(0, "new ");
        buf.save().unwrap();

        assert_eq!(
            fs::read_to_string(&b).unwrap(),
            "new old",
            "the hard link must see the new content"
        );
        assert_eq!(
            fs::metadata(&a).unwrap().ino(),
            fs::metadata(&b).unwrap().ino(),
            "save must not split the inode of hard-linked files"
        );
    }

    #[cfg(unix)]
    #[test]
    fn externally_modified_detected_through_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        fs::write(&target, "old").unwrap();
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let mut buf = Buffer::from_file(0, &link).unwrap();
        assert!(!buf.externally_modified());

        // Another program rewrites the *target* (force a distinct mtime).
        fs::write(&target, "changed elsewhere").unwrap();
        let f = fs::OpenOptions::new().write(true).open(&target).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
            .unwrap();
        drop(f);
        assert!(
            buf.externally_modified(),
            "external change of the symlink target must be detected"
        );

        // Saving through the link recaptures the mtime of the file
        // actually written.
        buf.insert(0, "x");
        buf.save().unwrap();
        assert!(!buf.externally_modified());
    }

    #[test]
    fn from_file_non_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();

        let result = Buffer::from_file(0, &file);
        assert!(result.is_err());
    }

    #[test]
    fn crlf_detection() {
        let buf = Buffer::from_str(0, "test", "hello\r\nworld\r\n");
        assert_eq!(buf.line_ending, LineEnding::Lf); // from_str doesn't detect
        // But from_file would:
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("crlf.txt");
        fs::write(&file, "hello\r\nworld\r\n").unwrap();
        let buf = Buffer::from_file(0, &file).unwrap();
        assert_eq!(buf.line_ending, LineEnding::CrLf);
    }

    #[test]
    fn binary_file_detection() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.dat");
        // File with null bytes is detected as binary
        fs::write(&file, b"hello\x00world").unwrap();
        let result = Buffer::from_file(0, &file);
        assert!(result.is_err());
        let err = format!("{}", result.err().unwrap());
        assert!(err.contains("binary"));
    }

    #[test]
    fn valid_utf8_file_loads() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("valid.txt");
        fs::write(&file, "valid utf-8 text").unwrap();
        let buf = Buffer::from_file(0, &file).unwrap();
        assert_eq!(buf.text.to_string(), "valid utf-8 text");
    }
}
