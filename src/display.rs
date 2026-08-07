use crate::buffer::Buffer;
use crate::indent::INDENT_WIDTH;
use crate::pane::{visual_lines_for_length, Pane};

pub(super) struct VisualCell {
    pub(super) text: String,
    pub(super) buffer_char_pos: usize,
    pub(super) buffer_byte_start: usize,
    pub(super) buffer_byte_end: usize,
}

/// Terminal column width of a char (0 for combining marks, 2 for CJK/emoji).
/// Tabs are handled separately via `tab_width_at`.
pub(crate) fn char_width(ch: char) -> usize {
    use unicode_width::UnicodeWidthChar;
    terminal_safe_char(ch).width().unwrap_or(0)
}

/// Convert a character into text that is safe to hand to a terminal backend.
/// C0 controls are shown with their Unicode control-picture glyph, DEL uses
/// its control picture, and C1 controls use the replacement character. Tabs
/// and newlines retain their structural meaning and are handled by layout.
pub(crate) fn terminal_safe_char(ch: char) -> char {
    match ch {
        '\t' | '\n' => ch,
        '\u{0}'..='\u{1f}' => {
            char::from_u32(0x2400 + ch as u32).expect("C0 control pictures are valid Unicode")
        }
        '\u{7f}' => '\u{2421}',
        '\u{80}'..='\u{9f}' => '\u{fffd}',
        _ => ch,
    }
}

pub(crate) fn terminal_safe_text(text: &str) -> String {
    text.chars().map(terminal_safe_char).collect()
}

pub(super) fn line_chars_without_ending(buf: &Buffer, line_idx: usize) -> Vec<char> {
    let line = buf.text().line(line_idx);
    let keep = line.len_chars() - crate::buffer::line_break_len_chars(line);
    line.chars().take(keep).collect()
}

pub(super) fn tab_width_at(visual_col: usize) -> usize {
    INDENT_WIDTH - (visual_col % INDENT_WIDTH)
}

pub(super) fn advance_visual_col(visual_col: usize, ch: char) -> usize {
    if ch == '\t' {
        visual_col + tab_width_at(visual_col)
    } else {
        visual_col + char_width(ch)
    }
}

pub(super) fn visual_width_for_chars(line_chars: &[char]) -> usize {
    line_chars.iter().copied().fold(0, advance_visual_col)
}

pub(crate) fn line_visual_width(buf: &Buffer, line_idx: usize) -> usize {
    VisualLineLayout::new(buf, line_idx, usize::MAX).visual_width()
}

pub(crate) fn visual_row_count(buf: &Buffer, line_idx: usize, text_width: usize) -> usize {
    VisualLineLayout::new(buf, line_idx, text_width).row_count()
}

pub(super) fn visual_col_for_buffer_col(line_chars: &[char], buffer_col: usize) -> usize {
    visual_width_for_chars(&line_chars[..buffer_col.min(line_chars.len())])
}

/// Visual (row, column) of a buffer position within its own line's wrap
/// segments, using tab-expanded display widths. The last wrap segment has no
/// continuation marker and holds a full `text_width` columns, so the row is
/// clamped to the line's actual row count.
///
/// When the position is at EOL of a segment that exactly fills the width
/// (an unwrapped line of exactly `text_width` columns, or a wrapped line
/// whose final segment does), the column would compute to `text_width` —
/// one past the last cell. The cursor wraps to column 0 of the next visual
/// row instead (emacs behavior). That row is one past the line's own rows:
/// on screen it is the next buffer line's first row, or a blank row past
/// the end of the buffer. Cursor placement and `compute_scroll_position`
/// both consume this function, so rendering and scrolling agree on the
/// extra row.
pub(crate) fn visual_row_col_in_line(
    buf: &Buffer,
    line_idx: usize,
    buffer_col: usize,
    text_width: usize,
) -> (usize, usize) {
    VisualLineLayout::new(buf, line_idx, text_width).row_col(buffer_col)
}

/// A pane's `scroll_row_offset` clamped to the top line's actual visual
/// height. The stored offset can go stale when a resize or an edit changes
/// how the `scroll_top` line wraps; every consumer (renderer, cursor
/// placement, mouse mapping) clamps through this so they agree.
pub(crate) fn clamped_row_offset(pane: &Pane, buf: &Buffer, text_width: usize) -> usize {
    if pane.scroll_row_offset() == 0 || pane.scroll_top() >= buf.line_count() {
        return 0;
    }
    let top_rows = visual_lines_for_length(line_visual_width(buf, pane.scroll_top()), text_width);
    pane.scroll_row_offset().min(top_rows - 1)
}

