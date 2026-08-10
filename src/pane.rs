use std::collections::HashMap;

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::buffer::BufferId;

/// Compute how many visual rows a buffer line occupies with wrapping.
pub fn visual_lines_for_length(line_char_len: usize, text_width: usize) -> usize {
    if text_width <= 1 || line_char_len <= text_width {
        return 1;
    }
    // One column is reserved for '\\' on continued visual lines.
    // First N-1 segments hold chars_per_segment chars each; the last segment
    // holds up to text_width chars.
    // N = 1 + ceil((line_char_len - text_width) / chars_per_segment)
    let chars_per_segment = text_width - 1;
    let excess = line_char_len - text_width;
    1 + excess.div_ceil(chars_per_segment)
}

/// Compute the scroll position — `(scroll_top, scroll_row_offset)`, where the
/// offset is the number of visual rows of the `scroll_top` line scrolled off
/// above the viewport — needed so that the cursor's visual row is visible
/// within a viewport of `viewport_height` visual rows and `viewport_width`
/// columns, accounting for line wrapping. `cursor_row_in_line` is the visual
/// row of the cursor within its own (possibly wrapped) line, so the cursor
/// can be brought into view even inside a single line taller than the
/// viewport.
pub fn compute_scroll_position(
    scroll_top: usize,
    scroll_row_offset: usize,
    cursor_line: usize,
    cursor_row_in_line: usize,
    viewport_height: usize,
    viewport_width: usize,
    line_len: impl Fn(usize) -> usize,
) -> (usize, usize) {
    // Cursor above the first visible visual row: make its row the top.
    if cursor_line < scroll_top
        || (cursor_line == scroll_top && cursor_row_in_line < scroll_row_offset)
    {
        return (cursor_line, cursor_row_in_line);
    }

    // Clamp a stale offset (e.g. after a resize or edit changed how the top
    // line wraps) to the top line's actual visual height.
    let top_rows = visual_lines_for_length(line_len(scroll_top), viewport_width);
    let scroll_row_offset = scroll_row_offset.min(top_rows - 1);

    let viewport_height = viewport_height.max(1);

    // Count visual rows from the first visible row through the cursor's
    // visual row (inclusive).
    let mut visual_rows: usize = cursor_row_in_line + 1;
    for line in scroll_top..cursor_line {
        visual_rows += visual_lines_for_length(line_len(line), viewport_width);
    }
    visual_rows -= scroll_row_offset;

    // If the cursor's row is within the viewport, nothing to do.
    if visual_rows <= viewport_height {
        return (scroll_top, scroll_row_offset);
    }

    // Scroll down by the excess so the cursor's visual row becomes the last
    // viewport row.
    let excess = visual_rows - viewport_height;
    let mut new_top = scroll_top;
    let mut new_offset = scroll_row_offset + excess;
    while new_top < cursor_line {
        let rows = visual_lines_for_length(line_len(new_top), viewport_width);
        if new_offset < rows {
            break;
        }
        new_offset -= rows;
        new_top += 1;
    }
    if new_top == cursor_line {
        new_offset = new_offset.min(cursor_row_in_line);
    }
    (new_top, new_offset)
}

/// Advance a `(scroll_top, scroll_row_offset)` scroll position down by `n`
/// visual rows, stopping at the last visual row of the last buffer line.
/// A stale offset is clamped to the top line's actual visual height first.
pub fn scroll_down_visual_rows(
    scroll_top: usize,
    scroll_row_offset: usize,
    n: usize,
    total_lines: usize,
    viewport_width: usize,
    line_len: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let mut top = scroll_top.min(total_lines.saturating_sub(1));
    let mut rows = visual_lines_for_length(line_len(top), viewport_width);
    let mut offset = scroll_row_offset.min(rows - 1);
    for _ in 0..n {
        if offset + 1 < rows {
            offset += 1;
        } else if top + 1 < total_lines {
            top += 1;
            offset = 0;
            rows = visual_lines_for_length(line_len(top), viewport_width);
        } else {
            break;
        }
    }
    (top, offset)
}

/// Move a `(scroll_top, scroll_row_offset)` scroll position up by `n` visual
/// rows, stopping at the first visual row of the first buffer line.
pub fn scroll_up_visual_rows(
    scroll_top: usize,
    scroll_row_offset: usize,
    n: usize,
    total_lines: usize,
    viewport_width: usize,
    line_len: impl Fn(usize) -> usize,
) -> (usize, usize) {
    let mut top = scroll_top.min(total_lines.saturating_sub(1));
    let mut offset =
        scroll_row_offset.min(visual_lines_for_length(line_len(top), viewport_width) - 1);
    for _ in 0..n {
        if offset > 0 {
            offset -= 1;
        } else if top > 0 {
            top -= 1;
            offset = visual_lines_for_length(line_len(top), viewport_width) - 1;
        } else {
            break;
        }
    }
    (top, offset)
}

/// Map a position through an edit that replaced `removed` units at `start`
/// with `inserted` units. Positions at or before the edit stay put (emacs
/// marker semantics); positions inside the removed span are kept within the
/// new text; positions after shift by the length delta. Used both for char
/// positions (point, mark) and, with line units, for `scroll_top`.
fn adjust_position(p: usize, start: usize, removed: usize, inserted: usize) -> usize {
    if p <= start {
        p
    } else if p >= start + removed {
        p - removed + inserted
    } else {
        p.min(start + inserted)
    }
}

