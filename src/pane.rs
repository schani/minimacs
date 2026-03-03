use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::buffer::BufferId;

/// A single pane (window) viewing a buffer.
#[allow(dead_code)]
pub struct Pane {
    pub buffer_id: BufferId,
    pub point: usize,
    pub mark: Option<usize>,
    pub preferred_column: Option<usize>,
    pub scroll_top: usize,
    pub viewport_height: usize,
    pub viewport_width: usize,
}

impl Pane {
    pub fn new(buffer_id: BufferId) -> Self {
        Self {
            buffer_id,
            point: 0,
            mark: None,
            preferred_column: None,
            scroll_top: 0,
            viewport_height: 24,
            viewport_width: 80,
        }
    }

    pub fn ensure_visible(&mut self, cursor_line: usize) {
        if cursor_line < self.scroll_top {
            self.scroll_top = cursor_line;
        } else if cursor_line >= self.scroll_top + self.viewport_height {
            self.scroll_top = cursor_line - self.viewport_height + 1;
        }
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
    pub root: PaneNode,
    /// Index path from root to focused leaf.
    focus_path: Vec<usize>,
}

#[allow(dead_code)]
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

    /// Get the focused pane mutably.
    pub fn focused_pane_mut(&mut self) -> &mut Pane {
        let path = self.focus_path.clone();
        self.pane_at_path_mut(&path)
    }

    /// Get the focus path (for comparing to determine if a pane is focused).
    pub fn focus_path(&self) -> &[usize] {
        &self.focus_path
    }

    /// Get pane at a specific path (for rendering).
    pub fn pane_at_focus_path(&self, path: &[usize]) -> &Pane {
        self.pane_at_path(path)
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
            (pane.buffer_id, pane.point, pane.mark, pane.scroll_top, pane.preferred_column)
        };

        let mut new_pane = Pane::new(pane_data.0);
        new_pane.point = pane_data.1;
        new_pane.mark = pane_data.2;
        new_pane.scroll_top = pane_data.3;
        new_pane.preferred_column = pane_data.4;

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
    pub fn calculate_rects(&self, area: Rect) -> Vec<(Vec<usize>, Rect)> {
        let mut result = Vec::new();
        Self::calc_rects_recursive(&self.root, area, &mut vec![], &mut result);
        result
    }

    fn calc_rects_recursive(
        node: &PaneNode,
        area: Rect,
        path: &mut Vec<usize>,
        result: &mut Vec<(Vec<usize>, Rect)>,
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
                let constraints: Vec<Constraint> =
                    children.iter().map(|_| Constraint::Ratio(1, n.into())).collect();
                let chunks = Layout::default()
                    .direction(*direction)
                    .constraints(constraints)
                    .split(area);

                for (i, (child, chunk)) in children.iter().zip(chunks.iter()).enumerate() {
                    path.push(i);
                    Self::calc_rects_recursive(child, *chunk, path, result);
                    path.pop();
                }
            }
        }
    }

    /// Count the number of leaf panes.
    pub fn pane_count(&self) -> usize {
        Self::count_leaves(&self.root)
    }

    fn count_leaves(node: &PaneNode) -> usize {
        match node {
            PaneNode::Leaf(_) => 1,
            PaneNode::Split { children, .. } => {
                children.iter().map(Self::count_leaves).sum()
            }
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

    /// Iterate over all panes (for checking modified buffers, etc.)
    pub fn for_each_pane<F: FnMut(&Pane)>(&self, f: &mut F) {
        Self::visit_panes(&self.root, f);
    }

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

    /// Update viewport dimensions for a pane at a given path.
    pub fn update_pane_viewport(&mut self, path: &[usize], height: usize, width: usize) {
        let pane = self.pane_at_path_mut(path);
        pane.viewport_height = height;
        pane.viewport_width = width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let rects = tree.calculate_rects(Rect::new(0, 0, 80, 24));
        assert_eq!(rects.len(), 2);
        // Both should have the full width, each half the height
        assert_eq!(rects[0].1.width, 80);
        assert_eq!(rects[1].1.width, 80);
        assert_eq!(rects[0].1.height + rects[1].1.height, 24);
    }

    #[test]
    fn ensure_visible_scrolls_down() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 10;
        pane.scroll_top = 0;
        pane.ensure_visible(15);
        assert_eq!(pane.scroll_top, 6);
    }

    #[test]
    fn ensure_visible_scrolls_up() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 10;
        pane.scroll_top = 10;
        pane.ensure_visible(5);
        assert_eq!(pane.scroll_top, 5);
    }

    #[test]
    fn ensure_visible_no_scroll_when_visible() {
        let mut pane = Pane::new(0);
        pane.viewport_height = 10;
        pane.scroll_top = 5;
        pane.ensure_visible(7);
        assert_eq!(pane.scroll_top, 5);
    }
}