#[cfg(test)]
pub(crate) fn buffer_col_for_visual_col(
    buf: &Buffer,
    line_idx: usize,
    target_visual_col: usize,
) -> usize {
    VisualLineLayout::new(buf, line_idx, usize::MAX).buffer_col_for_visual_col(target_visual_col)
}

pub(crate) fn buffer_col_for_visual_position(
    buf: &Buffer,
    line_idx: usize,
    visual_row: usize,
    visual_col: usize,
    text_width: usize,
) -> usize {
    VisualLineLayout::new(buf, line_idx, text_width).buffer_col_at(visual_row, visual_col)
}

#[cfg(test)]
pub(super) fn buffer_col_for_visual_col_general(
    buf: &Buffer,
    line_idx: usize,
    target_visual_col: usize,
) -> usize {
    let line_chars = line_chars_without_ending(buf, line_idx);
    let mut visual_col = 0;

    for (buffer_col, ch) in line_chars.iter().enumerate() {
        // Combining marks occupy no column of their own; never land on them.
        if *ch != '\t' && char_width(*ch) == 0 {
            continue;
        }
        if target_visual_col <= visual_col {
            return buffer_col;
        }

        let width = if *ch == '\t' {
            tab_width_at(visual_col)
        } else {
            char_width(*ch)
        };
        if target_visual_col < visual_col + width {
            // Inside a multi-column char (tab or wide char): land after it.
            return buffer_col + 1;
        }
        visual_col += width;
    }

    line_chars.len()
}

pub(super) struct VisualRow {
    pub(super) cells: Vec<VisualCell>,
    pub(super) continues: bool,
}

pub(super) struct VisibleVisualRows {
    pub(super) rows: Vec<VisualRow>,
    /// True when the returned rows include the end of the buffer line. If
    /// false, the viewport filled while more wrapped rows remained.
    pub(super) exhausted: bool,
}

/// One authority for buffer-line display geometry. Printable ASCII lines
/// have constant one-cell width, so positions and wrapped rows can be
/// indexed directly through the Rope. Lines containing tabs, controls, or
/// Unicode retain the general streaming width calculation.
pub(super) struct VisualLineLayout<'a> {
    pub(super) buf: &'a Buffer,
    pub(super) line_idx: usize,
    pub(super) text_width: usize,
    pub(super) line_start_char: usize,
    pub(super) line_start_byte: usize,
    pub(super) line_len: usize,
    pub(super) plain_ascii: bool,
}

impl<'a> VisualLineLayout<'a> {
    pub(super) fn new(buf: &'a Buffer, line_idx: usize, text_width: usize) -> Self {
        let line = buf.text().line(line_idx);
        let line_len = line.len_chars() - crate::buffer::line_break_len_chars(line);
        let plain_ascii = text_width > 1 && buf.line_is_printable_ascii(line_idx);
        Self {
            buf,
            line_idx,
            text_width,
            line_start_char: buf.text().line_to_char(line_idx),
            line_start_byte: buf.text().line_to_byte(line_idx),
            line_len,
            plain_ascii,
        }
    }

    pub(super) fn visual_width(&self) -> usize {
        if self.plain_ascii {
            self.line_len
        } else {
            let line = self.buf.text().line(self.line_idx);
            line.chars().take(self.line_len).fold(0, advance_visual_col)
        }
    }

    pub(super) fn row_count(&self) -> usize {
        visual_lines_for_length(self.visual_width(), self.text_width)
    }

    pub(super) fn row_col(&self, buffer_col: usize) -> (usize, usize) {
        if !self.plain_ascii && self.text_width > 1 {
            let target = self.line_start_char + buffer_col.min(self.line_len);
            let mut result = None;
            visit_streamed_visual_rows(
                self.buf,
                self.line_idx,
                self.text_width,
                |row_index, row| {
                    if let Some(col) = row
                        .cells
                        .iter()
                        .position(|cell| cell.buffer_char_pos >= target)
                    {
                        result = Some((row_index, col));
                        return false;
                    }
                    if !row.continues && target == self.line_start_char + self.line_len {
                        let col = row.cells.len();
                        result = Some(if col >= self.text_width {
                            (row_index + 1, 0)
                        } else {
                            (row_index, col)
                        });
                        return false;
                    }
                    true
                },
            );
            if let Some(result) = result {
                return result;
            }
        }
        let visual_col = if self.plain_ascii {
            buffer_col.min(self.line_len)
        } else {
            let chars = line_chars_without_ending(self.buf, self.line_idx);
            visual_col_for_buffer_col(&chars, buffer_col)
        };
        let line_visual_width = self.visual_width();
        let (row, col) = if self.text_width > 1 && line_visual_width > self.text_width {
            let chars_per_segment = self.text_width - 1;
            let last_row = visual_lines_for_length(line_visual_width, self.text_width) - 1;
            let row = (visual_col / chars_per_segment).min(last_row);
            (row, visual_col - row * chars_per_segment)
        } else {
            (0, visual_col)
        };
        if self.text_width > 0 && col >= self.text_width {
            (row + 1, 0)
        } else {
            (row, col)
        }
    }

