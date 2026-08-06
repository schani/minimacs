use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::buffer::Buffer;
use crate::display::*;
use crate::editor::Editor;
use crate::pane::{visual_lines_for_length, Pane};
use crate::syntax_worker::{SyntaxJob, SyntaxWorker};

use super::layout::*;
use super::PendingInput;

/// Render the entire editor UI into the given frame.
pub fn render(
    frame: &mut Frame,
    editor: &Editor,
    syntax_worker: &SyntaxWorker,
    pending_input: PendingInput<'_>,
) {
    let area = frame.area();
    let layout = screen_layout(editor, area);

    // Calculate rects for all panes
    let (pane_rects, separator_rects) = editor.pane_tree.calculate_rects(layout.pane_area);
    let focus_path = editor.pane_tree.focus_path();

    for (path, rect) in &pane_rects {
        let pane = editor.pane_tree.pane_at_focus_path(path);
        let buf = editor.buffer_by_id(pane.buffer_id());
        let is_focused = path.as_slice() == focus_path;

        // Split each pane rect into text area + mode line
        let pane_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),    // text area
                Constraint::Length(1), // mode line
            ])
            .split(*rect);

        let text_area = pane_chunks[0];
        let mode_line_area = pane_chunks[1];

        // Get region only for the focused pane
        let region = if is_focused {
            editor.region()
        } else {
            pane.mark().map(|mark| {
                let start = pane.point().min(mark);
                let end = pane.point().max(mark);
                (start, end)
            })
        };

        // Get search matches for the focused pane
        let search_matches = if is_focused {
            editor.isearch_matches()
        } else {
            Vec::new()
        };
        // match_covering() relies on sorted starts and a uniform match
        // length; isearch_update guarantees both today — catch regressions
        // (e.g. a future regex search) in debug builds.
        debug_assert!(
            search_matches
                .windows(2)
                .all(|w| w[0].0 <= w[1].0 && w[0].1 == w[1].1),
            "isearch matches must be sorted and uniform in length"
        );
        let current_match = if is_focused {
            editor.isearch.as_ref().and_then(|s| s.current_match)
        } else {
            None
        };

        render_pane_text(
            frame,
            buf,
            pane,
            PaneOverlays {
                region,
                search_matches: &search_matches,
                current_match,
            },
            text_area,
            syntax_worker,
        );
        render_pane_mode_line(frame, buf, pane, is_focused, pending_input, mode_line_area);

        // Set cursor position for the focused pane
        if is_focused && !editor.minibuffer.is_active() {
            let (cursor_line, cursor_col) = buf.char_to_line_col(pane.point());
            let text_width = text_area.width as usize;
            let row_offset = clamped_row_offset(pane, buf, text_width);

            // Compute visual row from the top of the scroll_top line,
            // accounting for wrapping and display-only tab expansion.
            // Only place cursor if it's within the visible viewport.
            let mut visual_row: usize = 0;
            let max_rows = text_area.height as usize + row_offset;
            for lidx in pane.scroll_top()..cursor_line {
                let line_visual_width = line_visual_width(buf, lidx);
                visual_row += visual_lines_for_length(line_visual_width, text_width);
                if visual_row >= max_rows {
                    // Cursor is below the viewport; its exact row no longer
                    // matters, so don't walk the rest of the buffer.
                    break;
                }
            }

            // Add the cursor's row within its own line if it wraps.
            let (row_in_line, col_in_segment) =
                visual_row_col_in_line(buf, cursor_line, cursor_col, text_width);
            visual_row += row_in_line;

            let screen_col = col_in_segment as u16;

            // The first row_offset visual rows of the top line are scrolled
            // off; a cursor row before them is above the viewport. Both
            // coordinates are pane-relative; compare against the pane's
            // dimensions, not its absolute right edge.
            if cursor_line >= pane.scroll_top() {
                if let Some(screen_line) = visual_row.checked_sub(row_offset) {
                    if screen_col < text_area.width && screen_line < text_area.height as usize {
                        frame.set_cursor_position((
                            text_area.x + screen_col,
                            text_area.y + screen_line as u16,
                        ));
                    }
                }
            }
        }
    }

    // Render separator bars between horizontally-split panes
    for sep_rect in &separator_rects {
        let sep_style = Style::default()
            .fg(Color::Rgb(200, 200, 200))
            .bg(Color::Rgb(255, 255, 255));
        let lines: Vec<Line> = (0..sep_rect.height)
            .map(|_| Line::from(Span::styled("│", sep_style)))
            .collect();
        let sep_widget = Paragraph::new(lines);
        frame.render_widget(sep_widget, *sep_rect);
    }

    if let Some(comp_area) = layout.completions_area {
        if let Some(candidates) = &editor.minibuffer.completions {
            render_completions(
                frame,
                candidates,
                editor.minibuffer.completion_page,
                comp_area,
            );
        }
    }

    render_minibuffer(frame, &layout.minibuffer, layout.minibuffer_area);
}

/// One terminal column of a rendered line. A tab expands to several cells; a
/// double-width char contributes its cell plus one continuation cell with
/// empty text; combining marks are appended to the preceding cell's text.
#[derive(Clone)]
struct PaneOverlays<'a> {
    region: Option<(usize, usize)>,
    search_matches: &'a [(usize, usize)],
    current_match: Option<usize>,
}