/// Describes a buffer edit for position adjustment: the replaced char span
/// plus its line-level effect, so panes can adjust char positions (point,
/// mark) and line positions (`scroll_top`) through the same edit.
#[derive(Debug, Clone, Copy)]
pub struct EditDelta {
    /// Char index where the edit starts.
    pub start: usize,
    /// Chars removed at `start`.
    pub removed: usize,
    /// Chars inserted at `start`.
    pub inserted: usize,
    /// Line containing `start` (the same before and after the edit).
    pub first_line: usize,
    /// Line breaks removed by the edit.
    pub removed_lines: usize,
    /// Line breaks inserted by the edit.
    pub inserted_lines: usize,
}

#[cfg(test)]
impl EditDelta {
    /// An edit that adds or removes no line breaks.
    pub fn same_line(start: usize, removed: usize, inserted: usize) -> Self {
        Self {
            start,
            removed,
            inserted,
            first_line: 0,
            removed_lines: 0,
            inserted_lines: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct BufferViewState {
    point: usize,
    mark: Option<usize>,
    preferred_column: Option<usize>,
    scroll_top: usize,
    scroll_row_offset: usize,
}

/// A single pane (window) viewing a buffer.
pub struct Pane {
    buffer_id: BufferId,
    point: usize,
    mark: Option<usize>,
    preferred_column: Option<usize>,
    scroll_top: usize,
    /// Visual rows of the `scroll_top` line scrolled off above the viewport;
    /// nonzero only when the top line wraps taller than the space above the
    /// cursor. Consumers clamp it to the top line's current visual height
    /// (it can go stale when a resize or edit changes how that line wraps).
    scroll_row_offset: usize,
    viewport_height: usize,
    viewport_width: usize,
    last_buffer_id: Option<BufferId>,
    buffer_states: HashMap<BufferId, BufferViewState>,
}

impl Pane {
    pub fn buffer_id(&self) -> BufferId {
        self.buffer_id
    }

    pub fn point(&self) -> usize {
        self.point
    }

    pub fn mark(&self) -> Option<usize> {
        self.mark
    }

    pub fn preferred_column(&self) -> Option<usize> {
        self.preferred_column
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    pub fn scroll_row_offset(&self) -> usize {
        self.scroll_row_offset
    }

    pub fn viewport_height(&self) -> usize {
        self.viewport_height
    }

    pub fn viewport_width(&self) -> usize {
        self.viewport_width
    }

    pub(crate) fn set_point(&mut self, point: usize) {
        self.point = point;
    }

    pub(crate) fn set_mark(&mut self, mark: Option<usize>) {
        self.mark = mark;
    }

    pub(crate) fn set_preferred_column(&mut self, preferred_column: Option<usize>) {
        self.preferred_column = preferred_column;
    }

    pub(crate) fn set_scroll_position(&mut self, top: usize, row_offset: usize) {
        self.scroll_top = top;
        self.scroll_row_offset = row_offset;
    }

    pub(crate) fn set_viewport(&mut self, height: usize, width: usize) {
        self.viewport_height = height;
        self.viewport_width = width;
    }

    pub(crate) fn set_point_mark_and_preferred(
        &mut self,
        point: usize,
        mark: Option<usize>,
        preferred_column: Option<usize>,
    ) {
        self.point = point;
        self.mark = mark;
        self.preferred_column = preferred_column;
    }

    pub fn new(buffer_id: BufferId) -> Self {
        Self {
            buffer_id,
            point: 0,
            mark: None,
            preferred_column: None,
            scroll_top: 0,
            scroll_row_offset: 0,
            viewport_height: 24,
            viewport_width: 80,
            last_buffer_id: None,
            buffer_states: HashMap::new(),
        }
    }

    fn current_buffer_state(&self) -> BufferViewState {
        BufferViewState {
            point: self.point,
            mark: self.mark,
            preferred_column: self.preferred_column,
            scroll_top: self.scroll_top,
            scroll_row_offset: self.scroll_row_offset,
        }
    }

    pub fn save_current_buffer_state(&mut self) {
        self.buffer_states
            .insert(self.buffer_id, self.current_buffer_state());
    }

    pub fn switch_buffer(&mut self, buffer_id: BufferId, buffer_len: usize) {
        if buffer_id == self.buffer_id {
            return;
        }

        let current_buffer_id = self.buffer_id;
        self.save_current_buffer_state();
        self.last_buffer_id = Some(current_buffer_id);
        self.restore_buffer_state(buffer_id, buffer_len);
    }

    pub fn restore_buffer_state(&mut self, buffer_id: BufferId, buffer_len: usize) {
        self.buffer_id = buffer_id;
        let state = self
            .buffer_states
            .get(&buffer_id)
            .copied()
            .unwrap_or_default();
        self.point = state.point.min(buffer_len);
        self.mark = state.mark.map(|mark| mark.min(buffer_len));
        self.preferred_column = state.preferred_column;
        self.scroll_top = state.scroll_top;
        self.scroll_row_offset = state.scroll_row_offset;
    }

    /// Adjust point, mark, scroll position, and saved view state for an edit
    /// to `buffer_id`. Keeps positions in other panes valid when a shared
    /// buffer is edited; `scroll_top` shifts with the edit's line delta so
    /// the pane keeps showing the same content. `scroll_row_offset` is left
    /// as-is — a pane has no access to line lengths, so an edit that changes
    /// how the top line wraps can leave it stale; every consumer clamps it
    /// to the top line's current visual height before use.
    pub fn adjust_for_edit(&mut self, buffer_id: BufferId, delta: EditDelta) {
        let EditDelta {
            start,
            removed,
            inserted,
            first_line,
            removed_lines,
            inserted_lines,
        } = delta;
        if self.buffer_id == buffer_id {
            self.point = adjust_position(self.point, start, removed, inserted);
            self.mark = self
                .mark
                .map(|m| adjust_position(m, start, removed, inserted));
            self.scroll_top =
                adjust_position(self.scroll_top, first_line, removed_lines, inserted_lines);
        }
        if let Some(state) = self.buffer_states.get_mut(&buffer_id) {
            state.point = adjust_position(state.point, start, removed, inserted);
            state.mark = state
                .mark
                .map(|m| adjust_position(m, start, removed, inserted));
            state.scroll_top =
                adjust_position(state.scroll_top, first_line, removed_lines, inserted_lines);
        }
    }

    pub fn alternate_buffer_id(&self) -> Option<BufferId> {
        self.last_buffer_id
            .filter(|buffer_id| *buffer_id != self.buffer_id)
    }

    pub fn forget_buffer(&mut self, buffer_id: BufferId) {
        self.buffer_states.remove(&buffer_id);
        if self.last_buffer_id == Some(buffer_id) {
            self.last_buffer_id = None;
        }
    }

    /// Adjust the scroll position so that visual row `cursor_row_in_line` of
    /// `cursor_line` is visible within the viewport, accounting for line
    /// wrapping. `line_len` returns the character count for a given buffer
    /// line index.
    #[cfg(test)]
    pub fn ensure_visible(
        &mut self,
        cursor_line: usize,
        cursor_row_in_line: usize,
        line_len: impl Fn(usize) -> usize,
    ) {
        let (top, offset) = compute_scroll_position(
            self.scroll_top,
            self.scroll_row_offset,
            cursor_line,
            cursor_row_in_line,
            self.viewport_height,
            self.viewport_width,
            line_len,
        );
        self.scroll_top = top;
        self.scroll_row_offset = offset;
    }
}

/// A node in the pane tree.
pub enum PaneNode {
    Leaf(Pane),
    Split {
        direction: Direction,
        children: Vec<PaneNode>,
    },
}

/// The pane tree manages all panes.
pub struct PaneTree {
    root: PaneNode,
    /// Index path from root to focused leaf.
    focus_path: Vec<usize>,
}

impl PaneTree {
    pub fn new(buffer_id: BufferId) -> Self {
        Self {
            root: PaneNode::Leaf(Pane::new(buffer_id)),
            focus_path: vec![],
        }
    }

    /// Get the focused pane.
    pub fn focused_pane(&self) -> &Pane {
        self.pane_at_path(&self.focus_path)
    }

    /// Get the focused pane mutably inside the pane ownership module.
    fn focused_pane_mut(&mut self) -> &mut Pane {
        let path = self.focus_path.clone();
        self.pane_at_path_mut(&path)
    }

    /// Get the focus path (for comparing to determine if a pane is focused).
    pub fn focus_path(&self) -> &[usize] {
        &self.focus_path
    }

    /// Set the focus path directly (e.g., for mouse clicks).
    pub fn set_focus_path(&mut self, path: Vec<usize>) {
        self.focus_path = path;
    }

    /// Get pane at a specific path (for rendering).
    pub fn pane_at_focus_path(&self, path: &[usize]) -> &Pane {
        self.pane_at_path(path)
    }

    pub(crate) fn set_focused_point(&mut self, point: usize) {
        self.focused_pane_mut().set_point(point);
    }

    pub(crate) fn set_focused_mark(&mut self, mark: Option<usize>) {
        self.focused_pane_mut().set_mark(mark);
    }

    pub(crate) fn set_focused_preferred_column(&mut self, column: Option<usize>) {
        self.focused_pane_mut().set_preferred_column(column);
    }

    pub(crate) fn set_focused_point_and_preferred(&mut self, point: usize, column: Option<usize>) {
        let pane = self.focused_pane_mut();
        pane.set_point(point);
        pane.set_preferred_column(column);
    }

    pub(crate) fn set_focused_point_mark_and_preferred(
        &mut self,
        point: usize,
        mark: Option<usize>,
        column: Option<usize>,
    ) {
        self.focused_pane_mut()
            .set_point_mark_and_preferred(point, mark, column);
    }

    pub(crate) fn set_focused_scroll_position(&mut self, top: usize, row_offset: usize) {
        self.focused_pane_mut().set_scroll_position(top, row_offset);
    }

    #[cfg(test)]
    pub(crate) fn set_focused_scroll_top(&mut self, top: usize) {
        let offset = self.focused_pane().scroll_row_offset();
        self.focused_pane_mut().set_scroll_position(top, offset);
    }

    #[cfg(test)]
    pub(crate) fn set_focused_viewport_height(&mut self, height: usize) {
        let width = self.focused_pane().viewport_width();
        self.focused_pane_mut().set_viewport(height, width);
    }

    #[cfg(test)]
    pub(crate) fn set_focused_viewport_width(&mut self, width: usize) {
        let height = self.focused_pane().viewport_height();
        self.focused_pane_mut().set_viewport(height, width);
    }

    pub(crate) fn restore_focused_view(&mut self, point: usize, top: usize, row_offset: usize) {
        let pane = self.focused_pane_mut();
        pane.set_point(point);
        pane.set_scroll_position(top, row_offset);
    }

    pub(crate) fn switch_focused_buffer(&mut self, buffer_id: BufferId, buffer_len: usize) {
        self.focused_pane_mut().switch_buffer(buffer_id, buffer_len);
    }

    pub(crate) fn set_pane_scroll_position(&mut self, path: &[usize], top: usize, offset: usize) {
        self.pane_at_path_mut(path).set_scroll_position(top, offset);
    }

    pub(crate) fn adjust_for_edit(&mut self, buffer_id: BufferId, delta: EditDelta) {
        Self::visit_panes_mut(&mut self.root, &mut |pane| {
            pane.adjust_for_edit(buffer_id, delta);
        });
    }

    /// Remove every reference to a killed buffer. Each pane displaying it
    /// returns to that pane's own surviving alternate; `fallback_id` is used
    /// only when the pane has no such alternate. Buffer lengths are supplied
    /// by the editor so restored point and mark can be clamped correctly.
    pub(crate) fn replace_killed_buffer(
        &mut self,
        buffer_id: BufferId,
        fallback_id: BufferId,
        surviving_buffer_lengths: &HashMap<BufferId, usize>,
    ) {
        Self::visit_panes_mut(&mut self.root, &mut |pane| {
            let replacement_id = pane
                .alternate_buffer_id()
                .filter(|id| surviving_buffer_lengths.contains_key(id))
                .unwrap_or(fallback_id);
            let replacement_len = surviving_buffer_lengths[&replacement_id];

            pane.forget_buffer(buffer_id);
            if pane.buffer_id() == buffer_id {
                pane.restore_buffer_state(replacement_id, replacement_len);
            }
        });
    }

    fn pane_at_path(&self, path: &[usize]) -> &Pane {
        let mut node = &self.root;
        for &idx in path {
            match node {
                PaneNode::Split { children, .. } => {
                    node = &children[idx];
                }
                PaneNode::Leaf(_) => panic!("path too long"),
            }
        }
        match node {
            PaneNode::Leaf(pane) => pane,
            PaneNode::Split { .. } => panic!("path doesn't reach a leaf"),
        }
    }

    fn pane_at_path_mut(&mut self, path: &[usize]) -> &mut Pane {
        let mut node = &mut self.root;
        for &idx in path {
            match node {
                PaneNode::Split { children, .. } => {
                    node = &mut children[idx];
                }
                PaneNode::Leaf(_) => panic!("path too long"),
            }
        }
        match node {
            PaneNode::Leaf(pane) => pane,
            PaneNode::Split { .. } => panic!("path doesn't reach a leaf"),
        }
    }

    /// Split the focused pane in the given direction.
    pub fn split(&mut self, direction: Direction, new_buffer_id: BufferId) {
        let path = self.focus_path.clone();
        let node = self.node_at_path_mut(&path);

        // Replace the leaf with a split containing two leaves
        let old = std::mem::replace(node, PaneNode::Leaf(Pane::new(0))); // placeholder
        let new_pane = PaneNode::Leaf(Pane::new(new_buffer_id));

        *node = PaneNode::Split {
            direction,
            children: vec![old, new_pane],
        };

        // Focus stays on the first child (original pane)
        self.focus_path.push(0);
    }

    /// Delete the focused pane. Returns false if it's the only pane.
    pub fn delete_focused(&mut self) -> bool {
        if self.focus_path.is_empty() {
            // Only one pane — can't delete
            return false;
        }

        let parent_path = self.focus_path[..self.focus_path.len() - 1].to_vec();
        let child_idx = *self.focus_path.last().unwrap();

        let parent = self.node_at_path_mut(&parent_path);
        match parent {
            PaneNode::Split { children, .. } => {
                children.remove(child_idx);
                if children.len() == 1 {
                    // Collapse: replace parent split with the single remaining child
                    let remaining = children.remove(0);
                    *parent = remaining;
                    self.focus_path = parent_path;
                    // Navigate to first leaf
                    self.navigate_to_first_leaf();
                } else {
                    // Focus the next available child
                    let new_idx = child_idx.min(children.len() - 1);
                    self.focus_path = parent_path;
                    self.focus_path.push(new_idx);
                    self.navigate_to_first_leaf_from_current();
                }
            }
            PaneNode::Leaf(_) => return false,
        }

        true
    }

    /// Delete all panes except the focused one.
    pub fn delete_others(&mut self) {
        let pane_data = {
            let pane = self.focused_pane();
            (
                pane.buffer_id,
                pane.point,
                pane.mark,
                pane.scroll_top,
                pane.scroll_row_offset,
                pane.preferred_column,
                pane.last_buffer_id,
                pane.buffer_states.clone(),
            )
        };

        let mut new_pane = Pane::new(pane_data.0);
        new_pane.point = pane_data.1;
        new_pane.mark = pane_data.2;
        new_pane.scroll_top = pane_data.3;
        new_pane.scroll_row_offset = pane_data.4;
        new_pane.preferred_column = pane_data.5;
        new_pane.last_buffer_id = pane_data.6;
        new_pane.buffer_states = pane_data.7;

        self.root = PaneNode::Leaf(new_pane);
        self.focus_path = vec![];
    }

    /// Cycle focus to the next pane.
    pub fn cycle_focus(&mut self) {
        let mut all_paths = Vec::new();
        Self::collect_leaf_paths(&self.root, &mut vec![], &mut all_paths);

        if all_paths.len() <= 1 {
            return;
        }

        let current_idx = all_paths
            .iter()
            .position(|p| p == &self.focus_path)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % all_paths.len();
        self.focus_path = all_paths[next_idx].clone();
    }

    /// Collect all leaf paths in order.
    fn collect_leaf_paths(
        node: &PaneNode,
        current_path: &mut Vec<usize>,
        paths: &mut Vec<Vec<usize>>,
    ) {
        match node {
            PaneNode::Leaf(_) => {
                paths.push(current_path.clone());
            }
            PaneNode::Split { children, .. } => {
                for (i, child) in children.iter().enumerate() {
                    current_path.push(i);
                    Self::collect_leaf_paths(child, current_path, paths);
                    current_path.pop();
                }
            }
        }
    }

    /// Calculate rects for all panes in the tree.
    /// Returns (pane_rects, separator_rects) where separator_rects are 1-column
    /// vertical bars between horizontally-arranged panes.
    pub fn calculate_rects(&self, area: Rect) -> (Vec<(Vec<usize>, Rect)>, Vec<Rect>) {
        let mut panes = Vec::new();
        let mut separators = Vec::new();
        Self::calc_rects_recursive(&self.root, area, &mut vec![], &mut panes, &mut separators);
        (panes, separators)
    }

    fn calc_rects_recursive(
        node: &PaneNode,
        area: Rect,
        path: &mut Vec<usize>,
        result: &mut Vec<(Vec<usize>, Rect)>,
        separators: &mut Vec<Rect>,
    ) {
        match node {
            PaneNode::Leaf(_) => {
                result.push((path.clone(), area));
            }
            PaneNode::Split {
                direction,
                children,
            } => {
                let n = children.len() as u16;
                if n == 0 {
                    return;
                }

                if *direction == Direction::Horizontal && n > 1 {
                    // For horizontal splits, reserve 1 column between each pair
                    // of children for a separator bar.
                    let num_separators = n - 1;
                    let available_width = area.width.saturating_sub(num_separators);
                    let base_width = available_width / n;
                    let extra = (available_width % n) as usize;

                    let mut x = area.x;
                    for (i, child) in children.iter().enumerate() {
                        let w = base_width + if i < extra { 1 } else { 0 };
                        let child_rect = Rect::new(x, area.y, w, area.height);
                        path.push(i);
                        Self::calc_rects_recursive(child, child_rect, path, result, separators);
                        path.pop();
                        x += w;

                        if (i as u16) < n - 1 {
                            separators.push(Rect::new(x, area.y, 1, area.height));
                            x += 1;
                        }
                    }
                } else {
                    let constraints: Vec<Constraint> = children
                        .iter()
                        .map(|_| Constraint::Ratio(1, n.into()))
                        .collect();
                    let chunks = Layout::default()
                        .direction(*direction)
                        .constraints(constraints)
                        .split(area);

                    for (i, (child, chunk)) in children.iter().zip(chunks.iter()).enumerate() {
                        path.push(i);
                        Self::calc_rects_recursive(child, *chunk, path, result, separators);
                        path.pop();
                    }
                }
            }
        }
    }

    /// Count the number of leaf panes.
    #[cfg(test)]
    pub fn pane_count(&self) -> usize {
        Self::count_leaves(&self.root)
    }

    #[cfg(test)]
    fn count_leaves(node: &PaneNode) -> usize {
        match node {
            PaneNode::Leaf(_) => 1,
            PaneNode::Split { children, .. } => children.iter().map(Self::count_leaves).sum(),
        }
    }

    fn node_at_path_mut(&mut self, path: &[usize]) -> &mut PaneNode {
        let mut node = &mut self.root;
        for &idx in path {
            match node {
                PaneNode::Split { children, .. } => {
                    node = &mut children[idx];
                }
                PaneNode::Leaf(_) => panic!("path goes through a leaf"),
            }
        }
        node
    }

    fn navigate_to_first_leaf(&mut self) {
        loop {
            let node = self.node_at_path_ref(&self.focus_path);
            match node {
                PaneNode::Leaf(_) => break,
                PaneNode::Split { .. } => {
                    self.focus_path.push(0);
                }
            }
        }
    }

    fn navigate_to_first_leaf_from_current(&mut self) {
        loop {
            let node = self.node_at_path_ref(&self.focus_path);
            match node {
                PaneNode::Leaf(_) => break,
                PaneNode::Split { .. } => {
                    self.focus_path.push(0);
                }
            }
        }
    }

    fn node_at_path_ref(&self, path: &[usize]) -> &PaneNode {
        let mut node = &self.root;
        for &idx in path {
            match node {
                PaneNode::Split { children, .. } => {
                    node = &children[idx];
                }
                PaneNode::Leaf(_) => panic!("path goes through a leaf"),
            }
        }
        node
    }

    /// Iterate over all panes.
    #[cfg(test)]
    pub fn for_each_pane<F: FnMut(&Pane)>(&self, f: &mut F) {
        Self::visit_panes(&self.root, f);
    }

    #[cfg(test)]
    fn visit_panes<F: FnMut(&Pane)>(node: &PaneNode, f: &mut F) {
        match node {
            PaneNode::Leaf(pane) => f(pane),
            PaneNode::Split { children, .. } => {
                for child in children {
                    Self::visit_panes(child, f);
                }
            }
        }
    }

    fn visit_panes_mut<F: FnMut(&mut Pane)>(node: &mut PaneNode, f: &mut F) {
        match node {
            PaneNode::Leaf(pane) => f(pane),
            PaneNode::Split { children, .. } => {
                for child in children {
                    Self::visit_panes_mut(child, f);
                }
            }
        }
    }

    /// Update viewport dimensions for a pane at a given path.
    pub fn update_pane_viewport(&mut self, path: &[usize], height: usize, width: usize) {
        self.pane_at_path_mut(path).set_viewport(height, width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_for_edit_shifts_point_and_mark_after_insertion() {
        let mut pane = Pane::new(1);
        pane.point = 5;
        pane.mark = Some(3);
        pane.adjust_for_edit(1, EditDelta::same_line(2, 0, 4)); // insert 4 chars at 2
        assert_eq!(pane.point, 9);
        assert_eq!(pane.mark, Some(7));
    }

    #[test]
    fn adjust_for_edit_point_at_insertion_stays() {
        let mut pane = Pane::new(1);
        pane.point = 2;
        pane.adjust_for_edit(1, EditDelta::same_line(2, 0, 4));
        assert_eq!(pane.point, 2);
    }

    #[test]
    fn adjust_for_edit_clamps_point_inside_deleted_region() {
        let mut pane = Pane::new(1);
        pane.point = 5;
        pane.adjust_for_edit(1, EditDelta::same_line(2, 6, 0)); // delete [2,8)
        assert_eq!(pane.point, 2);
    }

    #[test]
    fn adjust_for_edit_shifts_point_after_deletion() {
        let mut pane = Pane::new(1);
        pane.point = 10;
        pane.adjust_for_edit(1, EditDelta::same_line(2, 3, 0)); // delete [2,5)
        assert_eq!(pane.point, 7);
    }

    #[test]
    fn adjust_for_edit_ignores_other_buffers() {
        let mut pane = Pane::new(1);
        pane.point = 10;
        pane.mark = Some(4);
        pane.adjust_for_edit(2, EditDelta::same_line(0, 5, 0));
        assert_eq!(pane.point, 10);
        assert_eq!(pane.mark, Some(4));
    }

    #[test]
    fn adjust_for_edit_updates_saved_buffer_states() {
        let mut pane = Pane::new(1);
        pane.point = 10;
        pane.switch_buffer(2, 100); // saves state for buffer 1 (point 10)
        pane.adjust_for_edit(1, EditDelta::same_line(0, 5, 0)); // delete first 5 chars of buffer 1
        pane.restore_buffer_state(1, 95);
        assert_eq!(pane.point, 5);
    }

    #[test]
    fn scroll_top_shifts_down_when_lines_removed_above() {
        let mut pane = Pane::new(1);
        pane.scroll_top = 100;
        // Delete lines 0..50 (2500 chars, 50 line breaks).
        pane.adjust_for_edit(
            1,
            EditDelta {
                start: 0,
                removed: 2500,
                inserted: 0,
                first_line: 0,
                removed_lines: 50,
                inserted_lines: 0,
            },
        );
        assert_eq!(pane.scroll_top, 50);
    }

    #[test]
    fn scroll_top_shifts_up_when_lines_inserted_above() {
        let mut pane = Pane::new(1);
        pane.scroll_top = 100;
        // Insert 10 lines at the top of the buffer.
        pane.adjust_for_edit(
            1,
            EditDelta {
                start: 0,
                removed: 0,
                inserted: 60,
                first_line: 0,
                removed_lines: 0,
                inserted_lines: 10,
            },
        );
        assert_eq!(pane.scroll_top, 110);
    }

    #[test]
    fn scroll_top_unchanged_for_edit_below_viewport() {
        let mut pane = Pane::new(1);
        pane.scroll_top = 10;
        pane.adjust_for_edit(
            1,
            EditDelta {
                start: 5000,
                removed: 100,
                inserted: 0,
                first_line: 50,
                removed_lines: 2,
                inserted_lines: 0,
            },
        );
        assert_eq!(pane.scroll_top, 10);
    }

    #[test]
    fn scroll_top_clamps_when_its_line_is_removed() {
        let mut pane = Pane::new(1);
        pane.scroll_top = 100;
        // Delete lines 50..150; the top line no longer exists.
        pane.adjust_for_edit(
            1,
            EditDelta {
                start: 2500,
                removed: 5000,
                inserted: 0,
                first_line: 50,
                removed_lines: 100,
                inserted_lines: 0,
            },
        );
        assert_eq!(pane.scroll_top, 50);
    }

    #[test]
    fn saved_state_scroll_top_adjusted_too() {
        let mut pane = Pane::new(1);
        pane.scroll_top = 100;
        pane.switch_buffer(2, 100); // saves state for buffer 1 (scroll_top 100)
        pane.adjust_for_edit(
            1,
            EditDelta {
                start: 0,
                removed: 2500,
                inserted: 0,
                first_line: 0,
                removed_lines: 50,
                inserted_lines: 0,
            },
        );
        pane.restore_buffer_state(1, 7000);
        assert_eq!(pane.scroll_top, 50);
    }

    #[test]
    fn new_pane_tree() {
        let tree = PaneTree::new(0);
        assert_eq!(tree.pane_count(), 1);
        assert_eq!(tree.focused_pane().buffer_id, 0);
    }

    #[test]
    fn split_vertical() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Vertical, 0);
        assert_eq!(tree.pane_count(), 2);
    }

    #[test]
    fn split_horizontal() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Horizontal, 0);
        assert_eq!(tree.pane_count(), 2);
    }

    #[test]
    fn cycle_focus() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Vertical, 1);
        assert_eq!(tree.focused_pane().buffer_id, 0);
        tree.cycle_focus();
        assert_eq!(tree.focused_pane().buffer_id, 1);
        tree.cycle_focus();
        assert_eq!(tree.focused_pane().buffer_id, 0);
    }