    #[cfg(test)]
    pub(super) fn buffer_col_for_visual_col(&self, target_visual_col: usize) -> usize {
        if self.plain_ascii {
            target_visual_col.min(self.line_len)
        } else {
            buffer_col_for_visual_col_general(self.buf, self.line_idx, target_visual_col)
        }
    }

    pub(super) fn buffer_col_at(&self, visual_row: usize, visual_col: usize) -> usize {
        if self.plain_ascii {
            let start = if self.row_count() == 1 {
                0
            } else {
                visual_row.saturating_mul(self.text_width - 1)
            };
            return start.saturating_add(visual_col).min(self.line_len);
        }
        let visible =
            stream_visible_visual_rows(self.buf, self.line_idx, self.text_width, visual_row, 1);
        let Some(row) = visible.rows.first() else {
            return self.line_len;
        };
        if visual_col == 0 {
            return row
                .cells
                .first()
                .map(|cell| cell.buffer_char_pos - self.line_start_char)
                .unwrap_or(self.line_len);
        }
        row.cells
            .get(visual_col.min(row.cells.len()) - 1)
            .map(|cell| cell.buffer_char_pos - self.line_start_char + 1)
            .unwrap_or(self.line_len)
            .min(self.line_len)
    }

    pub(super) fn visible_rows(&self, skip_rows: usize, max_rows: usize) -> VisibleVisualRows {
        if !self.plain_ascii {
            return stream_visible_visual_rows(
                self.buf,
                self.line_idx,
                self.text_width,
                skip_rows,
                max_rows,
            );
        }
        if max_rows == 0 {
            return VisibleVisualRows {
                rows: Vec::new(),
                exhausted: false,
            };
        }

        let total_rows = self.row_count();
        if skip_rows >= total_rows {
            return VisibleVisualRows {
                rows: Vec::new(),
                exhausted: true,
            };
        }
        let chars_per_segment = self.text_width - 1;
        let mut rows = Vec::with_capacity(max_rows.min(total_rows - skip_rows));
        for row_index in skip_rows..total_rows.min(skip_rows + max_rows) {
            let start = if total_rows == 1 {
                0
            } else {
                row_index * chars_per_segment
            };
            let remaining = self.line_len - start;
            let continues = remaining > self.text_width;
            let len = if continues {
                chars_per_segment
            } else {
                remaining.min(self.text_width)
            };
            let cells = self
                .buf
                .text()
                .chars_at(self.line_start_char + start)
                .take(len)
                .enumerate()
                .map(|(offset, ch)| {
                    let buffer_col = start + offset;
                    VisualCell {
                        text: ch.to_string(),
                        buffer_char_pos: self.line_start_char + buffer_col,
                        buffer_byte_start: self.line_start_byte + buffer_col,
                        buffer_byte_end: self.line_start_byte + buffer_col + 1,
                    }
                })
                .collect();
            rows.push(VisualRow { cells, continues });
        }
        let exhausted = skip_rows + rows.len() == total_rows;
        VisibleVisualRows { rows, exhausted }
    }
}

