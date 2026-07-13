use anyhow::{bail, Result};
use ropey::{str_utils::byte_to_char_idx, Rope, RopeSlice};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use tree_house::tree_sitter::{InputEdit, Point};
use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

use crate::history::History;
use crate::syntax::{self, SyntaxState};

pub type BufferId = usize;

const LINE_CLASS_CACHE_CAPACITY: usize = 8;
type DiskFingerprint = [u8; 32];

#[derive(PartialEq, Eq)]
enum DiskState {
    Missing,
    Present {
        modified: Option<std::time::SystemTime>,
        fingerprint: DiskFingerprint,
    },
}

#[derive(Clone, Copy)]
struct CachedLineClass {
    generation: usize,
    line_idx: usize,
    printable_ascii: bool,
}

/// The file's on-disk line-break encoding, detected at load and applied
/// at save. In-memory text is always LF-only regardless of this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

/// Char length of the line break terminating a ropey line slice: 1 for
/// a trailing `\n` — the only break ropey recognizes with
/// `default-features = false` — or 0 on the final line. Every consumer
/// that strips a line's break — line lengths, rendering, mouse mapping,
/// kill-line — must use this so it agrees with where ropey breaks lines.
pub(crate) fn line_break_len_chars(line: RopeSlice) -> usize {
    let len = line.len_chars();
    if len > 0 && line.char(len - 1) == '\n' {
        1
    } else {
        0
    }
}

/// Find the extended grapheme boundary immediately before `char_idx` without
/// flattening the Rope. `GraphemeCursor` requests adjacent chunks and Unicode
/// pre-context as needed, so ordinary movement examines only the local chunk
/// while arbitrarily long clusters remain correct across chunk boundaries.
fn prev_grapheme_boundary_in_slice(slice: RopeSlice<'_>, char_idx: usize) -> usize {
    let byte_idx = slice.char_to_byte(char_idx);
    let (mut chunk, mut chunk_byte_idx, mut chunk_char_idx, _) = slice.chunk_at_byte(byte_idx);
    let mut cursor = GraphemeCursor::new(byte_idx, slice.len_bytes(), true);

    loop {
        match cursor.prev_boundary(chunk, chunk_byte_idx) {
            Ok(None) => return 0,
            Ok(Some(boundary)) => {
                return chunk_char_idx + byte_to_char_idx(chunk, boundary - chunk_byte_idx);
            }
            Err(GraphemeIncomplete::PrevChunk) => {
                let previous = slice.chunk_at_byte(chunk_byte_idx - 1);
                chunk = previous.0;
                chunk_byte_idx = previous.1;
                chunk_char_idx = previous.2;
            }
            Err(GraphemeIncomplete::PreContext(context_end)) => {
                let (context, context_start, _, _) = slice.chunk_at_byte(context_end - 1);
                cursor.provide_context(context, context_start);
            }
            Err(_) => unreachable!("valid Rope chunks must satisfy GraphemeCursor"),
        }
    }
}

/// Find the extended grapheme boundary immediately after `char_idx` while
/// walking Rope chunks on demand.
fn next_grapheme_boundary_in_slice(slice: RopeSlice<'_>, char_idx: usize) -> usize {
    let byte_idx = slice.char_to_byte(char_idx);
    let (mut chunk, mut chunk_byte_idx, mut chunk_char_idx, _) = slice.chunk_at_byte(byte_idx);
    let mut cursor = GraphemeCursor::new(byte_idx, slice.len_bytes(), true);

    loop {
        match cursor.next_boundary(chunk, chunk_byte_idx) {
            Ok(None) => return slice.len_chars(),
            Ok(Some(boundary)) => {
                return chunk_char_idx + byte_to_char_idx(chunk, boundary - chunk_byte_idx);
            }
            Err(GraphemeIncomplete::NextChunk) => {
                chunk_byte_idx += chunk.len();
                let next = slice.chunk_at_byte(chunk_byte_idx);
                chunk = next.0;
                chunk_char_idx = next.2;
            }
            Err(GraphemeIncomplete::PreContext(context_end)) => {
                let (context, context_start, _, _) = slice.chunk_at_byte(context_end - 1);
                cursor.provide_context(context, context_start);
            }
            Err(_) => unreachable!("valid Rope chunks must satisfy GraphemeCursor"),
        }
    }
}