    #[test]
    fn delete_focused() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Vertical, 1);
        assert_eq!(tree.pane_count(), 2);
        tree.cycle_focus(); // focus pane with buffer 1
        assert!(tree.delete_focused());
        assert_eq!(tree.pane_count(), 1);
        assert_eq!(tree.focused_pane().buffer_id, 0);
    }

    #[test]
    fn delete_only_pane_fails() {
        let mut tree = PaneTree::new(0);
        assert!(!tree.delete_focused());
    }

    #[test]
    fn delete_others() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Vertical, 1);
        tree.split(Direction::Horizontal, 2);
        assert_eq!(tree.pane_count(), 3);
        tree.delete_others();
        assert_eq!(tree.pane_count(), 1);
    }

    #[test]
    fn calculate_rects() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Vertical, 1);
        let (rects, separators) = tree.calculate_rects(Rect::new(0, 0, 80, 24));
        assert_eq!(rects.len(), 2);
        // Vertical splits have no separators
        assert_eq!(separators.len(), 0);
        // Both should have the full width, each half the height
        assert_eq!(rects[0].1.width, 80);
        assert_eq!(rects[1].1.width, 80);
        assert_eq!(rects[0].1.height + rects[1].1.height, 24);
    }

    #[test]
    fn horizontal_split_has_separator() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Horizontal, 1);
        let (rects, separators) = tree.calculate_rects(Rect::new(0, 0, 80, 24));
        assert_eq!(rects.len(), 2);
        assert_eq!(separators.len(), 1);
        // Separator should be 1 column wide, full height
        assert_eq!(separators[0].width, 1);
        assert_eq!(separators[0].height, 24);
        // Pane widths + separator = total width
        assert_eq!(rects[0].1.width + 1 + rects[1].1.width, 80);
        // Separator is between the two panes
        assert_eq!(separators[0].x, rects[0].1.x + rects[0].1.width);
        assert_eq!(rects[1].1.x, separators[0].x + 1);
    }

    #[test]
    fn three_way_horizontal_split_has_two_separators() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Horizontal, 1);
        // Focus first child, split it again
        tree.split(Direction::Horizontal, 2);
        // Now root is Horizontal with children:
        //   child 0: Split(Horizontal) { Leaf(0), Leaf(2) }
        //   child 1: Leaf(1)
        let (rects, separators) = tree.calculate_rects(Rect::new(0, 0, 81, 24));
        assert_eq!(rects.len(), 3);
        assert_eq!(separators.len(), 2);
        // All separators are 1 column wide
        for sep in &separators {
            assert_eq!(sep.width, 1);
            assert_eq!(sep.height, 24);
        }
    }

    // All lines are short (no wrapping) unless stated otherwise.
    fn short_line(_line: usize) -> usize {
        5
    }

    #[test]
    fn ensure_visible_scrolls_down() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 10;
        pane.scroll_top = 0;
        pane.ensure_visible(15, 0, short_line);
        assert_eq!(pane.scroll_top, 6);
    }

    #[test]
    fn ensure_visible_scrolls_up() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 10;
        pane.scroll_top = 10;
        pane.ensure_visible(5, 0, short_line);
        assert_eq!(pane.scroll_top, 5);
    }

    #[test]
    fn ensure_visible_no_scroll_when_visible() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 10;
        pane.scroll_top = 5;
        pane.ensure_visible(7, 0, short_line);
        assert_eq!(pane.scroll_top, 5);
    }

    #[test]
    fn ensure_visible_scrolls_down_with_wrapped_lines() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 4;
        pane.viewport_width = 20;
        pane.scroll_top = 0;
        // Line 0 is long (26 chars, wraps to 2 visual rows at width 20).
        // Lines 1-4 are short (5 chars, 1 visual row each).
        // Visual rows: line0=2, line1=1, line2=1 = 4 rows fills viewport.
        // Moving to line 3 should scroll.
        let line_len = |l: usize| if l == 0 { 26 } else { 5 };
        pane.ensure_visible(3, 0, line_len);
        // Scrolling is visual-row granular: one row of the wrapped line 0
        // moves off the top, which is enough to bring line 3 into view.
        assert!(
            pane.scroll_top > 0 || pane.scroll_row_offset > 0,
            "should have scrolled, scroll_top={} scroll_row_offset={}",
            pane.scroll_top,
            pane.scroll_row_offset
        );
    }

    #[test]
    fn ensure_visible_no_scroll_when_wrapped_line_still_fits() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 4;
        pane.viewport_width = 20;
        pane.scroll_top = 0;
        // Line 0 wraps to 2 visual rows, line 1 takes 1 row = 3 rows total.
        // viewport_height=4, so cursor on line 1 should NOT scroll.
        let line_len = |l: usize| if l == 0 { 26 } else { 5 };
        pane.ensure_visible(1, 0, line_len);
        assert_eq!(pane.scroll_top, 0);
    }

    // === Sub-line scrolling within lines taller than the viewport ===

    // One giant line: 200 chars at width 20 => 11 visual rows (19 chars per
    // wrapped segment).
    fn giant_line(_line: usize) -> usize {
        200
    }

    #[test]
    fn compute_scroll_position_scrolls_down_within_giant_line() {
        let (top, offset) = compute_scroll_position(0, 0, 0, 10, 4, 20, giant_line);
        assert_eq!((top, offset), (0, 7)); // rows 7..=10 visible
    }

    #[test]
    fn compute_scroll_position_scrolls_up_within_giant_line() {
        let (top, offset) = compute_scroll_position(0, 7, 0, 2, 4, 20, giant_line);
        assert_eq!((top, offset), (0, 2));
    }

    #[test]
    fn compute_scroll_position_no_change_when_row_visible() {
        let (top, offset) = compute_scroll_position(0, 7, 0, 8, 4, 20, giant_line);
        assert_eq!((top, offset), (0, 7));
    }

    #[test]
    fn compute_scroll_position_enters_giant_line_from_short_lines() {
        // Lines 0-2 short (1 row each), line 3 giant (11 rows). Cursor on
        // the giant line's last row must land it on the bottom viewport row.
        let line_len = |l: usize| if l == 3 { 200 } else { 5 };
        let (top, offset) = compute_scroll_position(0, 0, 3, 10, 4, 20, line_len);
        assert_eq!((top, offset), (3, 7));
    }

    #[test]
    fn compute_scroll_position_clamps_stale_offset() {
        // Offset 50 is beyond the top line's 11 rows; after clamping to the
        // last row (10), the cursor's row 10 is visible.
        let (top, offset) = compute_scroll_position(0, 50, 0, 10, 4, 20, giant_line);
        assert_eq!((top, offset), (0, 10));
    }

    #[test]
    fn scroll_down_visual_rows_moves_within_giant_line_and_stops_at_last_row() {
        let (top, offset) = scroll_down_visual_rows(0, 0, 3, 1, 20, giant_line);
        assert_eq!((top, offset), (0, 3));
        let (top, offset) = scroll_down_visual_rows(top, offset, 100, 1, 20, giant_line);
        assert_eq!((top, offset), (0, 10)); // clamped at the last visual row
    }

    #[test]
    fn scroll_down_visual_rows_crosses_into_next_line() {
        // Line 0 wraps to 2 rows (26 chars at width 20), lines 1+ short.
        // Scrolling 3 rows: (0,0) -> (0,1) -> (1,0) -> (2,0).
        let line_len = |l: usize| if l == 0 { 26 } else { 5 };
        let (top, offset) = scroll_down_visual_rows(0, 0, 3, 5, 20, line_len);
        assert_eq!((top, offset), (2, 0));
    }

    #[test]
    fn scroll_up_visual_rows_enters_wrapped_line_at_its_last_row() {
        let line_len = |l: usize| if l == 0 { 200 } else { 5 };
        let (top, offset) = scroll_up_visual_rows(1, 0, 1, 2, 20, line_len);
        assert_eq!((top, offset), (0, 10));
        let (top, offset) = scroll_up_visual_rows(top, offset, 100, 2, 20, line_len);
        assert_eq!((top, offset), (0, 0));
    }

    #[test]
    fn ensure_visible_scrolls_within_one_giant_line_and_back() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 4;
        pane.viewport_width = 20;
        pane.ensure_visible(0, 10, giant_line);
        assert_eq!((pane.scroll_top, pane.scroll_row_offset), (0, 7));
        pane.ensure_visible(0, 0, giant_line);
        assert_eq!((pane.scroll_top, pane.scroll_row_offset), (0, 0));
    }

    #[test]
    fn buffer_state_saves_and_restores_scroll_row_offset() {
        let mut pane = Pane::new(1);
        pane.scroll_row_offset = 7;
        pane.switch_buffer(2, 100);
        assert_eq!(pane.scroll_row_offset, 0);
        pane.restore_buffer_state(1, 200);
        assert_eq!(pane.scroll_row_offset, 7);
    }

    #[test]
    fn cycle_focus_single_pane_noop() {
        let mut tree = PaneTree::new(0);
        tree.cycle_focus();
        assert_eq!(tree.focused_pane().buffer_id, 0);
    }

    #[test]
    fn for_each_pane_visits_all() {
        let mut tree = PaneTree::new(0);
        tree.split(Direction::Vertical, 1);
        let mut ids = Vec::new();
        tree.for_each_pane(&mut |pane| {
            ids.push(pane.buffer_id);
        });
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&0));
        assert!(ids.contains(&1));
    }

    #[test]
    fn update_pane_viewport() {
        let mut tree = PaneTree::new(0);
        tree.update_pane_viewport(&[], 30, 100);
        assert_eq!(tree.focused_pane().viewport_height, 30);
        assert_eq!(tree.focused_pane().viewport_width, 100);
    }

    #[test]
    fn delete_from_three_way_split() {
        let mut tree = PaneTree::new(0);
        // Split twice to get 3 panes
        tree.split(Direction::Vertical, 1);
        // Focus is on first child [0], then move to second child [1].
        tree.cycle_focus();
        // Split the second child.
        tree.split(Direction::Vertical, 2);
        // Now we have a root split with 2 children:
        //   child 0: Leaf(0)
        //   child 1: Split { Leaf(1), Leaf(2) }
        assert_eq!(tree.pane_count(), 3);

        // Delete the focused pane (which is inside the nested split)
        assert!(tree.delete_focused());
        assert_eq!(tree.pane_count(), 2);
    }
}
