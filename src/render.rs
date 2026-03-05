use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::buffer::Buffer;
use crate::editor::Editor;
use crate::pane::Pane;

/// Compute the multi-column layout for completions.
///
/// Returns `(num_cols, num_rows, col_width)`.
pub fn completions_layout(num_candidates: usize, max_candidate_len: usize, width: usize) -> (usize, usize, usize) {
    let col_width = (max_candidate_len + 2).max(1).min(width);
    let num_cols = (width / col_width).max(1);
    let num_rows = num_candidates.div_ceil(num_cols);
    (num_cols, num_rows, col_width)
}

/// Compute the height of the completions area.
pub fn completions_height(editor: &Editor, total_height: u16, total_width: u16) -> u16 {
    if !editor.minibuffer.is_active() {
        return 0;
    }
    match &editor.minibuffer.completions {
        Some(candidates) if !candidates.is_empty() => {
            let max_rows = ((total_height.saturating_sub(2)) / 3).max(1) as usize;
            let max_len = candidates.iter().map(|c| c.len()).max().unwrap_or(0);
            let (_num_cols, num_rows, _col_width) = completions_layout(candidates.len(), max_len, total_width as usize);
            num_rows.min(max_rows) as u16
        }
        _ => 0,
    }
}

/// Render the entire editor UI into the given frame.
pub fn render(frame: &mut Frame, editor: &Editor) {
    let area = frame.area();

    let comp_height = completions_height(editor, area.height, area.width);
    let (pane_area, completions_area, minibuffer_area) = if comp_height > 0 {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(comp_height),
                Constraint::Length(1),
            ])
            .split(area);
        (chunks[0], Some(chunks[1]), chunks[2])
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);
        (chunks[0], None, chunks[1])
    };

    // Calculate rects for all panes
    let pane_rects = editor.pane_tree.calculate_rects(pane_area);
    let focus_path = editor.pane_tree.focus_path();

    for (path, rect) in &pane_rects {
        let pane = editor.pane_tree.pane_at_focus_path(path);
        let buf = editor.buffer_by_id(pane.buffer_id);
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
            pane.mark.map(|mark| {
                let start = pane.point.min(mark);
                let end = pane.point.max(mark);
                (start, end)
            })
        };

        // Get search matches for the focused pane
        let search_matches = if is_focused {
            editor.isearch_matches()
        } else {
            Vec::new()
        };
        let current_match = if is_focused {
            editor.isearch.as_ref().and_then(|s| s.current_match)
        } else {
            None
        };

        render_pane_text(frame, buf, pane, region, &search_matches, current_match, text_area);
        render_pane_mode_line(frame, editor, buf, pane, is_focused, mode_line_area);

        // Set cursor position for the focused pane
        if is_focused && !editor.minibuffer.is_active() {
            let (cursor_line, cursor_col) = buf.char_to_line_col(pane.point);
            let text_width = text_area.width as usize;

            // Compute visual row from scroll_top, accounting for wrapping
            let mut visual_row: usize = 0;
            for lidx in pane.scroll_top..cursor_line {
                let line_len = buf.line_len_chars(lidx);
                visual_row += visual_lines_for_length(line_len, text_width);
            }

            // Add offset within cursor's line if it wraps
            let line_len = buf.line_len_chars(cursor_line);
            let (row_in_line, col_in_segment) = if text_width > 1 && line_len > text_width {
                let cps = text_width - 1;
                (cursor_col / cps, cursor_col % cps)
            } else {
                (0, cursor_col)
            };
            visual_row += row_in_line;

            let screen_line = visual_row as u16;
            let screen_col = col_in_segment as u16;

            if screen_col < text_area.x + text_area.width && screen_line < text_area.height {
                frame.set_cursor_position((text_area.x + screen_col, text_area.y + screen_line));
            }
        }
    }

    if let Some(comp_area) = completions_area {
        if let Some(candidates) = &editor.minibuffer.completions {
            render_completions(frame, candidates, editor.minibuffer.completion_page, comp_area);
        }
    }

    render_minibuffer(frame, editor, minibuffer_area);
}

/// Compute how many visual rows a buffer line occupies with wrapping.
pub fn visual_lines_for_length(line_char_len: usize, text_width: usize) -> usize {
    if text_width <= 1 || line_char_len <= text_width {
        return 1;
    }
    let chars_per_segment = text_width - 1; // one column reserved for '\'
    // First N-1 segments hold chars_per_segment chars each; last segment holds up to text_width.
    // N = 1 + ceil((line_char_len - text_width) / chars_per_segment)
    let excess = line_char_len - text_width;
    1 + excess.div_ceil(chars_per_segment)
}

