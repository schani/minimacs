use anyhow::{bail, Result};
use ropey::Rope;
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

#[allow(dead_code)]
pub struct Buffer {
    pub id: BufferId,
    pub text: Rope,
    pub path: Option<PathBuf>,
    pub name: String,
    pub modified: bool,
    pub read_only: bool,
    pub line_ending: LineEnding,
    pub history: History,
    pub syntax: Option<SyntaxState>,
    pub edit_generation: usize,
    /// Modification time of the file when we last loaded or saved it.
    /// Used to detect external changes before clobbering them on save.
    disk_mtime: Option<std::time::SystemTime>,
}

#[allow(dead_code)]
impl Buffer {
    pub fn new_scratch(id: BufferId) -> Self {
        Self {
            id,
            text: Rope::new(),
            path: None,
            name: "*scratch*".to_string(),
            modified: false,
            read_only: false,
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
            read_only: false,
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
            read_only: false,
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
            read_only: false,
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
    pub fn save_as(&mut self, path: &Path) -> Result<()> {
        use std::io::Write;

        let content: String = self.text.to_string();

        let dir = match path.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
        tmp.write_all(content.as_bytes())?;
        // Keep the target's permissions; a fresh temp file defaults to 0600.
        if let Ok(meta) = fs::metadata(path) {
            tmp.as_file().set_permissions(meta.permissions())?;
        }
        tmp.as_file().sync_all()?;
        tmp.persist(path)?;

        self.path = Some(path.to_path_buf());
        self.disk_mtime = fs::metadata(path).and_then(|m| m.modified()).ok();
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

    /// Length of line in chars, excluding newline.
    pub fn line_len_chars(&self, line_idx: usize) -> usize {
        let line = self.text.line(line_idx);
        let len = line.len_chars();
        // Subtract trailing newline chars
        if len == 0 {
            return 0;
        }
        let last = line.char(len - 1);
        if last == '\n' {
            if len >= 2 && line.char(len - 2) == '\r' {
                len - 2
            } else {
                len - 1
            }
        } else {
            len
        }
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

    #[test]
    fn from_file_non_utf8() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("binary.bin");
        fs::write(&file, &[0xff, 0xfe, 0x00]).unwrap();

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
