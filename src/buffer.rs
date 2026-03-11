use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use ropey::Rope;
use tree_sitter::{InputEdit, Point};

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
    /// Pending tree-sitter InputEdits accumulated since last highlight.
    /// The render path drains these via `take_pending_edits()`.
    pub pending_ts_edits: RefCell<Vec<InputEdit>>,
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
            pending_ts_edits: RefCell::new(Vec::new()),
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
            pending_ts_edits: RefCell::new(Vec::new()),
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
            pending_ts_edits: RefCell::new(Vec::new()),
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
            pending_ts_edits: RefCell::new(Vec::new()),
        })
    }

    pub fn save(&mut self) -> Result<()> {
        let path = match &self.path {
            Some(p) => p.clone(),
            None => bail!("Buffer has no file path"),
        };
        let content: String = self.text.to_string();
        fs::write(&path, &content)?;
        self.history.mark_clean();
        self.modified = false;
        Ok(())
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
        // Record InputEdit BEFORE mutating the Rope (byte offsets are against current state).
        let start_byte = self.text.char_to_byte(char_idx);
        self.pending_ts_edits.borrow_mut().push(InputEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte: start_byte + text.len(),
            start_position: Point { row: 0, column: 0 },
            old_end_position: Point { row: 0, column: 0 },
            new_end_position: Point { row: 0, column: 0 },
        });
        self.text.insert(char_idx, text);
        self.modified = true;
        self.edit_generation += 1;
    }

    /// Remove chars in range [start..end).
    pub fn remove(&mut self, start: usize, end: usize) {
        if start < end {
            // Record InputEdit BEFORE mutating the Rope.
            let start_byte = self.text.char_to_byte(start);
            let old_end_byte = self.text.char_to_byte(end);
            self.pending_ts_edits.borrow_mut().push(InputEdit {
                start_byte,
                old_end_byte,
                new_end_byte: start_byte,
                start_position: Point { row: 0, column: 0 },
                old_end_position: Point { row: 0, column: 0 },
                new_end_position: Point { row: 0, column: 0 },
            });
            self.text.remove(start..end);
            self.modified = true;
            self.edit_generation += 1;
        }
    }

    /// Take all pending tree-sitter edits, leaving the vec empty.
    pub fn take_pending_edits(&self) -> Vec<InputEdit> {
        std::mem::take(&mut self.pending_ts_edits.borrow_mut())
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

    #[test]
    fn pending_edits_insert_single_char() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        buf.insert(5, "!");
        let edits = buf.take_pending_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start_byte, 5);
        assert_eq!(edits[0].old_end_byte, 5);
        assert_eq!(edits[0].new_end_byte, 6);
    }

    #[test]
    fn pending_edits_delete() {
        let mut buf = Buffer::from_str(0, "test", "hello world");
        // Remove " world" (chars 5..11)
        buf.remove(5, 11);
        let edits = buf.take_pending_edits();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].start_byte, 5);
        assert_eq!(edits[0].old_end_byte, 11);
        assert_eq!(edits[0].new_end_byte, 5);
    }

    #[test]
    fn pending_edits_accumulate() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        buf.insert(5, " world");
        buf.insert(11, "!");
        buf.remove(0, 5); // remove "hello"
        let edits = buf.take_pending_edits();
        assert_eq!(edits.len(), 3);
    }

    #[test]
    fn take_pending_edits_drains() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        buf.insert(5, "!");
        let edits = buf.take_pending_edits();
        assert_eq!(edits.len(), 1);
        // Second take should be empty
        let edits2 = buf.take_pending_edits();
        assert!(edits2.is_empty());
    }

    #[test]
    fn pending_edits_multibyte_char() {
        let mut buf = Buffer::from_str(0, "test", "café");
        // 'é' is 2 bytes, so "café" = 5 bytes (c=1, a=1, f=1, é=2)
        // Insert at char 4 (after 'é')
        buf.insert(4, "!");
        let edits = buf.take_pending_edits();
        assert_eq!(edits[0].start_byte, 5); // byte offset after "café"
        assert_eq!(edits[0].new_end_byte, 6);
    }
}