/// Stream just the requested visual rows of one buffer line. The old render
/// path expanded the complete line into individually allocated cells before
/// selecting the viewport; a multi-megabyte minified line therefore cost
/// multi-megabyte work on every frame even at its beginning.
pub(super) fn visit_streamed_visual_rows(
    buf: &Buffer,
    line_idx: usize,
    text_width: usize,
    mut visit: impl FnMut(usize, VisualRow) -> bool,
) -> bool {
    if text_width == 0 {
        visit(
            0,
            VisualRow {
                cells: Vec::new(),
                continues: false,
            },
        );
        return true;
    }

    let line = buf.text().line(line_idx);
    let keep = line.len_chars() - crate::buffer::line_break_len_chars(line);
    let line_start_char = buf.text().line_to_char(line_idx);
    let line_start_byte = buf.text().line_to_byte(line_idx);
    let mut chars = line.chars().take(keep).enumerate();
    let mut visual_col = 0usize;
    let mut byte_offset = line_start_byte;
    let mut carry = Vec::new();
    let mut input_exhausted = false;
    let mut row_index = 0usize;
    loop {
        let mut cells = std::mem::take(&mut carry);

        // Read one cell beyond the physical row width. That lookahead tells
        // us whether a width-filling row is final (and may use every column)
        // or continued (and must reserve the last column for '\\').
        while cells.len() <= text_width && !input_exhausted {
            let Some((buffer_col, ch)) = chars.next() else {
                input_exhausted = true;
                break;
            };
            let buffer_char_pos = line_start_char + buffer_col;
            let buffer_byte_start = byte_offset;
            let buffer_byte_end = buffer_byte_start + ch.len_utf8();
            byte_offset = buffer_byte_end;
            if ch == '\t' {
                let width = tab_width_at(visual_col);
                for _ in 0..width {
                    cells.push(VisualCell {
                        text: " ".to_string(),
                        buffer_char_pos,
                        buffer_byte_start,
                        buffer_byte_end,
                    });
                }
                visual_col += width;
                continue;
            }
            match char_width(ch) {
                0 => {
                    // Combining marks belong to the preceding glyph. The
                    // one-cell lookahead keeps that glyph available here
                    // even when it sits at the wrap boundary.
                    if let Some(index) = cells.iter().rposition(|cell| !cell.text.is_empty()) {
                        cells[index].text.push(ch);
                        let previous_char = cells[index].buffer_char_pos;
                        for cell in cells
                            .iter_mut()
                            .rev()
                            .take_while(|cell| cell.buffer_char_pos == previous_char)
                        {
                            cell.buffer_byte_end = buffer_byte_end;
                        }
                    }
                }
                2 => {
                    cells.push(VisualCell {
                        text: terminal_safe_char(ch).to_string(),
                        buffer_char_pos,
                        buffer_byte_start,
                        buffer_byte_end,
                    });
                    cells.push(VisualCell {
                        text: String::new(),
                        buffer_char_pos,
                        buffer_byte_start,
                        buffer_byte_end,
                    });
                    visual_col += 2;
                }
                _ => {
                    cells.push(VisualCell {
                        text: terminal_safe_char(ch).to_string(),
                        buffer_char_pos,
                        buffer_byte_start,
                        buffer_byte_end,
                    });
                    visual_col += 1;
                }
            }
        }

        let continues = !input_exhausted || cells.len() > text_width;
        if continues {
            let content_width = if text_width > 1 { text_width - 1 } else { 1 };
            let mut split = content_width.min(cells.len());
            // A double-width glyph is represented by a text cell followed
            // by an empty continuation cell. If the boundary falls between
            // them, move the complete glyph to the next row.
            if text_width > 1
                && split > 0
                && split < cells.len()
                && cells[split].text.is_empty()
                && cells[split - 1].buffer_char_pos == cells[split].buffer_char_pos
            {
                split -= 1;
            }
            carry = cells.split_off(split);
        }

        if !visit(row_index, VisualRow { cells, continues }) {
            return !continues;
        }

        if !continues {
            return true;
        }
        row_index += 1;
    }
}

pub(super) fn stream_visible_visual_rows(
    buf: &Buffer,
    line_idx: usize,
    text_width: usize,
    skip_rows: usize,
    max_rows: usize,
) -> VisibleVisualRows {
    if max_rows == 0 {
        return VisibleVisualRows {
            rows: Vec::new(),
            exhausted: false,
        };
    }
    let mut rows = Vec::with_capacity(max_rows);
    let exhausted = visit_streamed_visual_rows(buf, line_idx, text_width, |row_index, row| {
        if row_index >= skip_rows {
            rows.push(row);
        }
        rows.len() < max_rows
    });
    VisibleVisualRows { rows, exhausted }
}

pub(super) fn visible_visual_rows(
    buf: &Buffer,
    line_idx: usize,
    text_width: usize,
    skip_rows: usize,
    max_rows: usize,
) -> VisibleVisualRows {
    VisualLineLayout::new(buf, line_idx, text_width).visible_rows(skip_rows, max_rows)
}