fn is_grapheme_boundary_in_slice(slice: RopeSlice<'_>, char_idx: usize) -> bool {
    let byte_idx = slice.char_to_byte(char_idx);
    let (chunk, chunk_byte_idx, _, _) = slice.chunk_at_byte(byte_idx);
    let mut cursor = GraphemeCursor::new(byte_idx, slice.len_bytes(), true);

    loop {
        match cursor.is_boundary(chunk, chunk_byte_idx) {
            Ok(is_boundary) => return is_boundary,
            Err(GraphemeIncomplete::PreContext(context_end)) => {
                let (context, context_start, _, _) = slice.chunk_at_byte(context_end - 1);
                cursor.provide_context(context, context_start);
            }
            Err(_) => unreachable!("valid Rope chunks must satisfy GraphemeCursor"),
        }
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
    /// Small generation-keyed cache for the line classification used by
    /// display geometry. It avoids rescanning a giant unchanged line for each
    /// cursor, scroll, and render calculation without adding edit invalidation
    /// rules; stale generations simply stop matching and age out.
    line_class_cache: RefCell<VecDeque<CachedLineClass>>,
    /// Metadata and exact-byte fingerprint last observed at a successful
    /// load/save boundary.
    disk_state: DiskState,
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
            line_class_cache: RefCell::new(VecDeque::with_capacity(LINE_CLASS_CACHE_CAPACITY)),
            disk_state: DiskState::Missing,
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
            line_class_cache: RefCell::new(VecDeque::with_capacity(LINE_CLASS_CACHE_CAPACITY)),
            disk_state: DiskState::Missing,
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
            line_class_cache: RefCell::new(VecDeque::with_capacity(LINE_CLASS_CACHE_CAPACITY)),
            disk_state: DiskState::Missing,
        }
    }

    pub fn from_file(id: BufferId, path: &Path) -> Result<Self> {
        let mut file = fs::File::open(path)?;
        let modified = file.metadata().and_then(|metadata| metadata.modified()).ok();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        // Detect binary files (contains null bytes)
        if bytes.contains(&0) {
            bail!("File appears to be binary: {}", path.display());
        }

        // Check for non-UTF-8
        let content = match std::str::from_utf8(&bytes) {
            Ok(s) => s,
            Err(_) => bail!("File is not valid UTF-8: {}", path.display()),
        };

        // Detect line ending. The rope is invariantly LF-only: CRLF is a
        // file *encoding*, stripped here and reproduced by `save_as`.
        // A lone \r is not a line ending and passes through as content.
        let (line_ending, content) = if content.contains("\r\n") {
            (LineEnding::CrLf, Cow::Owned(content.replace("\r\n", "\n")))
        } else {
            (LineEnding::Lf, Cow::Borrowed(content))
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
            text: Rope::from_str(&content),
            path: Some(path.to_path_buf()),
            name,
            modified: false,
            line_ending,
            history: History::new(),
            syntax: syntax_state,
            edit_generation: 0,
            line_class_cache: RefCell::new(VecDeque::with_capacity(LINE_CLASS_CACHE_CAPACITY)),
            disk_state: DiskState::Present {
                modified,
                fingerprint: Sha256::digest(&bytes).into(),
            },
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
        let physical = resolve_write_target(path)?;

        let disk_state = if has_other_hard_links(&physical) {
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
            let fingerprint = write_rope_text(&self.text, self.line_ending, &mut file)?;
            file.sync_all()?;
            DiskState::Present {
                modified: file.metadata().and_then(|metadata| metadata.modified()).ok(),
                fingerprint,
            }
        } else {
            let dir = match physical.parent() {
                Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
                _ => PathBuf::from("."),
            };
            let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
            let fingerprint = write_rope_text(&self.text, self.line_ending, &mut tmp)?;
            // Keep the target's permissions; a fresh temp file defaults to 0600.
            if let Ok(meta) = fs::metadata(&physical) {
                tmp.as_file().set_permissions(meta.permissions())?;
            }
            tmp.as_file().sync_all()?;
            let modified = tmp
                .as_file()
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok();
            tmp.persist(&physical)?;
            DiskState::Present {
                modified,
                fingerprint,
            }
        };

        self.path = Some(path.to_path_buf());
        self.disk_state = disk_state;
        // Saving may target a buffer other than the active one (notably
        // during quit), so command dispatch cannot be relied on to have
        // committed this buffer's pending edit group. Advance history to
        // the exact version whose bytes were written before marking it clean.
        self.history.commit();
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
        match read_disk_state(path) {
            Ok(current) => self.disk_state != current,
            // If the current bytes cannot be established, saving must be
            // conservative rather than treating an I/O error as unchanged.
            Err(_) => true,
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
    /// boundary. Positions computed by column arithmetic (line movement's
    /// column clamping, mouse-click mapping) must be snapped so point
    /// never rests mid-cluster, where a backspace or insert would split
    /// the cluster.
    pub fn snap_to_grapheme_boundary(&self, pos: usize) -> usize {
        let pos = pos.min(self.char_count());
        let slice = self.text.slice(..);
        if is_grapheme_boundary_in_slice(slice, pos) {
            pos
        } else {
            prev_grapheme_boundary_in_slice(slice, pos)
        }
    }

    pub(crate) fn prev_grapheme_boundary(&self, pos: usize) -> usize {
        let pos = pos.min(self.char_count());
        if pos == 0 {
            0
        } else {
            prev_grapheme_boundary_in_slice(self.text.slice(..), pos)
        }
    }

    pub(crate) fn next_grapheme_boundary(&self, pos: usize) -> usize {
        let pos = pos.min(self.char_count());
        if pos == self.char_count() {
            pos
        } else {
            next_grapheme_boundary_in_slice(self.text.slice(..), pos)
        }
    }

    /// Length of line in chars, excluding its terminating line break.
    pub fn line_len_chars(&self, line_idx: usize) -> usize {
        let line = self.text.line(line_idx);
        line.len_chars() - line_break_len_chars(line)
    }

    /// Whether a line can use constant-width, one-byte display geometry.
    /// Results are cached because a single command can ask the renderer the
    /// same question several times, and proving it for a multi-megabyte line
    /// is otherwise the dominant cost after local grapheme navigation.
    pub(crate) fn line_is_printable_ascii(&self, line_idx: usize) -> bool {
        let line_idx = line_idx.min(self.line_count().saturating_sub(1));
        {
            let mut cache = self.line_class_cache.borrow_mut();
            if let Some(position) = cache.iter().position(|entry| {
                entry.generation == self.edit_generation && entry.line_idx == line_idx
            }) {
                let entry = cache
                    .remove(position)
                    .expect("position came from the same cache");
                cache.push_back(entry);
                return entry.printable_ascii;
            }
        }

        let line = self.text.line(line_idx);
        let line_len = line.len_chars() - line_break_len_chars(line);
        let printable_ascii = line
            .slice(..line_len)
            .chunks()
            .all(|chunk| chunk.bytes().all(|byte| (b' '..=b'~').contains(&byte)));

        let mut cache = self.line_class_cache.borrow_mut();
        if cache.len() == LINE_CLASS_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back(CachedLineClass {
            generation: self.edit_generation,
            line_idx,
            printable_ascii,
        });
        printable_ascii
    }

    /// Atomically replace chars in `[start, end)` and return the corresponding
    /// tree-sitter edit. A replacement advances `edit_generation` exactly once,
    /// regardless of whether it deletes, inserts, or does both.
    pub(crate) fn replace(
        &mut self,
        start: usize,
        end: usize,
        replacement: &str,
    ) -> Option<InputEdit> {
        let len = self.text.len_chars();
        let start = start.min(len);
        let end = end.min(len).max(start);
        if start == end && replacement.is_empty() {
            return None;
        }

        let edit = input_edit_for_replace(&self.text, start, end, replacement);
        if start < end {
            self.text.remove(start..end);
        }
        if !replacement.is_empty() {
            self.text.insert(start, replacement);
        }
        if let Some(syntax) = self.syntax.as_ref() {
            syntax.apply_edit(self.text.slice(..), edit);
        }
        self.modified = true;
        self.edit_generation += 1;
        Some(edit)
    }

    /// Update the modified flag based on undo history clean state.
    pub fn update_modified(&mut self) {
        self.modified = !self.history.is_clean();
    }
}

/// Stream rope chunks to disk, applying the buffer's on-disk line ending
/// without first flattening the whole buffer into a contiguous `String`.
/// The returned fingerprint covers the encoded bytes actually passed to the
/// writer, including CRLF expansion.
fn write_rope_text(
    text: &Rope,
    line_ending: LineEnding,
    writer: impl Write,
) -> io::Result<DiskFingerprint> {
    let mut writer = FingerprintingWriter::new(writer);
    if line_ending == LineEnding::Lf {
        text.write_to(&mut writer)?;
        return Ok(writer.finish());
    }

    // The rope is LF-only (see `from_file`); a CrLf buffer gets its line
    // ending re-applied at write time. Only `\n` is rewritten, so a content
    // `\r` is untouched — even directly before `\n`, which saves as `\r\r\n`.
    for chunk in text.chunks() {
        let mut rest = chunk;
        while let Some((before_newline, after_newline)) = rest.split_once('\n') {
            writer.write_all(before_newline.as_bytes())?;
            writer.write_all(b"\r\n")?;
            rest = after_newline;
        }
        writer.write_all(rest.as_bytes())?;
    }
    Ok(writer.finish())
}

struct FingerprintingWriter<W> {
    inner: W,
    hasher: Sha256,
}

impl<W> FingerprintingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
        }
    }

    fn finish(self) -> DiskFingerprint {
        self.hasher.finalize().into()
    }
}