fn render_pane_text(
    frame: &mut Frame,
    buf: &Buffer,
    pane: &Pane,
    overlays: PaneOverlays<'_>,
    area: Rect,
    syntax_worker: &SyntaxWorker,
) {
    let scroll_top = pane.scroll_top();
    let max_visual_rows = area.height as usize;
    let total_lines = buf.line_count();
    let text_width = area.width as usize;
    // Visual rows of the top line scrolled off above the viewport (nonzero
    // only when that line wraps taller than the space above the cursor).
    let row_offset = clamped_row_offset(pane, buf, text_width);

    // First lay out only the visible visual rows. Their exact byte range is
    // then used for syntax highlighting, so a single giant buffer line does
    // not make the style path allocate or scan the whole line.
    let mut visible_rows: Vec<(usize, VisualRow)> = Vec::new();
    let mut line_idx = scroll_top;

    while visible_rows.len() < max_visual_rows && line_idx < total_lines {
        let skip_rows = if line_idx == scroll_top {
            row_offset
        } else {
            0
        };
        let visible = visible_visual_rows(
            buf,
            line_idx,
            text_width,
            skip_rows,
            max_visual_rows - visible_rows.len(),
        );

        for row in visible.rows {
            visible_rows.push((line_idx, row));
        }

        if !visible.exhausted {
            break;
        }
        line_idx += 1;
    }

    let syntax_spans = compute_visible_syntax_spans(buf, &visible_rows, syntax_worker);
    let mut output_lines: Vec<Line> = Vec::with_capacity(max_visual_rows);
    for (line_idx, row) in visible_rows {
        let mut spans = Vec::new();
        if !row.cells.is_empty() {
            build_styled_spans(
                &mut spans,
                &row.cells,
                0,
                row.cells.len(),
                line_idx,
                overlays.region,
                overlays.search_matches,
                overlays.current_match,
                &syntax_spans,
            );
        }
        if row.continues {
            spans.push(Span::styled(
                "\\",
                Style::default().fg(Color::Rgb(35, 120, 147)),
            ));
        }
        output_lines.push(Line::from(spans));
    }

    // Fill remaining rows with ~
    while output_lines.len() < max_visual_rows {
        output_lines.push(Line::from(Span::styled(
            "~",
            Style::default().fg(Color::Rgb(35, 120, 147)),
        )));
    }

    let paragraph = Paragraph::new(output_lines);
    frame.render_widget(paragraph, area);
}

/// Char position of the search match covering `char_pos`, if any.
///
/// Invariants (established by `isearch_update`, asserted per frame in
/// `render()`): `matches` is sorted by start position and every match has
/// the same length — the query's. Overlaps are fine: with equal lengths the
/// match with the largest start ≤ `char_pos` reaches furthest, so only that
/// one needs checking.
fn match_covering(matches: &[(usize, usize)], char_pos: usize) -> Option<usize> {
    let idx = matches.partition_point(|&(pos, _)| pos <= char_pos);
    let &(pos, len) = matches.get(idx.checked_sub(1)?)?;
    (char_pos < pos + len).then_some(pos)
}

/// Build styled spans for a segment of expanded display cells.
#[allow(clippy::too_many_arguments)]
fn build_styled_spans(
    spans: &mut Vec<Span<'static>>,
    visual_cells: &[VisualCell],
    start: usize,
    end: usize,
    _line_idx: usize,
    region: Option<(usize, usize)>,
    search_matches: &[(usize, usize)],
    current_match: Option<usize>,
    syntax_spans: &[crate::syntax::StyledSpan],
) {
    let segment = &visual_cells[start..end];
    if segment.is_empty() {
        return;
    }

    // All matches share the query's length (see `match_covering`).
    let match_len = search_matches.first().map(|&(_, len)| len).unwrap_or(0);

    let char_styles: Vec<Style> = segment
        .iter()
        .map(|cell| {
            let char_pos = cell.buffer_char_pos;

            let in_region = region
                .map(|(rs, re)| char_pos >= rs && char_pos < re)
                .unwrap_or(false);

            if in_region {
                Style::default().bg(Color::Rgb(173, 214, 255))
            } else {
                let is_current_match =
                    current_match.is_some_and(|cm| char_pos >= cm && char_pos < cm + match_len);
                let is_other_match =
                    !is_current_match && match_covering(search_matches, char_pos).is_some();

                if is_current_match {
                    Style::default()
                        .bg(Color::Rgb(168, 172, 148))
                        .fg(Color::Black)
                } else if is_other_match {
                    Style::default()
                        .bg(Color::Rgb(248, 201, 171))
                        .fg(Color::Black)
                } else {
                    syntax_style_at_byte(syntax_spans, cell.buffer_byte_start)
                }
            }
        })
        .collect();

    // RLE: merge consecutive chars with same style into spans
    let mut run_start = 0;
    while run_start < segment.len() {
        let style = char_styles[run_start];
        let mut run_end = run_start + 1;
        while run_end < segment.len() && char_styles[run_end] == style {
            run_end += 1;
        }
        let text: String = segment[run_start..run_end]
            .iter()
            .map(|cell| cell.text.as_str())
            .collect();
        spans.push(Span::styled(text, style));
        run_start = run_end;
    }
}

