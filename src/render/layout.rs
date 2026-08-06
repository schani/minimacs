use ratatui::layout::Rect;

use crate::editor::Editor;

use crate::display::{char_width, terminal_safe_text};

pub fn completions_layout(
    num_candidates: usize,
    max_candidate_len: usize,
    width: usize,
) -> (usize, usize, usize) {
    let col_width = (max_candidate_len + 2).max(1).min(width.max(1));
    let num_cols = (width / col_width).max(1);
    let num_rows = num_candidates.div_ceil(num_cols);
    (num_cols, num_rows, col_width)
}

/// Longest prefix of `s` that fits in `max_width` display columns, and that
/// prefix's width. A wide char that would straddle the budget is dropped
/// entirely — never render half a glyph.
pub(super) fn truncate_to_width(s: &str, max_width: usize) -> (&str, usize) {
    let mut width = 0;
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        let w = char_width(ch);
        if width + w > max_width {
            break;
        }
        width += w;
        end = i + ch.len_utf8();
    }
    (&s[..end], width)
}

/// Compute the height of the completions area.
pub fn completions_height(editor: &Editor, total_height: u16, total_width: u16) -> u16 {
    if !editor.minibuffer().is_active() {
        return 0;
    }
    match editor.minibuffer().completions() {
        Some(candidates) if !candidates.is_empty() => {
            use unicode_width::UnicodeWidthStr;
            let max_rows = ((total_height.saturating_sub(2)) / 3).max(1) as usize;
            let max_len = candidates
                .iter()
                .map(|candidate| terminal_safe_text(candidate).width())
                .max()
                .unwrap_or(0);
            let (_num_cols, num_rows, _col_width) =
                completions_layout(candidates.len(), max_len, total_width as usize);
            num_rows.min(max_rows) as u16
        }
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MinibufferLayout {
    pub(super) visible_rows: Vec<String>,
    pub(super) height: u16,
    /// Cursor coordinates relative to the visible minibuffer area.
    pub(super) cursor: Option<(usize, usize)>,
}

pub(crate) struct ScreenLayout {
    pub(crate) pane_area: Rect,
    pub(crate) completions_area: Option<Rect>,
    pub(crate) minibuffer_area: Rect,
    pub(super) minibuffer: MinibufferLayout,
}

fn minibuffer_content(editor: &Editor) -> (String, Option<usize>) {
    let Some(prompt) = editor.minibuffer().prompt() else {
        return (
            terminal_safe_text(editor.minibuffer().message().unwrap_or("")),
            None,
        );
    };

    let input = terminal_safe_text(&editor.minibuffer_buffer().text().to_string());
    let label = terminal_safe_text(prompt.label());
    let point_byte = input
        .char_indices()
        .nth(editor.minibuffer_pane().point())
        .map_or(input.len(), |(byte, _)| byte);
    let mut text = String::with_capacity(label.len() + input.len());
    text.push_str(&label);
    text.push_str(&input);
    (text, Some(label.len() + point_byte))
}

/// Hard-wrap minibuffer content into terminal rows and locate the prompt
/// cursor. This is the single authority for minibuffer height, rendering,
/// and cursor geometry. Wrapping is grapheme-safe and measured in display
/// columns, so wide and combining characters agree with terminal rendering.
pub(super) fn minibuffer_layout(editor: &Editor, width: u16, max_height: u16) -> MinibufferLayout {
    use unicode_segmentation::UnicodeSegmentation;
    use unicode_width::UnicodeWidthStr;

    if width == 0 || max_height == 0 {
        return MinibufferLayout {
            visible_rows: Vec::new(),
            height: 0,
            cursor: None,
        };
    }

    let (text, cursor_byte) = minibuffer_content(editor);
    let width = width as usize;
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_width = 0;
    let mut cursor = None;

    for (byte, grapheme) in text.grapheme_indices(true) {
        if grapheme == "\n" {
            if cursor_byte == Some(byte) {
                cursor = Some(if row_width >= width {
                    (rows.len() + 1, 0)
                } else {
                    (rows.len(), row_width)
                });
            }
            rows.push(std::mem::take(&mut row));
            row_width = 0;
            continue;
        }

        let grapheme_width = grapheme.width();
        if row_width > 0 && grapheme_width > 0 && row_width + grapheme_width > width {
            rows.push(std::mem::take(&mut row));
            row_width = 0;
        }
        let cursor_in_grapheme = cursor_byte
            .filter(|cursor_byte| byte <= *cursor_byte && *cursor_byte < byte + grapheme.len());
        if let Some(cursor_byte) = cursor_in_grapheme {
            let prefix_width = grapheme[..cursor_byte - byte].width();
            cursor = Some((rows.len(), row_width + prefix_width));
        }
        row.push_str(grapheme);
        row_width += grapheme_width;
    }

    if cursor_byte == Some(text.len()) && cursor.is_none() {
        cursor = Some(if row_width >= width {
            (rows.len() + 1, 0)
        } else {
            (rows.len(), row_width)
        });
    }
    rows.push(row);
    if let Some((cursor_row, _)) = cursor {
        while rows.len() <= cursor_row {
            rows.push(String::new());
        }
    }

    let height = rows.len().min(max_height as usize);
    let max_first = rows.len().saturating_sub(height);
    let first_visible_row = cursor
        .map(|(cursor_row, _)| cursor_row.saturating_add(1).saturating_sub(height))
        .unwrap_or(0)
        .min(max_first);
    let cursor = cursor.and_then(|(row, col)| {
        row.checked_sub(first_visible_row)
            .filter(|row| *row < height)
            .map(|row| (row, col))
    });
    let visible_rows = rows
        .into_iter()
        .skip(first_visible_row)
        .take(height)
        .collect();

    MinibufferLayout {
        visible_rows,
        height: height as u16,
        cursor,
    }
}

/// Divide the frame into editor panes, optional completions, and a dynamic
/// minibuffer. The minibuffer may occupy at most one third of the frame;
/// longer prompts scroll around their cursor instead of consuming the editor.
pub(crate) fn screen_layout(editor: &Editor, area: Rect) -> ScreenLayout {
    let comp_height =
        completions_height(editor, area.height, area.width).min(area.height.saturating_sub(1));
    let remaining = area.height.saturating_sub(comp_height);
    let max_minibuffer_height = if remaining == 0 {
        0
    } else {
        (area.height / 3).max(1).min(remaining)
    };
    let minibuffer = minibuffer_layout(editor, area.width, max_minibuffer_height);
    let pane_height = remaining.saturating_sub(minibuffer.height);
    let pane_area = Rect::new(area.x, area.y, area.width, pane_height);
    let completions_area = (comp_height > 0).then(|| {
        Rect::new(
            area.x,
            area.y.saturating_add(pane_height),
            area.width,
            comp_height,
        )
    });
    let minibuffer_area = Rect::new(
        area.x,
        area.y
            .saturating_add(pane_height)
            .saturating_add(comp_height),
        area.width,
        minibuffer.height,
    );

    ScreenLayout {
        pane_area,
        completions_area,
        minibuffer_area,
        minibuffer,
    }
}