impl<W: Write> Write for FingerprintingWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(bytes)?;
        self.hasher.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn read_disk_state(path: &Path) -> io::Result<DiskState> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DiskState::Missing),
        Err(error) => return Err(error),
    };
    let modified = file.metadata().and_then(|metadata| metadata.modified()).ok();
    let mut hasher = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    loop {
        let read = file.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        hasher.update(&bytes[..read]);
    }
    Ok(DiskState::Present {
        modified,
        fingerprint: hasher.finalize().into(),
    })
}

fn input_edit_for_replace(
    text: &Rope,
    start: usize,
    end: usize,
    replacement: &str,
) -> InputEdit {
    let start_byte = text.char_to_byte(start);
    let old_end_byte = text.char_to_byte(end);
    let start_point = tree_sitter_point_at_char(text, start);
    let old_end_point = tree_sitter_point_at_char(text, end);
    let replacement_bytes = replacement.as_bytes();
    let newline_count = replacement_bytes
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count();
    let new_end_point = match replacement_bytes.iter().rposition(|&byte| byte == b'\n') {
        Some(last_newline) => Point {
            row: start_point.row.saturating_add(to_u32(newline_count)),
            col: to_u32(replacement_bytes.len() - last_newline - 1),
        },
        None => Point {
            row: start_point.row,
            col: start_point.col.saturating_add(to_u32(replacement_bytes.len())),
        },
    };

    InputEdit {
        start_byte: to_u32(start_byte),
        old_end_byte: to_u32(old_end_byte),
        new_end_byte: to_u32(start_byte.saturating_add(replacement_bytes.len())),
        start_point,
        old_end_point,
        new_end_point,
    }
}