/// Return the syntax spans intersecting the cells actually laid out for the
/// viewport. Unlike the old byte-vector + `(line, col)` hash map, memory and
/// post-parse work are proportional to visible captures, not line length.
fn compute_visible_syntax_spans(
    buf: &Buffer,
    rows: &[(usize, VisualRow)],
    syntax_worker: &SyntaxWorker,
) -> Vec<crate::syntax::StyledSpan> {
    let Some(syntax) = buf.syntax() else {
        return Vec::new();
    };
    if syntax.is_disabled() {
        return Vec::new();
    }
    let first = rows.iter().find_map(|(_, row)| row.cells.first());
    let last = rows.iter().rev().find_map(|(_, row)| row.cells.last());
    let (Some(first), Some(last)) = (first, last) else {
        return Vec::new();
    };
    let requested = first.buffer_byte_start..last.buffer_byte_end;
    let cached = syntax.background_spans(requested.clone(), buf.edit_generation());
    if !cached.exact {
        let (base_generation, edits) = syntax.background_update_for(buf.edit_generation());
        syntax_worker.submit(SyntaxJob {
            key: syntax.background_key(),
            language: syntax.language,
            base_generation,
            generation: buf.edit_generation(),
            source: buf.text().clone(),
            edits,
            requested,
        });
    }
    cached.spans
}

fn syntax_style_at_byte(spans: &[crate::syntax::StyledSpan], byte: usize) -> Style {
    let index = spans.partition_point(|span| span.end <= byte);
    spans
        .get(index)
        .filter(|span| span.start <= byte && byte < span.end)
        .map(|span| span.style)
        .unwrap_or_default()
}

/// Join the mode line's left and right parts, padding with spaces so the
/// line fills `total_width` display columns. Widths are measured in display
/// columns, not bytes — buffer names can contain multibyte and wide chars.
fn mode_line_text(left: &str, right: &str, total_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let left = terminal_safe_text(left);
    let right = terminal_safe_text(right);
    let padding = total_width.saturating_sub(left.width() + right.width());
    format!("{left}{}{right}", " ".repeat(padding))
}

fn render_pane_mode_line(
    frame: &mut Frame,
    buf: &Buffer,
    pane: &Pane,
    is_focused: bool,
    pending_input: PendingInput<'_>,
    area: Rect,
) {
    let (line, col) = buf.char_to_line_col(pane.point());

    let modified_indicator = if buf.is_modified() { "**" } else { "--" };
    let name = buf.name();

    // Position percentage
    let total_lines = buf.line_count();
    let position = if total_lines <= 1 {
        "All".to_string()
    } else if line == 0 {
        "Top".to_string()
    } else if line >= total_lines - 1 {
        "Bot".to_string()
    } else {
        format!("{}%", line * 100 / total_lines)
    };

    let language_display = buf
        .syntax()
        .as_ref()
        .map(|s| format!("  ({})", s.language.name()))
        .unwrap_or_default();

    let pending = pending_input.display;
    let pending_display = if is_focused && !pending.is_empty() {
        format!("  {pending}")
    } else {
        String::new()
    };

    let left = format!(
        " {} {} ({},{})  {}{}",
        modified_indicator,
        name,
        line + 1,
        col,
        position,
        language_display
    );
    let right = format!("{pending_display} ");

    let mode_line_text = mode_line_text(&left, &right, area.width as usize);

    let style = if is_focused {
        Style::default()
            .bg(Color::Rgb(0, 122, 204))
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(Color::Rgb(223, 223, 223))
            .fg(Color::Rgb(51, 51, 51))
    };

    let mode_line = Paragraph::new(Line::from(Span::styled(mode_line_text, style)));
    frame.render_widget(mode_line, area);
}

