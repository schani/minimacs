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
    query: String,
    /// Contiguous snapshot made once when search starts. Isearch owns input
    /// until it finishes, so the searched buffer cannot change underneath it.
    text_snapshot: String,
    direction: SearchDirection,
    /// Position before search started (to restore on C-g).
    original_point: usize,
    original_scroll_top: usize,
    original_scroll_row_offset: usize,
    /// Current match position (char offset of match start).
    current_match: Option<usize>,
    /// Char positions of all matches, recomputed once per query change.
    /// Navigation and rendering read this instead of rescanning the buffer.
    matches: Vec<usize>,
    /// Whether the last search action failed to find a match. Shown in the
    /// prompt label ("Failing I-search: "), like emacs.
    failing: bool,
}

impl ISearchState {
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn text_snapshot(&self) -> &str {
        &self.text_snapshot
    }

    pub fn direction(&self) -> SearchDirection {
        self.direction
    }

    pub fn original_view(&self) -> (usize, usize, usize) {
        (
            self.original_point,
            self.original_scroll_top,
            self.original_scroll_row_offset,
        )
    }

    pub fn current_match(&self) -> Option<usize> {
        self.current_match
    }

    pub fn matches(&self) -> &[usize] {
        &self.matches
    }

    pub fn is_failing(&self) -> bool {
        self.failing
    }

    #[cfg(test)]
    pub(crate) fn set_query_for_test(&mut self, query: &str) {
        self.query = query.to_string();
    }
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
        let original_point = pane.point();
        let original_scroll_top = pane.scroll_top();
        let original_scroll_row_offset = pane.scroll_row_offset();
        let text_snapshot = self.current_buffer().text().to_string();
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
        let label = isearch_label(isearch.direction(), isearch.is_failing());
        self.minibuffer.set_prompt_label(label);
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
        self.pane_tree.set_focused_point(char_pos);
        if let Some(ref mut isearch) = self.isearch {
            isearch.current_match = Some(char_pos);
        }
        self.ensure_cursor_visible();
    }

    /// Called when isearch input changes — rescan the buffer once, cache all
    /// match positions, and jump to the first match from the original point.
    pub fn isearch_update(&mut self) {
        let (query, direction, original_point) = match &self.isearch {
            Some(s) => (s.query().to_string(), s.direction(), s.original_point),
            None => return,
        };
        if query.is_empty() {
            // Restore to original position
            if let Some(ref isearch) = self.isearch {
                self.pane_tree.restore_focused_view(
                    isearch.original_point,
                    isearch.original_scroll_top,
                    isearch.original_scroll_row_offset,
                );
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
            Self::compute_matches_for_query(self.isearch.as_ref().unwrap().text_snapshot(), &query);
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

    /// Replace isearch input and synchronize every derived representation:
    /// query, minibuffer display/point, cached matches, selected match, and
    /// live prompt label. Typed input, paste, and backspace all use this one
    /// transition so their state cannot drift apart.
    fn isearch_set_query(&mut self, query: String) {
        let Some(isearch) = self.isearch.as_mut() else {
            return;
        };
        isearch.query = query;
        self.minibuffer_buffer.reset_transient_text(&isearch.query);
        self.minibuffer_pane
            .set_point(isearch.query.chars().count());
        self.isearch_update();
    }

    /// Append one typed character to the isearch query.
    pub fn isearch_input_char(&mut self, ch: char) {
        let Some(isearch) = &self.isearch else {
            return;
        };
        let mut query = isearch.query.clone();
        query.push(ch);
        self.isearch_set_query(query);
    }

    /// Append pasted text to the isearch query (emacs `isearch-yank`).
    /// The query is a single line, so line breaks in the pasted text become
    /// spaces (the standard minibuffer paste normalization).
    pub fn isearch_yank(&mut self, text: &str) {
        let Some(isearch) = &self.isearch else {
            return;
        };
        let mut query = isearch.query.clone();
        query.push_str(&self.normalized_paste(text));
        self.isearch_set_query(query);
    }

    /// Remove one user-perceived character from the query.
    pub fn isearch_backspace(&mut self) {
        let Some(isearch) = &self.isearch else {
            return;
        };
        let mut query = isearch.query.clone();
        if let Some((start, _)) = query.grapheme_indices(true).next_back() {
            query.truncate(start);
        }
        self.isearch_set_query(query);
    }

    /// Set the search direction and cycle to the next match. Direction,
    /// failure status, and the live prompt label are updated atomically from
    /// App's perspective.
    pub fn isearch_cycle(&mut self, direction: SearchDirection) {
        let Some(isearch) = self.isearch.as_mut() else {
            return;
        };
        isearch.direction = direction;
        self.isearch_next();
    }

    /// Cycle to next/previous match during isearch, using the cached matches.
    pub fn isearch_next(&mut self) {
        let found = match &self.isearch {
            Some(s) if !s.query.is_empty() => {
                let current_point = self.pane_tree.focused_pane().point();
                let query_len = s.query().chars().count();
                match s.direction() {
                    SearchDirection::Forward => {
                        s.matches().iter().copied().find(|&p| p > current_point)
                    }
                    SearchDirection::Backward => s
                        .matches()
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
            .matches()
            .iter()
            .map(|&pos| (pos, query_char_len))
            .collect()
    }
}
