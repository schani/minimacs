use unicode_segmentation::UnicodeSegmentation;

use crate::minibuffer::PromptKind;

use super::Editor;

/// Direction of incremental search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchDirection {
    Forward,
    Backward,
}

/// State for incremental search.
#[derive(Debug)]
pub struct ISearchState {
    pub query: String,
    /// Contiguous snapshot made once when search starts. Isearch owns input
    /// until it finishes, so the searched buffer cannot change underneath it.
    pub text_snapshot: String,
    pub direction: SearchDirection,
    /// Position before search started (to restore on C-g).
    pub original_point: usize,
    pub original_scroll_top: usize,
    pub original_scroll_row_offset: usize,
    /// Current match position (char offset of match start).
    pub current_match: Option<usize>,
    /// Char positions of all matches, recomputed once per query change.
    /// Navigation and rendering read this instead of rescanning the buffer.
    pub matches: Vec<usize>,
    /// Whether the last search action failed to find a match. Shown in the
    /// prompt label ("Failing I-search: "), like emacs.
    pub failing: bool,
}

/// The isearch prompt label for the given state — the label is live: it
/// tracks direction changes and failing searches while the prompt is up.
fn isearch_label(direction: SearchDirection, failing: bool) -> &'static str {
    match (direction, failing) {
        (SearchDirection::Forward, false) => "I-search: ",
        (SearchDirection::Forward, true) => "Failing I-search: ",
        (SearchDirection::Backward, false) => "I-search backward: ",
        (SearchDirection::Backward, true) => "Failing I-search backward: ",
    }
}

impl Editor {
    // === Incremental Search ===

    pub(super) fn isearch_start(&mut self, direction: SearchDirection) {
        if self.minibuffer.is_active() {
            return;
        }
        let pane = self.pane_tree.focused_pane();
        let original_point = pane.point;
        let original_scroll_top = pane.scroll_top;
        let original_scroll_row_offset = pane.scroll_row_offset;
        let text_snapshot = self.current_buffer().text.to_string();
        self.isearch = Some(ISearchState {
            query: String::new(),
            text_snapshot,
            direction,
            original_point,
            original_scroll_top,
            original_scroll_row_offset,
            current_match: None,
            matches: Vec::new(),
            failing: false,
        });
        self.start_minibuffer_prompt(PromptKind::ISearch, isearch_label(direction, false));
    }

    /// Sync the prompt label with the current search state. Called after
    /// every state change (query edit, cycling, direction flip) so the
    /// label always shows the current direction and failing status.
    fn isearch_sync_label(&mut self) {
        let Some(isearch) = &self.isearch else {
            return;
        };
        let label = isearch_label(isearch.direction, isearch.failing);
        if let Some(prompt) = self.minibuffer.prompt_mut() {
            prompt.label = label.to_string();
        }
    }