fn render_completions(frame: &mut Frame, candidates: &[String], page: usize, area: Rect) {
    let width = area.width as usize;
    let rows = area.height as usize;
    if width == 0 || rows == 0 {
        return;
    }

    // All measurements below are in display columns (unicode-width), never
    // bytes or chars: candidate names can contain multibyte and wide chars.
    use unicode_width::UnicodeWidthStr;
    let candidates: Vec<String> = candidates
        .iter()
        .map(|candidate| terminal_safe_text(candidate))
        .collect();
    let max_len = candidates.iter().map(|c| c.width()).max().unwrap_or(0);
    let (num_cols, _num_rows, col_width) = completions_layout(candidates.len(), max_len, width);

    // How many candidates can we display per page?
    let displayable = rows * num_cols;
    let page_count = candidates.len().div_ceil(displayable).max(1);
    let page = page % page_count;
    let start = page * displayable;

    let bg = Style::default()
        .bg(Color::Rgb(243, 243, 243))
        .fg(Color::Black);

    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut text = String::new();
        let mut text_cols = 0;
        for col in 0..num_cols {
            let idx = start + col * rows + row;
            if idx < candidates.len() && idx < start + displayable {
                let name = &candidates[idx];
                let name_cols = name.width();
                if text_cols + name_cols <= width {
                    text.push_str(name);
                    // Pad to column width
                    let padding = col_width.saturating_sub(name_cols);
                    let remaining_width = width.saturating_sub(text_cols + name_cols);
                    let pad = padding.min(remaining_width);
                    text.extend(std::iter::repeat_n(' ', pad));
                    text_cols += name_cols + pad;
                } else {
                    // Truncate to fit; the row is full after this.
                    let remaining = width.saturating_sub(text_cols);
                    let (prefix, prefix_cols) = truncate_to_width(name, remaining);
                    text.push_str(prefix);
                    text_cols += prefix_cols;
                    break;
                }
            }
        }
        // Pad remaining width with spaces for background fill
        if text_cols < width {
            text.extend(std::iter::repeat_n(' ', width - text_cols));
        }

        // If this is the last row and there are multiple pages, show page indicator
        if row == rows - 1 && page_count > 1 {
            let suffix = format!("[Page {}/{}]", page + 1, page_count);
            let suffix_cols = suffix.width();
            if suffix_cols <= width {
                let keep = width - suffix_cols;
                let (prefix, prefix_cols) = truncate_to_width(&text, keep);
                let mut truncated = prefix.to_string();
                // A dropped straddling wide char leaves a gap; pad so the
                // indicator still lands flush against the right edge.
                truncated.extend(std::iter::repeat_n(' ', keep - prefix_cols));
                truncated.push_str(&suffix);
                text = truncated;
            }
        }

        lines.push(Line::from(Span::styled(text, bg)));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn render_minibuffer(frame: &mut Frame, layout: &MinibufferLayout, area: Rect) {
    let lines: Vec<Line> = layout
        .visible_rows
        .iter()
        .map(String::as_str)
        .map(Line::from)
        .collect();
    let minibuffer = Paragraph::new(lines);
    frame.render_widget(minibuffer, area);

    if let Some((row, col)) = layout.cursor {
        let x = area.x.saturating_add(col as u16);
        let y = area.y.saturating_add(row as u16);
        if x < area.right() && y < area.bottom() {
            frame.set_cursor_position((x, y));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_lines_no_wrap() {
        // Lines that fit within text_width don't wrap
        assert_eq!(visual_lines_for_length(5, 18), 1);
        assert_eq!(visual_lines_for_length(18, 18), 1); // exactly fits
        assert_eq!(visual_lines_for_length(0, 18), 1);
    }

    #[test]
    fn visual_lines_single_wrap() {
        // text_width=18, cps=17. Line of 19 chars wraps into 2 visual lines.
        assert_eq!(visual_lines_for_length(19, 18), 2);
        // 35 chars: seg1=17+\, seg2=18 (fits). 2 visual lines.
        assert_eq!(visual_lines_for_length(35, 18), 2);
    }

    #[test]
    fn visual_lines_double_wrap() {
        // text_width=18, cps=17. 36 chars: seg1=17, seg2=17, seg3=2. 3 lines.
        assert_eq!(visual_lines_for_length(36, 18), 3);
    }

    #[test]
    fn visual_lines_triple_wrap() {
        // text_width=13, cps=12. 36 chars:
        // seg1=12, remaining=24. excess=36-13=23. 1+ceil(23/12)=1+2=3.
        assert_eq!(visual_lines_for_length(36, 13), 3);
    }

    #[test]
    fn visual_row_col_wraps_eol_of_exactly_full_line() {
        // Unwrapped 10-char line at width 10: EOL computes to column 10
        // (one past the last cell) and must wrap to (1, 0). The virtual
        // row is one past the line's own rows; renderer and scroll both
        // count it.
        let buf = Buffer::from_str(0, "t", "aaaaaaaaaa");
        assert_eq!(visual_row_col_in_line(&buf, 0, 9, 10), (0, 9));
        assert_eq!(visual_row_col_in_line(&buf, 0, 10, 10), (1, 0));
    }

    #[test]
    fn visual_row_col_wraps_eol_of_exactly_full_final_segment() {
        // 19 chars at width 10 (cps=9): rows [0..9)+'\' and [9..19); the
        // final segment exactly fills the width, so EOL wraps to (2, 0).
        let buf = Buffer::from_str(0, "t", "abcdefghijklmnopqrs");
        assert_eq!(visual_row_col_in_line(&buf, 0, 18, 10), (1, 9));
        assert_eq!(visual_row_col_in_line(&buf, 0, 19, 10), (2, 0));
    }

    #[test]
    fn visual_row_col_eol_of_non_full_final_segment_stays_on_its_row() {
        // 18 chars at width 10: the final segment holds 9 columns; EOL is
        // a real cell position on that row and must not wrap.
        let buf = Buffer::from_str(0, "t", "abcdefghijklmnopqr");
        assert_eq!(visual_row_col_in_line(&buf, 0, 18, 10), (1, 9));
    }

    #[test]
    fn visual_row_col_wraps_eol_of_exactly_full_cjk_line() {
        // 5 CJK chars = 10 visual columns at width 10.
        let buf = Buffer::from_str(0, "t", "你好你好你");
        assert_eq!(visual_row_col_in_line(&buf, 0, 5, 10), (1, 0));
    }

    // === isearch match lookup tests ===

    #[test]
    fn match_covering_empty() {
        assert_eq!(match_covering(&[], 5), None);
    }

    #[test]
    fn match_covering_start_inclusive_end_exclusive() {
        let matches = [(10, 3)];
        assert_eq!(match_covering(&matches, 9), None);
        assert_eq!(match_covering(&matches, 10), Some(10));
        assert_eq!(match_covering(&matches, 12), Some(10));
        assert_eq!(match_covering(&matches, 13), None);
    }

    #[test]
    fn match_covering_gap_between_matches() {
        let matches = [(0, 2), (10, 2)];
        assert_eq!(match_covering(&matches, 1), Some(0));
        assert_eq!(match_covering(&matches, 5), None);
        assert_eq!(match_covering(&matches, 10), Some(10));
    }

    #[test]
    fn match_covering_overlapping_matches() {
        // Searching "aa" in "aaaa" produces overlapping matches; equal
        // lengths mean the latest start <= pos is always the one that
        // reaches furthest, so every position in 0..4 is covered.
        let matches = [(0, 2), (1, 2), (2, 2)];
        for pos in 0..4 {
            assert!(match_covering(&matches, pos).is_some(), "pos {pos}");
        }
        assert_eq!(match_covering(&matches, 3), Some(2));
        assert_eq!(match_covering(&matches, 4), None);
    }

    // === mode line tests ===

    #[test]
    fn mode_line_padding_uses_display_width_not_bytes() {
        use unicode_width::UnicodeWidthStr;
        // "日本語.md" is 9 display columns but 12 bytes; byte-based padding
        // would overshoot and misalign the right side.
        let text = mode_line_text(" ** 日本語.md (1,0)  Top", "C-x ", 60);
        assert_eq!(text.width(), 60);
        assert!(text.ends_with("C-x "));
    }

    #[test]
    fn mode_line_padding_fills_exactly_with_ascii() {
        use unicode_width::UnicodeWidthStr;
        let text = mode_line_text(" -- test.txt (1,0)  All", " ", 40);
        assert_eq!(text.width(), 40);
    }

    #[test]
    fn mode_line_wider_than_area_gets_no_padding() {
        let text = mode_line_text("0123456789", "abc", 5);
        assert_eq!(text, "0123456789abc");
    }

    #[test]
    fn minibuffer_layout_handles_multibyte_label() {
        // Confirm-prompt labels embed buffer names, which can be non-ASCII.
        // "Save buffer 日本語.md? " is 12 + 6 + 3 + 2 = 23 display columns
        // (34 bytes) — the cursor must go after it, not 34 cells right.
        let editor = prompt_editor("Save buffer 日本語.md? ", "", 0);
        let layout = minibuffer_layout(&editor, 40, 2);
        assert_eq!(layout.cursor, Some((0, 23)));
    }

    #[test]
    fn minibuffer_layout_handles_combining_mark_in_input() {
        // "e" + combining acute renders as one column; point after both
        // chars is still column 1 past the label.
        let editor = prompt_editor("x", "e\u{301}", 2);
        let layout = minibuffer_layout(&editor, 10, 2);
        assert_eq!(layout.cursor, Some((0, 2)));
    }

    #[test]
    fn minibuffer_layout_escapes_terminal_controls_in_labels_and_input() {
        let editor = prompt_editor("x\u{1b}]0;title\u{7}: ", "a\u{85}b", 3);
        let layout = minibuffer_layout(&editor, 80, 2);

        assert_eq!(layout.visible_rows, ["x␛]0;title␇: a�b"]);
        assert!(!layout.visible_rows[0].chars().any(char::is_control));
    }

    #[test]
    fn minibuffer_layout_keeps_cursor_at_boundary_before_combining_input() {
        // Segmentation can join an input-leading combining mark to the last
        // label grapheme. Point zero must remain visible between them.
        let editor = prompt_editor("x", "\u{301}", 0);
        let layout = minibuffer_layout(&editor, 10, 2);
        assert_eq!(layout.cursor, Some((0, 1)));
    }

    fn prompt_editor(label: &str, input: &str, point: usize) -> Editor {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, label);
        editor.minibuffer_buffer.reset_transient_text(input);
        editor.minibuffer_pane.set_point(point);
        editor
    }

    #[test]
    fn minibuffer_layout_hard_wraps_and_places_cursor() {
        let editor = prompt_editor("I: ", "abcdefgh", 8);
        let layout = minibuffer_layout(&editor, 6, 3);

        assert_eq!(layout.visible_rows, ["I: abc", "defgh"]);
        assert_eq!(layout.height, 2);
        assert_eq!(layout.cursor, Some((1, 5)));
    }

    #[test]
    fn minibuffer_layout_adds_row_for_cursor_after_exact_fit() {
        let editor = prompt_editor("I: ", "abc", 3);
        let layout = minibuffer_layout(&editor, 6, 3);

        assert_eq!(layout.visible_rows, ["I: abc", ""]);
        assert_eq!(layout.height, 2);
        assert_eq!(layout.cursor, Some((1, 0)));
    }

    #[test]
    fn minibuffer_layout_uses_unicode_display_width() {
        let editor = prompt_editor("I: ", "你好x", 3);
        let layout = minibuffer_layout(&editor, 6, 3);

        assert_eq!(layout.visible_rows, ["I: 你", "好x"]);
        assert_eq!(layout.cursor, Some((1, 3)));
    }

    #[test]
    fn minibuffer_layout_keeps_capped_prompt_cursor_visible() {
        let editor = prompt_editor("", "abcdefghijklmnopqrst", 20);
        let layout = minibuffer_layout(&editor, 4, 3);

        assert_eq!(layout.visible_rows, ["mnop", "qrst", ""]);
        assert_eq!(layout.height, 3);
        assert_eq!(layout.cursor, Some((2, 0)));
    }

    #[test]
    fn screen_layout_grows_idle_messages_and_preserves_pane_space() {
        let mut editor = Editor::new();
        editor.minibuffer.show_message("abcdefghijkl".to_string());

        let layout = screen_layout(&editor, Rect::new(0, 0, 5, 9));

        assert_eq!(layout.pane_area, Rect::new(0, 0, 5, 6));
        assert_eq!(layout.completions_area, None);
        assert_eq!(layout.minibuffer_area, Rect::new(0, 6, 5, 3));
        assert_eq!(layout.minibuffer.visible_rows, ["abcde", "fghij", "kl"]);
    }

    #[test]
    fn screen_layout_keeps_completions_above_grown_minibuffer() {
        let mut editor = prompt_editor("I: ", "abcdefghijkl", 12);
        editor.minibuffer.completions = Some(vec!["alpha".into(), "alpine".into()]);

        let layout = screen_layout(&editor, Rect::new(0, 0, 8, 12));

        let completions = layout.completions_area.unwrap();
        assert_eq!(layout.pane_area.bottom(), completions.y);
        assert_eq!(completions.bottom(), layout.minibuffer_area.y);
        assert_eq!(layout.minibuffer_area.height, 2);
    }

    #[test]
    fn screen_layout_minibuffer_shrinks_with_content_or_wider_terminal() {
        let mut editor = prompt_editor("I: ", "abcdefgh", 8);
        assert_eq!(
            screen_layout(&editor, Rect::new(0, 0, 6, 9))
                .minibuffer_area
                .height,
            2
        );
        assert_eq!(
            screen_layout(&editor, Rect::new(0, 0, 20, 9))
                .minibuffer_area
                .height,
            1
        );

        editor.minibuffer_buffer.reset_transient_text("a");
        editor.minibuffer_pane.set_point(1);
        assert_eq!(
            screen_layout(&editor, Rect::new(0, 0, 6, 9))
                .minibuffer_area
                .height,
            1
        );
    }

    #[test]
    fn screen_layout_handles_zero_sized_terminal() {
        let editor = prompt_editor("I: ", "你好", 2);
        let layout = screen_layout(&editor, Rect::new(0, 0, 0, 0));

        assert_eq!(layout.pane_area, Rect::new(0, 0, 0, 0));
        assert_eq!(layout.completions_area, None);
        assert_eq!(layout.minibuffer_area, Rect::new(0, 0, 0, 0));
        assert!(layout.minibuffer.visible_rows.is_empty());
        assert_eq!(layout.minibuffer.cursor, None);
    }

    // === completions_layout tests ===

    #[test]
    fn completions_layout_single_column() {
        // max_len=10 => col_width=12, width=12 => 1 col, 3 rows
        let (cols, rows, cw) = completions_layout(3, 10, 12);
        assert_eq!(cols, 1);
        assert_eq!(rows, 3);
        assert_eq!(cw, 12);
    }

    // === Wide-character width math ===

    #[test]
    fn visual_width_counts_wide_chars_as_two_columns() {
        let chars: Vec<char> = "你好a".chars().collect();
        assert_eq!(visual_width_for_chars(&chars), 5);
    }

    #[test]
    fn visual_width_counts_combining_marks_as_zero() {
        let chars: Vec<char> = "ae\u{301}b".chars().collect();
        assert_eq!(visual_width_for_chars(&chars), 3);
    }

    #[test]
    fn visible_rows_materialize_only_the_viewport_of_a_huge_line() {
        let source = "x".repeat(5 * 1024 * 1024);
        let buf = Buffer::from_str(0, "huge", &source);
        let rows = visible_visual_rows(&buf, 0, 120, 0, 38);

        assert_eq!(rows.rows.len(), 38);
        assert!(!rows.exhausted);
        assert!(
            rows.rows.iter().map(|row| row.cells.len()).sum::<usize>() <= 120 * 38,
            "the renderer must not materialize the rest of the 5 MiB line"
        );
    }

    #[test]
    fn plain_ascii_layout_jumps_directly_to_a_deep_wrapped_row() {
        let source = "x".repeat(5 * 1024 * 1024);
        let buf = Buffer::from_str(0, "huge", &source);
        let layout = VisualLineLayout::new(&buf, 0, 120);
        let last_row = layout.row_count() - 1;
        let rows = layout.visible_rows(last_row, 1);

        assert!(layout.plain_ascii);
        assert_eq!(rows.rows.len(), 1);
        assert!(rows.exhausted);
        assert_eq!(rows.rows[0].cells[0].buffer_char_pos, last_row * (120 - 1));
    }

    #[test]
    fn plain_ascii_layout_maps_positions_without_scanning_prefix_cells() {
        let buf = Buffer::from_str(0, "plain", &"x".repeat(10_000));
        let layout = VisualLineLayout::new(&buf, 0, 80);
        assert_eq!(layout.row_col(7_900), (100, 0));
        assert_eq!(layout.buffer_col_for_visual_col(7_900), 7_900);
    }

    #[test]
    fn visible_rows_do_not_split_a_wide_glyph_at_the_wrap_marker() {
        // Width 6 gives five content cells on continued rows. The third CJK
        // glyph would straddle that boundary if wrapping sliced raw cells.
        let buf = Buffer::from_str(0, "wide", "你你你你");
        let rows = visible_visual_rows(&buf, 0, 6, 0, 2);

        assert_eq!(rows.rows.len(), 2);
        assert!(rows.rows[0].continues);
        assert_eq!(
            rows.rows[0]
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            "你你"
        );
        assert_eq!(
            rows.rows[1]
                .cells
                .iter()
                .map(|cell| cell.text.as_str())
                .collect::<String>(),
            "你你"
        );
        let layout = VisualLineLayout::new(&buf, 0, 6);
        assert_eq!(layout.row_col(2), (1, 0));
        assert_eq!(layout.row_col(3), (1, 2));
        assert_eq!(layout.buffer_col_at(1, 0), 2);
        assert_eq!(layout.buffer_col_at(1, 1), 3);
    }

    #[test]
    fn syntax_style_lookup_uses_half_open_span_boundaries() {
        let styled = Style::default().fg(Color::Red);
        let spans = vec![crate::syntax::StyledSpan {
            start: 2,
            end: 5,
            style: styled,
        }];

        assert_eq!(syntax_style_at_byte(&spans, 1), Style::default());
        assert_eq!(syntax_style_at_byte(&spans, 2), styled);
        assert_eq!(syntax_style_at_byte(&spans, 4), styled);
        assert_eq!(syntax_style_at_byte(&spans, 5), Style::default());
    }

    #[test]
    fn line_visual_width_crosses_rope_chunks() {
        let prefix = "a".repeat(2_000);
        let buf = Buffer::from_str(0, "test", &format!("{prefix}\t你e\u{301}\n"));
        assert!(buf.text().line(0).chunks().count() > 1);
        assert_eq!(line_visual_width(&buf, 0), 2_000 + 4 + 2 + 1);
    }

    #[test]
    fn visual_col_for_buffer_col_with_wide_chars() {
        let chars: Vec<char> = "你a".chars().collect();
        assert_eq!(visual_col_for_buffer_col(&chars, 0), 0);
        assert_eq!(visual_col_for_buffer_col(&chars, 1), 2);
        assert_eq!(visual_col_for_buffer_col(&chars, 2), 3);
    }

    #[test]
    fn buffer_col_for_visual_col_with_wide_chars() {
        let buf = Buffer::from_str(0, "test", "你a");
        assert_eq!(buffer_col_for_visual_col(&buf, 0, 0), 0);
        // Clicking the second cell of the wide char lands after it.
        assert_eq!(buffer_col_for_visual_col(&buf, 0, 1), 1);
        assert_eq!(buffer_col_for_visual_col(&buf, 0, 2), 1);
        assert_eq!(buffer_col_for_visual_col(&buf, 0, 3), 2);
    }

    #[test]
    fn line_chars_without_ending_strips_only_newline() {
        let buf = Buffer::from_str(0, "test", "ab\ncd");
        assert_eq!(line_chars_without_ending(&buf, 0), vec!['a', 'b']);
        assert_eq!(line_chars_without_ending(&buf, 1), vec!['c', 'd']);
        // Ex-break chars (lone CR, VT, FF, NEL, LS, PS) are content and
        // stay in the line's chars.
        for ch in ['\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}'] {
            let buf = Buffer::from_str(0, "test", &format!("ab{ch}cd\n"));
            assert_eq!(
                line_chars_without_ending(&buf, 0),
                vec!['a', 'b', ch, 'c', 'd'],
                "{ch:?}"
            );
        }
    }

    #[test]
    fn buffer_col_for_visual_col_counts_form_feed_as_content() {
        // "one\u{0c}two" is one line of seven chars; clicking beyond its
        // EOL clamps to the end of the line's text, after the FF.
        let buf = Buffer::from_str(0, "test", "one\u{0c}two\n");
        assert_eq!(buffer_col_for_visual_col(&buf, 0, 10), 7);
    }

    #[test]
    fn completions_layout_zero_width_does_not_divide_by_zero() {
        let (cols, rows, cw) = completions_layout(5, 10, 0);
        assert!(cols >= 1);
        assert_eq!(rows, 5);
        assert!(cw >= 1);
    }

    #[test]
    fn completions_layout_multi_column() {
        // max_len=8 => col_width=10, width=40 => 4 cols, ceil(10/4)=3 rows
        let (cols, rows, cw) = completions_layout(10, 8, 40);
        assert_eq!(cols, 4);
        assert_eq!(rows, 3);
        assert_eq!(cw, 10);
    }

    #[test]
    fn completions_layout_col_width_capped_at_terminal_width() {
        // max_len=100 => col_width=min(102,20)=20, width=20 => 1 col
        let (cols, rows, cw) = completions_layout(5, 100, 20);
        assert_eq!(cols, 1);
        assert_eq!(rows, 5);
        assert_eq!(cw, 20);
    }

    // === truncate_to_width tests ===

    #[test]
    fn truncate_to_width_drops_straddling_wide_char() {
        // The second wide char would occupy columns 3-4; it is dropped
        // entirely rather than split.
        assert_eq!(truncate_to_width("你好x", 3), ("你", 2));
    }

    #[test]
    fn truncate_to_width_keeps_exact_fit() {
        assert_eq!(truncate_to_width("你好x", 5), ("你好x", 5));
    }

    #[test]
    fn truncate_to_width_zero_budget() {
        assert_eq!(truncate_to_width("abc", 0), ("", 0));
    }

    #[test]
    fn truncate_to_width_keeps_combining_marks_with_base() {
        assert_eq!(truncate_to_width("e\u{301}x", 1), ("e\u{301}", 1));
    }

    // === completions_height tests ===

    #[test]
    fn completions_height_no_prompt() {
        let editor = Editor::new();
        assert_eq!(completions_height(&editor, 24, 80), 0);
    }

    #[test]
    fn completions_height_prompt_no_completions() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        assert_eq!(completions_height(&editor, 24, 80), 0);
    }

    #[test]
    fn completions_height_with_few_candidates() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        editor.minibuffer.completions = Some(vec!["a".into(), "b".into(), "c".into()]);
        // width=80, col_width=3, num_cols=26, num_rows=ceil(3/26)=1
        // max_rows=(24-2)/3=7, min(1,7)=1
        assert_eq!(completions_height(&editor, 24, 80), 1);
    }

    #[test]
    fn completions_height_narrow_terminal() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        editor.minibuffer.completions = Some(vec!["a".into(), "b".into(), "c".into()]);
        // width=1, col_width=1, num_cols=1, num_rows=3
        // max_rows=(24-2)/3=7, min(3,7)=3
        assert_eq!(completions_height(&editor, 24, 1), 3);
    }

    #[test]
    fn completions_height_capped() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        let many: Vec<String> = (0..50).map(|i| format!("file{}.txt", i)).collect();
        editor.minibuffer.completions = Some(many);
        // max_len=11 ("file10.txt"...), col_width=13, width=80 => 6 cols
        // num_rows=ceil(50/6)=9, capped at max_rows=(24-2)/3=7
        assert_eq!(completions_height(&editor, 24, 80), 7);
    }

    #[test]
    fn completions_height_measures_display_width_not_chars() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        // "你你你你a.txt" is 9 chars but 13 display columns.
        let candidates: Vec<String> = ["a", "b", "c", "d"]
            .iter()
            .map(|s| format!("你你你你{s}.txt"))
            .collect();
        editor.minibuffer.completions = Some(candidates);
        // col_width = 13+2 = 15; at width 24 only one column fits => 4 rows.
        // (Counting chars would give col_width 11, two columns => 2 rows.)
        assert_eq!(completions_height(&editor, 24, 24), 4);
    }

    #[test]
    fn completions_height_small_terminal() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        editor.minibuffer.completions = Some(vec!["a".into(), "b".into()]);
        // height=4, max_rows = (4-2)/3 = 0 -> max(1) = 1
        assert_eq!(completions_height(&editor, 4, 80), 1);
    }

    /// Regression test: when the viewport starts inside a fenced code block,
    /// syntax highlighting should still be correct. Lines after the closing ```
    /// should be highlighted as normal markdown (e.g., headings should be bold+blue),
    /// not as code.
    #[test]
    fn markdown_highlight_consistent_when_scrolled_into_code_block() {
        use crate::syntax::Language;
        use ratatui::style::Modifier;

        // Build a markdown document with a code block in the middle.
        // Lines 0-3: normal markdown
        // Lines 4-8: fenced code block (``` ... ```)
        // Lines 9-11: normal markdown including a heading
        let markdown = "\
# Title\n\
\n\
Some text\n\
\n\
```\n\
fn main() {\n\
    let x = 42;\n\
}\n\
```\n\
\n\
# After Code Block\n\
\n\
Some *emphasis* here.\n";

        let mut buf = Buffer::from_str(0, "test.md", markdown);
        buf.enable_syntax(Language::Markdown);

        let syntax = buf.syntax().unwrap();
        // Case 1: scroll_top=0, see everything.
        let styles_full = syntax.highlight_rope(
            buf.text().slice(..),
            0..buf.text().len_bytes(),
            buf.edit_generation(),
        );

        // Case 2: viewport starts inside the code block.
        let scrolled_start = buf.text().line_to_byte(6);
        let styles_scrolled = syntax.highlight_rope(
            buf.text().slice(..),
            scrolled_start..buf.text().len_bytes(),
            buf.edit_generation(),
        );

        // Line 10 is "# After Code Block" — the '#' and heading text should have
        // the heading style (bold + blue) regardless of scroll position.
        let heading_line = 10;
        let heading_start = buf.text().line_to_byte(heading_line);
        let heading_end = buf.text().line_to_byte(heading_line + 1);

        // With full context (scroll_top=0), the heading should be styled
        let has_heading_style_full = styles_full.iter().any(|span| {
            span.end > heading_start
                && span.start < heading_end
                && span.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(
            has_heading_style_full,
            "Heading on line {heading_line} should be bold when fully visible"
        );

        // With scrolled context (scroll_top=6, inside code block), the heading
        // should STILL be styled the same way.
        let has_heading_style_scrolled = styles_scrolled.iter().any(|span| {
            span.end > heading_start
                && span.start < heading_end
                && span.style.add_modifier.contains(Modifier::BOLD)
        });
        assert!(
            has_heading_style_scrolled,
            "Heading on line {heading_line} should be bold even when viewport starts inside code block"
        );
    }

    #[test]
    fn completions_height_multicolumn_reduces_rows() {
        let mut editor = Editor::new();
        editor
            .minibuffer
            .start_prompt(crate::minibuffer::PromptKind::FindFile, "Find file: ");
        // 30 short candidates in 80-wide terminal
        let candidates: Vec<String> = (0..30).map(|i| format!("f{}", i)).collect();
        editor.minibuffer.completions = Some(candidates);
        // max_len=3 ("f10"...), col_width=5, width=80 => 16 cols
        // num_rows=ceil(30/16)=2, max_rows=(24-2)/3=7, min(2,7)=2
        let h = completions_height(&editor, 24, 80);
        assert!(h < 30, "should be much less than candidate count");
        assert_eq!(h, 2);
    }
}