fn tree_sitter_point_at_char(text: &Rope, char_idx: usize) -> Point {
    // With ropey pinned to `default-features = false`, only `\n` breaks
    // lines — exactly tree-sitter's row semantics — so ropey's line index
    // equals the Point row for arbitrary content. The column is
    // explicitly measured in UTF-8 bytes, not chars.
    let row = text.char_to_line(char_idx);
    let line_start_char = text.line_to_char(row);
    let byte = text.char_to_byte(char_idx);
    let line_start_byte = text.char_to_byte(line_start_char);
    Point {
        row: to_u32(row),
        col: to_u32(byte - line_start_byte),
    }
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
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

    /// Chars that were line breaks under ropey's `unicode_lines` feature
    /// but are ordinary content with `default-features = false`: lone CR,
    /// VT, FF, NEL, LS, PS.
    const EX_LINE_BREAKS: [char; 6] = [
        '\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}',
    ];

    #[test]
    fn only_newline_breaks_lines() {
        for ch in EX_LINE_BREAKS {
            let buf = Buffer::from_str(0, "test", &format!("ab{ch}cd\n"));
            assert_eq!(buf.line_count(), 2, "{ch:?} must not break lines");
            assert_eq!(buf.line_len_chars(0), 5, "{ch:?} counts as content");
        }
    }

    #[test]
    fn line_break_len_chars_counts_only_newline() {
        let buf = Buffer::from_str(0, "test", "ab\ncd");
        assert_eq!(line_break_len_chars(buf.text.line(0)), 1);
        assert_eq!(line_break_len_chars(buf.text.line(1)), 0, "final line");
        for ch in EX_LINE_BREAKS {
            // A trailing ex-break char is part of the line's text.
            let buf = Buffer::from_str(0, "test", &format!("ab{ch}"));
            assert_eq!(line_break_len_chars(buf.text.line(0)), 0, "{ch:?}");
        }
        let empty = Buffer::from_str(0, "test", "");
        assert_eq!(line_break_len_chars(empty.text.line(0)), 0);
    }

    #[test]
    fn line_col_to_char_clamps_at_line_text_end() {
        // The FF is content, so col clamps to 3, before the \n.
        let buf = Buffer::from_str(0, "test", "a\u{0c}b\n");
        assert_eq!(buf.line_col_to_char(0, 10), 3);
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
    fn snap_to_grapheme_boundary_clamps_past_end() {
        let buf = Buffer::from_str(0, "test", "ab");
        assert_eq!(buf.snap_to_grapheme_boundary(10), 2);
    }

    #[test]
    fn grapheme_movement_crosses_rope_chunks() {
        // One extended grapheme deliberately spans several Rope chunks.
        let marks = "\u{301}".repeat(3_000);
        let buf = Buffer::from_str(0, "test", &format!("x{marks}z"));
        let z = 1 + marks.chars().count();

        assert_eq!(buf.next_grapheme_boundary(0), z);
        assert_eq!(buf.prev_grapheme_boundary(z), 0);
        assert_eq!(buf.snap_to_grapheme_boundary(z / 2), 0);
        assert_eq!(buf.next_grapheme_boundary(z), z + 1);
        assert_eq!(buf.prev_grapheme_boundary(z + 1), z);
    }

    #[test]
    fn grapheme_movement_treats_newline_as_one_step() {
        let buf = Buffer::from_str(0, "test", "ab\ncd");
        assert_eq!(buf.next_grapheme_boundary(2), 3);
        assert_eq!(buf.prev_grapheme_boundary(3), 2);
    }

    #[test]
    fn grapheme_movement_clamps_to_buffer_bounds() {
        let buf = Buffer::from_str(0, "test", "abc");
        assert_eq!(buf.prev_grapheme_boundary(0), 0);
        assert_eq!(buf.next_grapheme_boundary(3), 3);
        assert_eq!(buf.next_grapheme_boundary(usize::MAX), 3);
        assert_eq!(buf.prev_grapheme_boundary(usize::MAX), 2);
    }

    #[test]
    fn printable_ascii_line_cache_reuses_a_generation() {
        let buf = Buffer::from_str(0, "test", "plain ascii");
        assert!(buf.line_is_printable_ascii(0));
        assert!(buf.line_is_printable_ascii(0));
        assert_eq!(buf.line_class_cache.borrow().len(), 1);
    }

    #[test]
    fn printable_ascii_line_cache_does_not_survive_an_edit_generation() {
        let mut buf = Buffer::from_str(0, "test", "plain ascii");
        assert!(buf.line_is_printable_ascii(0));
        buf.replace(0, 1, "é");
        assert!(!buf.line_is_printable_ascii(0));
        let cache = buf.line_class_cache.borrow();
        assert_eq!(cache.len(), 2);
        assert_ne!(cache[0].generation, cache[1].generation);
    }

    #[test]
    fn replace_can_insert_and_remove() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        buf.replace(5, 5, " world");
        assert_eq!(buf.text.to_string(), "hello world");
        assert!(buf.modified);

        buf.replace(5, 11, "");
        assert_eq!(buf.text.to_string(), "hello");
    }

    #[test]
    fn edit_generation_increments_once_per_replace() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        assert_eq!(buf.edit_generation, 0);

        buf.replace(5, 5, " world");
        assert_eq!(buf.edit_generation, 1);

        buf.replace(11, 11, "!");
        assert_eq!(buf.edit_generation, 2);

        buf.replace(11, 12, "");
        assert_eq!(buf.edit_generation, 3);

        // No-op remove (start == end) should NOT increment
        buf.replace(5, 5, "");
        assert_eq!(buf.edit_generation, 3);
    }

    #[test]
    fn replace_is_one_atomic_generation() {
        let mut buf = Buffer::from_str(0, "test", "hello world");
        buf.replace(6, 11, "tree-house");

        assert_eq!(buf.text.to_string(), "hello tree-house");
        assert_eq!(buf.edit_generation, 1);
        assert!(buf.modified);
    }

    #[test]
    fn input_edit_uses_byte_offsets_and_byte_columns_for_unicode() {
        let text = Rope::from_str("αbc\ndéf");
        let edit = input_edit_for_replace(&text, 1, 3, "x\nλ");

        assert_eq!(edit.start_byte, 2);
        assert_eq!(edit.old_end_byte, 4);
        assert_eq!(edit.new_end_byte, 6);
        assert_eq!(edit.start_point, tree_house::tree_sitter::Point { row: 0, col: 2 });
        assert_eq!(edit.old_end_point, tree_house::tree_sitter::Point { row: 0, col: 4 });
        assert_eq!(edit.new_end_point, tree_house::tree_sitter::Point { row: 1, col: 2 });
    }

    #[test]
    fn input_edit_points_ignore_ex_line_break_chars() {
        // Chars that were ropey line breaks under `unicode_lines` (LS,
        // lone CR, FF) are content: they must not advance the
        // tree-sitter row — only \n does. Chars: a LS b \r c \n d FF e;
        // 'e' (char 8) is row 1, byte-col 2 (LS is 3 bytes, FF is 1).
        let text = Rope::from_str("a\u{2028}b\rc\nd\u{0c}e");
        let edit = input_edit_for_replace(&text, 8, 9, "x");

        assert_eq!(edit.start_byte, 10);
        assert_eq!(edit.start_point, tree_house::tree_sitter::Point { row: 1, col: 2 });
        assert_eq!(edit.old_end_point, tree_house::tree_sitter::Point { row: 1, col: 3 });
    }

    #[test]
    fn input_edit_multiline_endpoint_is_after_last_newline() {
        let text = Rope::from_str("first\nsecond\nthird");
        let edit = input_edit_for_replace(&text, 6, 12, "one\ntwo\nthree");

        assert_eq!(edit.start_point, tree_house::tree_sitter::Point { row: 1, col: 0 });
        assert_eq!(edit.old_end_point, tree_house::tree_sitter::Point { row: 1, col: 6 });
        assert_eq!(edit.new_end_point, tree_house::tree_sitter::Point { row: 3, col: 5 });
        assert_eq!(edit.new_end_byte - edit.start_byte, 13);
    }

    #[test]
    fn empty_replace_is_a_noop() {
        let mut buf = Buffer::from_str(0, "test", "hello");
        buf.replace(2, 2, "");
        assert_eq!(buf.text.to_string(), "hello");
        assert_eq!(buf.edit_generation, 0);
        assert!(!buf.modified);
    }

    #[test]
    fn replace_updates_the_buffer_syntax_tree() {
        let mut buf = Buffer::from_str(0, "test.rs", "fn main() { let value = 1; }\n");
        buf.syntax = SyntaxState::new(crate::syntax::Language::Rust);
        let syntax = buf.syntax.as_ref().unwrap();
        syntax.highlight_rope(buf.text.slice(..), 0..buf.text.len_bytes(), 0);

        let start = buf.text.to_string().find('1').unwrap();
        buf.replace(start, start + 1, "call()");

        let incremental = buf.syntax.as_ref().unwrap().highlight_rope(
            buf.text.slice(..),
            0..buf.text.len_bytes(),
            buf.edit_generation,
        );
        let fresh = SyntaxState::new(crate::syntax::Language::Rust).unwrap();
        let full = fresh.highlight_rope(buf.text.slice(..), 0..buf.text.len_bytes(), 0);
        assert_eq!(
            incremental
                .iter()
                .map(|span| (span.start, span.end, span.style))
                .collect::<Vec<_>>(),
            full.iter()
                .map(|span| (span.start, span.end, span.style))
                .collect::<Vec<_>>()
        );
    }

    fn span_signature(
        spans: &[crate::syntax::StyledSpan],
    ) -> Vec<(usize, usize, ratatui::style::Style)> {
        spans
            .iter()
            .map(|span| (span.start, span.end, span.style))
            .collect()
    }

    #[test]
    fn incremental_syntax_matches_full_parse_across_mixed_edits() {
        let source = "fn main() {\n    let greeting = \"hello\";\n    println!(\"{}\", greeting);\n}\n";
        let mut buf = Buffer::from_str(0, "test.rs", source);
        buf.syntax = SyntaxState::new(crate::syntax::Language::Rust);
        buf.syntax.as_ref().unwrap().highlight_rope(
            buf.text.slice(..),
            0..buf.text.len_bytes(),
            0,
        );
        let replacements = ["", "x", "λ", "\n", "/* note */", "\"text\""];
        let mut random = 0x5eed_f00d_u64;

        for step in 0..48 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = buf.text.len_chars();
            let start = (random as usize) % (len + 1);
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let delete_len = (random as usize) % (len.saturating_sub(start).min(4) + 1);
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let replacement = replacements[(random as usize) % replacements.len()];
            if delete_len == 0 && replacement.is_empty() {
                continue;
            }

            buf.replace(start, start + delete_len, replacement);
            let incremental = buf.syntax.as_ref().unwrap().highlight_rope(
                buf.text.slice(..),
                0..buf.text.len_bytes(),
                buf.edit_generation,
            );
            let fresh = SyntaxState::new(crate::syntax::Language::Rust).unwrap();
            let full = fresh.highlight_rope(buf.text.slice(..), 0..buf.text.len_bytes(), 0);
            assert_eq!(
                span_signature(&incremental),
                span_signature(&full),
                "incremental parse diverged at step {step} after replacing {start}..{} with {replacement:?}; source: {:?}",
                start + delete_len,
                buf.text.to_string(),
            );
        }
    }

    #[test]
    fn incremental_markdown_injections_match_full_parse_after_edits() {
        let source = "# Demo\n\n```rust\nfn answer() -> u32 { 42 }\n```\n\n*tail*\n";
        let mut buf = Buffer::from_str(0, "test.md", source);
        buf.syntax = SyntaxState::new(crate::syntax::Language::Markdown);
        buf.syntax.as_ref().unwrap().highlight_rope(
            buf.text.slice(..),
            0..buf.text.len_bytes(),
            0,
        );

        for (needle, replacement) in [
            ("42", "compute()"),
            ("rust", "javascript"),
            ("*tail*", "**strong λ**"),
        ] {
            let text = buf.text.to_string();
            let start_byte = text.find(needle).unwrap();
            let start = buf.text.byte_to_char(start_byte);
            let end = start + needle.chars().count();
            buf.replace(start, end, replacement);

            let incremental = buf.syntax.as_ref().unwrap().highlight_rope(
                buf.text.slice(..),
                0..buf.text.len_bytes(),
                buf.edit_generation,
            );
            let fresh = SyntaxState::new(crate::syntax::Language::Markdown).unwrap();
            let full = fresh.highlight_rope(buf.text.slice(..), 0..buf.text.len_bytes(), 0);
            assert_eq!(span_signature(&incremental), span_signature(&full));
        }
    }

    #[test]
    fn edit_before_initial_parse_is_included_in_lazy_full_parse() {
        let mut buf = Buffer::from_str(0, "test.rs", "fn old() {}\n");
        buf.syntax = SyntaxState::new(crate::syntax::Language::Rust);
        buf.replace(3, 6, "new_name");

        let lazy = buf.syntax.as_ref().unwrap().highlight_rope(
            buf.text.slice(..),
            0..buf.text.len_bytes(),
            buf.edit_generation,
        );
        let fresh = SyntaxState::new(crate::syntax::Language::Rust).unwrap();
        let full = fresh.highlight_rope(buf.text.slice(..), 0..buf.text.len_bytes(), 0);
        assert_eq!(span_signature(&lazy), span_signature(&full));
    }

    #[test]
    fn write_rope_text_preserves_line_endings_across_chunks() {
        let source = format!("{}\nmiddle\r\n{}\n", "λ".repeat(2_000), "終".repeat(2_000));
        let text = Rope::from_str(&source);
        assert!(text.chunks().count() > 1, "fixture must span rope chunks");

        for (line_ending, expected) in [
            (LineEnding::Lf, source.clone()),
            (LineEnding::CrLf, source.replace('\n', "\r\n")),
        ] {
            let mut written = Vec::new();
            write_rope_text(&text, line_ending, &mut written).unwrap();
            assert_eq!(String::from_utf8(written).unwrap(), expected);
        }
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

        buf.replace(5, 5, " there");
        assert!(buf.modified);
        buf.save().unwrap();
        assert!(!buf.modified);

        let content = fs::read_to_string(&file).unwrap();
        assert_eq!(content, "hello there\nworld\n");
    }

    #[test]
    fn successful_save_commits_pending_history_before_marking_clean() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "old").unwrap();

        let mut buf = Buffer::from_file(0, &file).unwrap();
        buf.history.record_insert(0, "x");
        buf.replace(0, 0, "x");
        buf.save().unwrap();

        assert!(
            buf.history.is_clean(),
            "the exact undo-history version written to disk must be clean"
        );
        assert!(buf.history.undo().is_some());
        buf.update_modified();
        assert!(
            buf.modified,
            "undoing saved contents must make the buffer modified"
        );
    }

    #[test]
    fn failed_save_does_not_commit_pending_history() {
        let dir = tempfile::tempdir().unwrap();
        let missing_parent = dir.path().join("missing");
        let file = missing_parent.join("test.txt");
        let mut buf = Buffer::new_for_path(0, &file);

        buf.history.record_insert(0, "a");
        buf.replace(0, 0, "a");
        assert!(buf.save().is_err());

        buf.history.record_insert(1, "b");
        buf.replace(1, 1, "b");
        let group = buf.history.undo().unwrap();
        assert_eq!(
            group.edits.len(),
            2,
            "a failed save must not split the pending edit group"
        );
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
        buf.replace(0, 0, "x");
        buf.save().unwrap();
        assert!(!buf.externally_modified());
    }

    #[cfg(unix)]
    #[test]
    fn externally_modified_detects_same_mtime_size_and_inode_rewrite() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "before").unwrap();

        let buf = Buffer::from_file(0, &file).unwrap();
        let original = fs::metadata(&file).unwrap();
        let original_mtime = original.modified().unwrap();

        // Simulate a timestamp-preserving deployment tool rewriting the
        // existing file in place with different bytes of the same length.
        fs::write(&file, "after!").unwrap();
        let rewritten = fs::OpenOptions::new().write(true).open(&file).unwrap();
        rewritten.set_modified(original_mtime).unwrap();
        drop(rewritten);

        let current = fs::metadata(&file).unwrap();
        assert_eq!(current.modified().unwrap(), original_mtime);
        assert_eq!(current.len(), original.len());
        assert_eq!(current.ino(), original.ino());
        assert!(
            buf.externally_modified(),
            "different on-disk bytes must be detected even when metadata is unchanged"
        );
    }

    #[test]
    fn externally_modified_baseline_uses_raw_crlf_bytes_after_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "one\r\ntwo\r\n").unwrap();

        let mut buf = Buffer::from_file(0, &file).unwrap();
        buf.replace(3, 3, "!");
        buf.save().unwrap();

        assert_eq!(fs::read(&file).unwrap(), b"one!\r\ntwo\r\n");
        assert!(
            !buf.externally_modified(),
            "save must fingerprint the CRLF-encoded disk bytes, not normalized rope bytes"
        );
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
        buf.replace(0, 0, "x");
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
        buf.replace(0, 0, "#!/bin/sh\n");
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
        buf.replace(0, 0, "new ");
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
        buf.replace(0, 0, "hello");
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
        buf.replace(0, 0, "x");
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
        buf.replace(0, 0, "new ");
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
        buf.replace(0, 0, "x");
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
    fn crlf_file_loads_lf_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("crlf.txt");
        fs::write(&file, "hello\r\nworld\r\n").unwrap();
        let buf = Buffer::from_file(0, &file).unwrap();
        assert_eq!(buf.line_ending, LineEnding::CrLf);
        assert_eq!(buf.text.to_string(), "hello\nworld\n");
        assert!(!buf.modified, "load-time conversion is not an edit");
    }

    #[test]
    fn crlf_file_round_trips_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("crlf.txt");
        fs::write(&file, "hello\r\nworld\r\n").unwrap();
        let mut buf = Buffer::from_file(0, &file).unwrap();
        buf.save().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello\r\nworld\r\n");
    }

    #[test]
    fn edited_crlf_buffer_saves_crlf() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("crlf.txt");
        fs::write(&file, "hello\r\nworld\r\n").unwrap();
        let mut buf = Buffer::from_file(0, &file).unwrap();
        // Positions are in the LF-only rope: "hello\nworld\n".
        buf.replace(5, 5, " there");
        buf.replace(17, 17, "\nmore");
        buf.save().unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "hello there\r\nworld\r\nmore\r\n"
        );
    }

    #[test]
    fn mixed_endings_normalize_on_save() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mixed.txt");
        fs::write(&file, "a\r\nb\nc\r\n").unwrap();
        let mut buf = Buffer::from_file(0, &file).unwrap();
        assert_eq!(buf.line_ending, LineEnding::CrLf);
        assert_eq!(buf.text.to_string(), "a\nb\nc\n");
        buf.save().unwrap();
        // Intended normalization: mixed files save with one uniform ending.
        assert_eq!(fs::read_to_string(&file).unwrap(), "a\r\nb\r\nc\r\n");
    }

    #[test]
    fn lone_cr_is_content_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();

        // In an LF file, a lone \r is left untouched in both directions.
        let lf = dir.path().join("lf.txt");
        fs::write(&lf, "a\rb\nc\n").unwrap();
        let mut buf = Buffer::from_file(0, &lf).unwrap();
        assert_eq!(buf.line_ending, LineEnding::Lf);
        assert_eq!(buf.text.to_string(), "a\rb\nc\n");
        buf.save().unwrap();
        assert_eq!(fs::read_to_string(&lf).unwrap(), "a\rb\nc\n");

        // In a CRLF file, only the \r of a \r\n pair is an encoding
        // artifact; a lone \r survives load and save unchanged.
        let crlf = dir.path().join("crlf.txt");
        fs::write(&crlf, "a\rb\r\nc\r\n").unwrap();
        let mut buf = Buffer::from_file(0, &crlf).unwrap();
        assert_eq!(buf.line_ending, LineEnding::CrLf);
        assert_eq!(buf.text.to_string(), "a\rb\nc\n");
        buf.save().unwrap();
        assert_eq!(fs::read_to_string(&crlf).unwrap(), "a\rb\r\nc\r\n");
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