    /// Find the char positions of all occurrences of `query` in the search
    /// snapshot. One O(buffer) scan; called only when the query changes.
    fn compute_matches_for_query(text: &str, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return Vec::new();
        }
        let mut matches = Vec::new();
        let mut search_start = 0usize; // byte offset
        let mut char_offset = 0usize;
        while let Some(byte_pos) = text[search_start..].find(query) {
            let match_char =
                char_offset + text[search_start..search_start + byte_pos].chars().count();
            matches.push(match_char);
            // Advance past this match start by one char (overlaps allowed).
            let next_byte = search_start
                + byte_pos
                + text[search_start + byte_pos..]
                    .chars()
                    .next()
                    .map_or(1, |c| c.len_utf8());
            char_offset = match_char + 1;
            search_start = next_byte;
        }
        matches
    }

    /// Move point to an isearch match and scroll it into view.
    fn isearch_goto_match(&mut self, char_pos: usize) {
        self.pane_tree.focused_pane_mut().point = char_pos;
        let pane = self.pane_tree.focused_pane();
        let scroll_top = pane.scroll_top;
        let scroll_row_offset = pane.scroll_row_offset;
        let vh = pane.viewport_height;
        let vw = pane.viewport_width;
        let buf = self.current_buffer();
        let (line, col) = buf.char_to_line_col(char_pos);
        let (cursor_row, _) = crate::display::visual_row_col_in_line(buf, line, col, vw);
        let (new_top, new_offset) = crate::pane::compute_scroll_position(
            scroll_top,
            scroll_row_offset,
            line,
            cursor_row,
            vh,
            vw,
            |l| crate::display::line_visual_width(buf, l),
        );
        let pane = self.pane_tree.focused_pane_mut();
        pane.scroll_top = new_top;
        pane.scroll_row_offset = new_offset;
        if let Some(ref mut isearch) = self.isearch {
            isearch.current_match = Some(char_pos);
        }
    }

    /// Called when isearch input changes — rescan the buffer once, cache all
    /// match positions, and jump to the first match from the original point.
    pub fn isearch_update(&mut self) {
        let (query, direction, original_point) = match &self.isearch {
            Some(s) => (s.query.clone(), s.direction, s.original_point),
            None => return,
        };
        if query.is_empty() {
            // Restore to original position
            if let Some(ref isearch) = self.isearch {
                let pane = self.pane_tree.focused_pane_mut();
                pane.point = isearch.original_point;
                pane.scroll_top = isearch.original_scroll_top;
                pane.scroll_row_offset = isearch.original_scroll_row_offset;
            }
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = None;
                isearch.matches.clear();
                isearch.failing = false;
            }
            self.isearch_sync_label();
            return;
        }

        let matches =
            Self::compute_matches_for_query(&self.isearch.as_ref().unwrap().text_snapshot, &query);
        let query_len = query.chars().count();
        let found = match direction {
            SearchDirection::Forward => matches.iter().copied().find(|&p| p >= original_point),
            SearchDirection::Backward => matches
                .iter()
                .copied()
                .rev()
                .find(|&p| p + query_len <= original_point),
        };
        if let Some(ref mut isearch) = self.isearch {
            isearch.matches = matches;
            isearch.failing = found.is_none();
        }

        if let Some(char_pos) = found {
            self.isearch_goto_match(char_pos);
        } else if let Some(ref mut isearch) = self.isearch {
            isearch.current_match = None;
        }
        self.isearch_sync_label();
    }

    /// Append pasted text to the isearch query (emacs `isearch-yank`).
    /// The query is a single line, so line breaks in the pasted text become
    /// spaces (the standard minibuffer paste normalization); the minibuffer
    /// display is synced to the query and the search re-runs.
    pub fn isearch_yank(&mut self, text: &str) {
        if self.isearch.is_none() {
            return;
        }
        let text = self.normalized_paste(text);
        if let Some(ref mut isearch) = self.isearch {
            isearch.query.push_str(&text);
            let query = isearch.query.clone();
            self.minibuffer_buffer.text = ropey::Rope::from_str(&query);
            self.minibuffer_pane.point = query.chars().count();
        }
        self.isearch_update();
    }

    /// Remove one user-perceived character from the query and keep the
    /// minibuffer mirror synchronized.
    pub fn isearch_backspace(&mut self) {
        if let Some(ref mut isearch) = self.isearch {
            if let Some((start, _)) = isearch.query.grapheme_indices(true).next_back() {
                isearch.query.truncate(start);
            }
            let query = isearch.query.clone();
            self.minibuffer_buffer.text = ropey::Rope::from_str(&query);
            self.minibuffer_pane.point = query.chars().count();
        }
        self.isearch_update();
    }

    /// Cycle to next/previous match during isearch, using the cached matches.
    pub fn isearch_next(&mut self) {
        let found = match &self.isearch {
            Some(s) if !s.query.is_empty() => {
                let current_point = self.pane_tree.focused_pane().point;
                let query_len = s.query.chars().count();
                match s.direction {
                    SearchDirection::Forward => {
                        s.matches.iter().copied().find(|&p| p > current_point)
                    }
                    SearchDirection::Backward => s
                        .matches
                        .iter()
                        .copied()
                        .rev()
                        .find(|&p| p + query_len <= current_point),
                }
            }
            Some(_) => {
                // Empty query, but C-s/C-r may have just flipped the
                // direction — keep the label's direction current.
                self.isearch_sync_label();
                return;
            }
            None => return,
        };

        if let Some(ref mut isearch) = self.isearch {
            isearch.failing = found.is_none();
        }
        if let Some(char_pos) = found {
            self.isearch_goto_match(char_pos);
        }
        self.isearch_sync_label();
    }

    /// Accept the current isearch position.
    pub fn isearch_accept(&mut self) {
        self.isearch = None;
        self.minibuffer.finish();
        // Isearch keys bypass `execute()`, so a stale `last_command` from
        // before the search would survive it — a kill before C-s must not
        // chain with a kill right after accepting.
        self.clear_last_command();
    }

    /// All match positions for rendering (char offset, query char length).
    /// Reads the cache built by `isearch_update` — no buffer scan per frame.
    pub fn isearch_matches(&self) -> Vec<(usize, usize)> {
        let isearch = match &self.isearch {
            Some(s) if !s.query.is_empty() => s,
            _ => return Vec::new(),
        };
        let query_char_len = isearch.query.chars().count();
        isearch
            .matches
            .iter()
            .map(|&pos| (pos, query_char_len))
            .collect()
    }
}