fn render_pane_text(
    frame: &mut Frame,
    buf: &Buffer,
    pane: &Pane,
    region: Option<(usize, usize)>,
    search_matches: &[(usize, usize)],
    current_match: Option<usize>,
    area: Rect,
) {
    let scroll_top = pane.scroll_top;
    let max_visual_rows = area.height as usize;
    let total_lines = buf.line_count();
    let text_width = area.width as usize;

    // Compute per-character syntax styles for visible buffer lines
    let syntax_styles = compute_syntax_char_styles(buf, scroll_top, max_visual_rows);

    let mut output_lines: Vec<Line> = Vec::new();
    let mut line_idx = scroll_top;

    while output_lines.len() < max_visual_rows && line_idx < total_lines {
        let line_text: String = buf.text.line(line_idx).chars().collect();
        let line_text = line_text.trim_end_matches('\n').trim_end_matches('\r');
        let line_start_char = buf.text.line_to_char(line_idx);
        let line_chars: Vec<char> = line_text.chars().collect();

        if text_width == 0 {
            output_lines.push(Line::from(Span::raw(String::new())));
            line_idx += 1;
            continue;
        }

        if line_chars.len() <= text_width {
            // Line fits in one visual row — no wrapping needed
            let mut spans = Vec::new();

            if !line_chars.is_empty() {
                build_styled_spans(
                    &mut spans,
                    &line_chars,
                    0,
                    line_chars.len(),
                    line_idx,
                    line_start_char,
                    region,
                    search_matches,
                    current_match,
                    &syntax_styles,
                );
            }

            output_lines.push(Line::from(spans));
        } else {
            // Line needs wrapping
            let chars_per_segment = (text_width - 1).max(1);
            let mut offset = 0;

            while offset < line_chars.len() && output_lines.len() < max_visual_rows {
                let remaining = line_chars.len() - offset;
                let is_last = remaining <= text_width;
                let segment_len = if is_last { remaining } else { chars_per_segment };

                let mut spans = Vec::new();

                build_styled_spans(
                    &mut spans,
                    &line_chars,
                    offset,
                    offset + segment_len,
                    line_idx,
                    line_start_char,
                    region,
                    search_matches,
                    current_match,
                    &syntax_styles,
                );

                if !is_last {
                    spans.push(Span::styled(
                        "\\",
                        Style::default().fg(Color::Rgb(35, 120, 147)),
                    ));
                }

                output_lines.push(Line::from(spans));
                offset += segment_len;
            }
        }

        line_idx += 1;
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

/// Build styled spans for a segment of a buffer line (chars[start..end]).
#[allow(clippy::too_many_arguments)]
fn build_styled_spans(
    spans: &mut Vec<Span<'static>>,
    line_chars: &[char],
    start: usize,
    end: usize,
    line_idx: usize,
    line_start_char: usize,
    region: Option<(usize, usize)>,
    search_matches: &[(usize, usize)],
    current_match: Option<usize>,
    syntax_styles: &Option<std::collections::HashMap<(usize, usize), Style>>,
) {
    let segment = &line_chars[start..end];
    if segment.is_empty() {
        return;
    }

    let char_styles: Vec<Style> = segment
        .iter()
        .enumerate()
        .map(|(j, _)| {
            let col = start + j; // column within the buffer line
            let char_pos = line_start_char + col; // global char position

            let in_region = region
                .map(|(rs, re)| char_pos >= rs && char_pos < re)
                .unwrap_or(false);

            if in_region {
                Style::default().bg(Color::Rgb(173, 214, 255))
            } else {
                let is_current_match = current_match.is_some_and(|cm| {
                    search_matches
                        .iter()
                        .any(|(pos, len)| *pos == cm && char_pos >= *pos && char_pos < *pos + *len)
                });
                let is_other_match = search_matches
                    .iter()
                    .any(|(pos, len)| char_pos >= *pos && char_pos < *pos + *len);

                if is_current_match {
                    Style::default().bg(Color::Rgb(168, 172, 148)).fg(Color::Black)
                } else if is_other_match {
                    Style::default().bg(Color::Rgb(248, 201, 171)).fg(Color::Black)
                } else if let Some(ref syn) = syntax_styles {
                    syn.get(&(line_idx, col)).copied().unwrap_or_default()
                } else {
                    Style::default()
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
        let text: String = segment[run_start..run_end].iter().collect();
        spans.push(Span::styled(text, style));
        run_start = run_end;
    }
}

/// Compute per-character syntax styles for visible lines.
/// Returns a map from (line_idx, col) -> Style, or None if no syntax.
fn compute_syntax_char_styles(
    buf: &Buffer,
    scroll_top: usize,
    visible_lines: usize,
) -> Option<std::collections::HashMap<(usize, usize), Style>> {
    let syntax = buf.syntax.as_ref()?;
    let total_lines = buf.line_count();
    let first_line = scroll_top;
    let last_line = (scroll_top + visible_lines).min(total_lines);
    if first_line >= last_line {
        return None;
    }

    // Collect visible text as bytes
    let first_byte = buf.text.line_to_byte(first_line);
    let last_byte = if last_line < total_lines {
        buf.text.line_to_byte(last_line)
    } else {
        buf.text.len_bytes()
    };

    let mut visible_bytes = Vec::with_capacity(last_byte - first_byte);
    for chunk in buf.text.byte_slice(first_byte..last_byte).chunks() {
        visible_bytes.extend_from_slice(chunk.as_bytes());
    }

    let styled_spans = syntax.highlight(&visible_bytes);

    // Build a byte-to-style lookup, then map to (line, col) via char positions
    let mut byte_styles = vec![Style::default(); visible_bytes.len()];
    for ss in &styled_spans {
        for item in byte_styles
            .iter_mut()
            .take(ss.end.min(visible_bytes.len()))
            .skip(ss.start)
        {
            *item = ss.style;
        }
    }

    // Map each visible line's chars to styles
    let mut result = std::collections::HashMap::new();

    for line_idx in first_line..last_line {
        let line_byte_start = buf.text.line_to_byte(line_idx) - first_byte;
        let line_text: String = buf.text.line(line_idx).chars().collect();
        let line_text = line_text.trim_end_matches('\n').trim_end_matches('\r');

        let mut byte_offset = line_byte_start;
        for (col, ch) in line_text.chars().enumerate() {
            let style = byte_styles.get(byte_offset).copied().unwrap_or_default();
            if style != Style::default() {
                result.insert((line_idx, col), style);
            }
            byte_offset += ch.len_utf8();
        }
    }

    Some(result)
}

fn render_pane_mode_line(
    frame: &mut Frame,
    editor: &Editor,
    buf: &Buffer,
    pane: &Pane,
    is_focused: bool,
    area: Rect,
) {
    let (line, col) = buf.char_to_line_col(pane.point);

    let modified_indicator = if buf.modified { "**" } else { "--" };
    let name = &buf.name;

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

    let pending = &editor.pending_keys;
    let pending_display = if is_focused && !pending.is_empty() {
        format!("  {}", pending)
    } else {
        String::new()
    };

    let left = format!(
        " {} {} ({},{})  {}",
        modified_indicator,
        name,
        line + 1,
        col,
        position
    );
    let right = format!("{} ", pending_display);

    // Pad to fill the line
    let total_width = area.width as usize;
    let left_len = left.len();
    let right_len = right.len();
    let padding = if total_width > left_len + right_len {
        " ".repeat(total_width - left_len - right_len)
    } else {
        String::new()
    };

    let mode_line_text = format!("{}{}{}", left, padding, right);

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

    let max_len = candidates.iter().map(|c| c.len()).max().unwrap_or(0);
    let (num_cols, _num_rows, col_width) = completions_layout(candidates.len(), max_len, width);

    // How many candidates can we display per page?
    let displayable = rows * num_cols;
    let page_count = candidates.len().div_ceil(displayable).max(1);
    let page = page % page_count;
    let start = page * displayable;

    let bg = Style::default().bg(Color::Rgb(243, 243, 243)).fg(Color::Black);

    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for row in 0..rows {
        let mut text = String::new();
        for col in 0..num_cols {
            let idx = start + col * rows + row;
            if idx < candidates.len() && idx < start + displayable {
                let name = &candidates[idx];
                if text.len() + name.len() <= width {
                    text.push_str(name);
                    // Pad to column width
                    let padding = col_width.saturating_sub(name.len());
                    let remaining_width = width.saturating_sub(text.len());
                    let pad = padding.min(remaining_width);
                    text.extend(std::iter::repeat_n(' ', pad));
                } else {
                    // Truncate to fit
                    let remaining = width.saturating_sub(text.len());
                    if remaining > 0 {
                        text.push_str(&name[..remaining.min(name.len())]);
                    }
                }
            }
        }
        // Pad remaining width with spaces for background fill
        if text.len() < width {
            text.extend(std::iter::repeat_n(' ', width - text.len()));
        }

        // If this is the last row and there are multiple pages, show page indicator
        if row == rows - 1 && page_count > 1 {
            let suffix = format!("[Page {}/{}]", page + 1, page_count);
            if suffix.len() <= width {
                let start = width - suffix.len();
                text.replace_range(start..width, &suffix);
            }
        }

        lines.push(Line::from(Span::styled(text, bg)));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn render_minibuffer(frame: &mut Frame, editor: &Editor, area: Rect) {
    let text = if editor.minibuffer.is_active() {
        let label = &editor.minibuffer.prompt().unwrap().label;
        let input: String = editor.minibuffer_buffer.text.to_string();
        format!("{}{}", label, input)
    } else {
        editor.minibuffer.message.as_deref().unwrap_or("").to_string()
    };
    let minibuffer = Paragraph::new(Line::from(text));
    frame.render_widget(minibuffer, area);

    // If minibuffer has a prompt, set cursor there
    if let Some(prompt) = editor.minibuffer.prompt() {
        let cursor_pos = prompt.label.len() + editor.minibuffer_pane.point;
        let x = area.x + cursor_pos as u16;
        if x < area.x + area.width {
            frame.set_cursor_position((x, area.y));
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

    // === completions_layout tests ===

    #[test]
    fn completions_layout_single_column() {
        // max_len=10 => col_width=12, width=12 => 1 col, 3 rows
        let (cols, rows, cw) = completions_layout(3, 10, 12);
        assert_eq!(cols, 1);
        assert_eq!(rows, 3);
        assert_eq!(cw, 12);
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

    // === completions_height tests ===

    #[test]
    fn completions_height_no_prompt() {
        let editor = Editor::new();
        assert_eq!(completions_height(&editor, 24, 80), 0);
    }

    #[test]
    fn completions_height_prompt_no_completions() {
        let mut editor = Editor::new();
        editor.minibuffer.start_prompt(
            crate::minibuffer::PromptKind::FindFile,
            "Find file: ",
        );
        assert_eq!(completions_height(&editor, 24, 80), 0);
    }

    #[test]
    fn completions_height_with_few_candidates() {
        let mut editor = Editor::new();
        editor.minibuffer.start_prompt(
            crate::minibuffer::PromptKind::FindFile,
            "Find file: ",
        );
        editor.minibuffer.completions = Some(vec!["a".into(), "b".into(), "c".into()]);
        // width=80, col_width=3, num_cols=26, num_rows=ceil(3/26)=1
        // max_rows=(24-2)/3=7, min(1,7)=1
        assert_eq!(completions_height(&editor, 24, 80), 1);
    }

    #[test]
    fn completions_height_narrow_terminal() {
        let mut editor = Editor::new();
        editor.minibuffer.start_prompt(
            crate::minibuffer::PromptKind::FindFile,
            "Find file: ",
        );
        editor.minibuffer.completions = Some(vec!["a".into(), "b".into(), "c".into()]);
        // width=1, col_width=1, num_cols=1, num_rows=3
        // max_rows=(24-2)/3=7, min(3,7)=3
        assert_eq!(completions_height(&editor, 24, 1), 3);
    }

    #[test]
    fn completions_height_capped() {
        let mut editor = Editor::new();
        editor.minibuffer.start_prompt(
            crate::minibuffer::PromptKind::FindFile,
            "Find file: ",
        );
        let many: Vec<String> = (0..50).map(|i| format!("file{}.txt", i)).collect();
        editor.minibuffer.completions = Some(many);
        // max_len=11 ("file10.txt"...), col_width=13, width=80 => 6 cols
        // num_rows=ceil(50/6)=9, capped at max_rows=(24-2)/3=7
        assert_eq!(completions_height(&editor, 24, 80), 7);
    }

    #[test]
    fn completions_height_small_terminal() {
        let mut editor = Editor::new();
        editor.minibuffer.start_prompt(
            crate::minibuffer::PromptKind::FindFile,
            "Find file: ",
        );
        editor.minibuffer.completions = Some(vec!["a".into(), "b".into()]);
        // height=4, max_rows = (4-2)/3 = 0 -> max(1) = 1
        assert_eq!(completions_height(&editor, 4, 80), 1);
    }

    #[test]
    fn completions_height_multicolumn_reduces_rows() {
        let mut editor = Editor::new();
        editor.minibuffer.start_prompt(
            crate::minibuffer::PromptKind::FindFile,
            "Find file: ",
        );
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
