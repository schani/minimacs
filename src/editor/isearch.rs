use crate::buffer::Buffer;
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
    pub direction: SearchDirection,
    /// Position before search started (to restore on C-g).
    pub original_point: usize,
    pub original_scroll_top: usize,
    /// Current match position (char offset of match start).
    pub current_match: Option<usize>,
    /// Char positions of all matches, recomputed once per query change.
    /// Navigation and rendering read this instead of rescanning the buffer.
    pub matches: Vec<usize>,
}

impl Editor {
    // === Incremental Search ===

    pub(super) fn isearch_start(&mut self, direction: SearchDirection) {
        if self.minibuffer.is_active() {
            return;
        }
        let pane = self.pane_tree.focused_pane();
        self.isearch = Some(ISearchState {
            query: String::new(),
            direction,
            original_point: pane.point,
            original_scroll_top: pane.scroll_top,
            current_match: None,
            matches: Vec::new(),
        });
        let label = match direction {
            SearchDirection::Forward => "I-search: ",
            SearchDirection::Backward => "I-search backward: ",
        };
        self.start_minibuffer_prompt(PromptKind::ISearch, label);
    }

    /// Find the char positions of all occurrences of `query` in the buffer.
    /// One O(buffer) scan; called only when the search query changes.
    fn compute_matches_for_query(buf: &Buffer, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return Vec::new();
        }
        let text: String = buf.text.chars().collect();
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
        let vh = pane.viewport_height;
        let vw = pane.viewport_width;
        let buf = self.current_buffer();
        let (line, _) = buf.char_to_line_col(char_pos);
        let new_top = crate::pane::compute_scroll_top(scroll_top, line, vh, vw, |l| {
            crate::render::line_visual_width(buf, l)
        });
        self.pane_tree.focused_pane_mut().scroll_top = new_top;
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
            }
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = None;
                isearch.matches.clear();
            }
            return;
        }

        let matches = Self::compute_matches_for_query(self.current_buffer(), &query);
        let query_len = query.chars().count();
        let found = match direction {
            SearchDirection::Forward => {
                matches.iter().copied().find(|&p| p >= original_point)
            }
            SearchDirection::Backward => matches
                .iter()
                .copied()
                .rev()
                .find(|&p| p + query_len <= original_point),
        };
        if let Some(ref mut isearch) = self.isearch {
            isearch.matches = matches;
        }

        if let Some(char_pos) = found {
            self.isearch_goto_match(char_pos);
        } else {
            self.minibuffer.show_message("Failing I-search".to_string());
            if let Some(ref mut isearch) = self.isearch {
                isearch.current_match = None;
            }
        }
    }

    /// Cycle to next/previous match during isearch, using the cached matches.
    pub fn isearch_next(&mut self) {
        let (query, found) = match &self.isearch {
            Some(s) if !s.query.is_empty() => {
                let current_point = self.pane_tree.focused_pane().point;
                let query_len = s.query.chars().count();
                let found = match s.direction {
                    SearchDirection::Forward => {
                        s.matches.iter().copied().find(|&p| p > current_point)
                    }
                    SearchDirection::Backward => s
                        .matches
                        .iter()
                        .copied()
                        .rev()
                        .find(|&p| p + query_len <= current_point),
                };
                (s.query.clone(), found)
            }
            _ => return,
        };

        if let Some(char_pos) = found {
            self.isearch_goto_match(char_pos);
        } else {
            self.minibuffer
                .show_message(format!("Failing I-search: {query}"));
        }
    }

    /// Accept the current isearch position.
    pub fn isearch_accept(&mut self) {
        self.isearch = None;
        self.minibuffer.finish();
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
