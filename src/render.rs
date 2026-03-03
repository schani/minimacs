use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::buffer::Buffer;
use crate::editor::Editor;
use crate::pane::Pane;

/// Render the entire editor UI into the given frame.
pub fn render(frame: &mut Frame, editor: &Editor) {
    let area = frame.area();

    // Layout: pane area + minibuffer (1 row)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // pane area
            Constraint::Length(1), // minibuffer
        ])
        .split(area);

    let pane_area = chunks[0];
    let minibuffer_area = chunks[1];

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
            let gw = gutter_width(buf.line_count());

            let screen_line = cursor_line.saturating_sub(pane.scroll_top) as u16;
            let screen_col = gw as u16 + cursor_col as u16;

            if screen_col < text_area.x + text_area.width && screen_line < text_area.height {
                frame.set_cursor_position((text_area.x + screen_col, text_area.y + screen_line));
            }
        }
    }

    render_minibuffer(frame, editor, minibuffer_area);
}

fn gutter_width(line_count: usize) -> usize {
    let digits = if line_count == 0 {
        1
    } else {
        (line_count as f64).log10().floor() as usize + 1
    };
    digits + 1 // +1 for separator space
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
    let visible_lines = area.height as usize;
    let total_lines = buf.line_count();
    let gw = gutter_width(total_lines);

    // Compute per-character syntax styles for visible lines (if syntax available)
    let syntax_styles = compute_syntax_char_styles(buf, scroll_top, visible_lines);

    let mut lines = Vec::new();
    for i in 0..visible_lines {
        let line_idx = scroll_top + i;
        if line_idx < total_lines {
            let line_num = format!("{:>width$} ", line_idx + 1, width = gw - 1);
            let line_text: String = buf.text.line(line_idx).chars().collect();
            let line_text = line_text.trim_end_matches('\n').trim_end_matches('\r');

            let mut spans = vec![Span::styled(
                line_num,
                Style::default().fg(Color::DarkGray),
            )];

            let line_start_char = buf.text.line_to_char(line_idx);
            let line_chars: Vec<char> = line_text.chars().collect();

            if line_chars.is_empty() {
                spans.push(Span::raw(String::new()));
            } else {
                // Build per-character style array
                let char_styles: Vec<Style> = line_chars.iter().enumerate().map(|(col, _)| {
                    let char_pos = line_start_char + col;

                    // Priority: region > current match > other matches > syntax > default
                    let in_region = region
                        .map(|(rs, re)| char_pos >= rs && char_pos < re)
                        .unwrap_or(false);

                    if in_region {
                        Style::default().bg(Color::White).fg(Color::Black)
                    } else {
                        // Check if in a search match
                        let is_current_match = current_match.is_some_and(|cm| {
                            search_matches.iter().any(|(pos, len)| {
                                *pos == cm && char_pos >= *pos && char_pos < *pos + *len
                            })
                        });
                        let is_other_match = search_matches.iter().any(|(pos, len)| {
                            char_pos >= *pos && char_pos < *pos + *len
                        });

                        if is_current_match {
                            // Current match: bright yellow bg
                            Style::default().bg(Color::Yellow).fg(Color::Black)
                        } else if is_other_match {
                            // Other matches: dim highlight
                            Style::default().bg(Color::Indexed(58)).fg(Color::White)
                        } else if let Some(ref syn) = syntax_styles {
                            syn.get(&(line_idx, col)).copied().unwrap_or_default()
                        } else {
                            Style::default()
                        }
                    }
                }).collect();

                // RLE: merge consecutive chars with same style into spans
                let mut run_start = 0;
                while run_start < line_chars.len() {
                    let style = char_styles[run_start];
                    let mut run_end = run_start + 1;
                    while run_end < line_chars.len() && char_styles[run_end] == style {
                        run_end += 1;
                    }
                    let text: String = line_chars[run_start..run_end].iter().collect();
                    spans.push(Span::styled(text, style));
                    run_start = run_end;
                }
            }

            lines.push(Line::from(spans));
        } else {
            let padding = " ".repeat(gw);
            lines.push(Line::from(Span::styled(
                format!("{}~", padding),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
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
    // First, build a byte-offset -> style array
    let mut byte_styles = vec![Style::default(); visible_bytes.len()];
    for ss in &styled_spans {
        for item in byte_styles.iter_mut().take(ss.end.min(visible_bytes.len())).skip(ss.start) {
            *item = ss.style;
        }
    }

    // Now map each visible line's chars to styles
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
            .bg(Color::White)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
    };

    let mode_line = Paragraph::new(Line::from(Span::styled(mode_line_text, style)));
    frame.render_widget(mode_line, area);
}

fn render_minibuffer(frame: &mut Frame, editor: &Editor, area: Rect) {
    let text = editor.minibuffer.display_text();
    let minibuffer = Paragraph::new(Line::from(text));
    frame.render_widget(minibuffer, area);

    // If minibuffer has a prompt, set cursor there instead
    if let Some(cursor_pos) = editor.minibuffer.cursor_position() {
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
    fn gutter_width_for_small_files() {
        assert_eq!(gutter_width(1), 2);   // "1 "
        assert_eq!(gutter_width(9), 2);   // "9 "
        assert_eq!(gutter_width(10), 3);  // "10 "
        assert_eq!(gutter_width(99), 3);  // "99 "
        assert_eq!(gutter_width(100), 4); // "100 "
    }
}
