/// A single edit operation, recording what was changed.
#[derive(Debug, Clone)]
pub struct Edit {
    /// Char position where the edit occurred.
    pub position: usize,
    /// Text that was deleted (empty for pure insertions).
    pub deleted: String,
    /// Text that was inserted (empty for pure deletions).
    pub inserted: String,
}

/// A group of edits that should be undone/redone together.
#[derive(Debug, Clone)]
pub struct EditGroup {
    pub edits: Vec<Edit>,
}

/// Undo/redo history for a buffer.
pub struct History {
    undo_stack: Vec<EditGroup>,
    redo_stack: Vec<EditGroup>,
    /// Current uncommitted edits being accumulated.
    current_group: Vec<Edit>,
    /// Track the kind of last edit for grouping decisions.
    last_edit_kind: EditKind,
    /// Monotonic version counter, incremented on each commit.
    version: usize,
    /// Version at which the buffer was last saved/loaded (clean).
    /// `None` means the clean state is unreachable (branch diverged).
    clean_version: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    None,
    Insert,
    Delete,
    Other,
}

impl History {
    pub fn new() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_group: Vec::new(),
            last_edit_kind: EditKind::None,
            version: 0,
            clean_version: Some(0),
        }
    }

    /// Record an insert edit. Consecutive inserts at adjacent positions
    /// are grouped together until committed.
    pub fn record_insert(&mut self, position: usize, text: &str) {
        // If switching from non-insert, commit the previous group
        if self.last_edit_kind != EditKind::Insert && self.last_edit_kind != EditKind::None {
            self.commit();
        }

        // Check if this insert is adjacent to the last one (for grouping)
        let should_group = if let Some(last) = self.current_group.last() {
            // Adjacent if the new position is right after the last insertion.
            // Positions are char indices, so length must be in chars too.
            last.deleted.is_empty()
                && last.position + last.inserted.chars().count() == position
                && !text.contains(' ')
                && !text.contains('\n')
        } else {
            true // first edit in group
        };

        if !should_group {
            self.commit();
        }

        self.current_group.push(Edit {
            position,
            deleted: String::new(),
            inserted: text.to_string(),
        });
        self.last_edit_kind = EditKind::Insert;
        // Clear redo stack on new edit
        self.clear_redo();
    }

    /// Record a replacement edit (delete + insert at same position) as a single edit.
    /// This creates a single undo group so the replacement is undone atomically.
    pub fn record_replace(&mut self, position: usize, deleted_text: &str, inserted_text: &str) {
        // Commit any pending group first
        self.commit();

        self.current_group.push(Edit {
            position,
            deleted: deleted_text.to_string(),
            inserted: inserted_text.to_string(),
        });
        self.last_edit_kind = EditKind::Other;
        self.clear_redo();
    }

    /// Record a delete edit. Each delete is its own group.
    pub fn record_delete(&mut self, position: usize, deleted_text: &str) {
        // Commit any pending group first
        if self.last_edit_kind != EditKind::Delete && self.last_edit_kind != EditKind::None {
            self.commit();
        }

        self.current_group.push(Edit {
            position,
            deleted: deleted_text.to_string(),
            inserted: String::new(),
        });
        self.last_edit_kind = EditKind::Delete;
        // Don't auto-commit deletes - they'll be committed on next non-delete action
        // Clear redo stack on new edit
        self.clear_redo();
    }

    /// Clear the redo stack, invalidating clean_version if it was on the redo path.
    fn clear_redo(&mut self) {
        if !self.redo_stack.is_empty() {
            // If the clean version is ahead of current version,
            // it was on the redo path and is now unreachable
            if self.clean_version.is_some_and(|cv| cv > self.version) {
                self.clean_version = None;
            }
            self.redo_stack.clear();
        }
    }

    /// Commit the current group to the undo stack.
    pub fn commit(&mut self) {
        if !self.current_group.is_empty() {
            let group = EditGroup {
                edits: std::mem::take(&mut self.current_group),
            };
            self.undo_stack.push(group);
            self.version += 1;
            self.last_edit_kind = EditKind::None;
        }
    }

    /// Mark that a non-edit action occurred, which should commit the current group.
    pub fn mark_action(&mut self) {
        if self.last_edit_kind != EditKind::None {
            self.commit();
        }
    }

    /// Pop the top undo group, returning it for the editor to reverse.
    /// The group is moved to the redo stack.
    pub fn undo(&mut self) -> Option<EditGroup> {
        self.commit(); // commit any pending edits first
        if let Some(group) = self.undo_stack.pop() {
            self.version -= 1;
            self.redo_stack.push(group.clone());
            Some(group)
        } else {
            None
        }
    }

    /// Pop the top redo group, returning it for the editor to re-apply.
    /// The group is moved back to the undo stack.
    pub fn redo(&mut self) -> Option<EditGroup> {
        if let Some(group) = self.redo_stack.pop() {
            self.version += 1;
            self.undo_stack.push(group.clone());
            Some(group)
        } else {
            None
        }
    }

    /// Mark the current state as "clean" (e.g., after saving).
    pub fn mark_clean(&mut self) {
        self.clean_version = Some(self.version);
    }

    /// Check if the buffer is in the clean state (no modifications since last save/load).
    pub fn is_clean(&self) -> bool {
        self.current_group.is_empty() && self.clean_version == Some(self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_undo() {
        let mut hist = History::new();
        hist.record_insert(0, "h");
        hist.record_insert(1, "i");
        hist.commit();

        let group = hist.undo().unwrap();
        assert_eq!(group.edits.len(), 2);
        assert_eq!(group.edits[0].inserted, "h");
        assert_eq!(group.edits[1].inserted, "i");
    }

    #[test]
    fn undo_redo_roundtrip() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.record_insert(1, "b");
        hist.commit();

        let group = hist.undo().unwrap();
        assert_eq!(group.edits.len(), 2);

        let group = hist.redo().unwrap();
        assert_eq!(group.edits.len(), 2);
        assert_eq!(group.edits[0].inserted, "a");
    }

    #[test]
    fn redo_cleared_on_new_edit() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.commit();
        hist.undo();

        // Now record a new edit — redo should be cleared
        hist.record_insert(0, "b");
        hist.commit();

        assert!(hist.redo().is_none());
    }

    #[test]
    fn consecutive_inserts_group_together() {
        let mut hist = History::new();
        hist.record_insert(0, "h");
        hist.record_insert(1, "e");
        hist.record_insert(2, "l");
        hist.record_insert(3, "l");
        hist.record_insert(4, "o");
        hist.commit();

        let group = hist.undo().unwrap();
        // All adjacent chars should be in one group
        assert_eq!(group.edits.len(), 5);
    }

    #[test]
    fn multibyte_inserts_group_together() {
        let mut hist = History::new();
        // Positions are char indices; multibyte chars must not break grouping.
        hist.record_insert(0, "é");
        hist.record_insert(1, "x");
        hist.record_insert(2, "ü");
        hist.commit();

        let group = hist.undo().unwrap();
        assert_eq!(group.edits.len(), 3);
    }

    #[test]
    fn space_breaks_insert_group() {
        let mut hist = History::new();
        hist.record_insert(0, "h");
        hist.record_insert(1, "i");
        hist.record_insert(2, " "); // space commits ["h","i"], then starts new group with " "
        hist.record_insert(3, "y");
        hist.record_insert(4, "o");
        hist.commit();

        // Space triggers commit of ["h","i"], then [" ","y","o"] are in the next group
        let group2 = hist.undo().unwrap();
        assert_eq!(group2.edits.len(), 3); // " ", "y", "o"

        let group1 = hist.undo().unwrap();
        assert_eq!(group1.edits.len(), 2); // "h", "i"
    }

    #[test]
    fn delete_grouping() {
        let mut hist = History::new();
        hist.record_delete(4, "o");
        hist.record_delete(3, "l");
        hist.commit();

        let group = hist.undo().unwrap();
        assert_eq!(group.edits.len(), 2);
    }

    #[test]
    fn switching_edit_kind_commits() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.record_insert(1, "b");
        // Now delete — should commit the inserts first
        hist.record_delete(1, "b");
        hist.commit();

        let delete_group = hist.undo().unwrap();
        assert_eq!(delete_group.edits.len(), 1);
        assert_eq!(delete_group.edits[0].deleted, "b");

        let insert_group = hist.undo().unwrap();
        assert_eq!(insert_group.edits.len(), 2);
    }

    #[test]
    fn empty_undo_returns_none() {
        let mut hist = History::new();
        assert!(hist.undo().is_none());
    }

    #[test]
    fn empty_redo_returns_none() {
        let mut hist = History::new();
        assert!(hist.redo().is_none());
    }

    #[test]
    fn initially_clean() {
        let hist = History::new();
        assert!(hist.is_clean());
    }

    #[test]
    fn dirty_after_edit() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        assert!(!hist.is_clean());
    }

    #[test]
    fn clean_after_undo_to_origin() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.commit();
        assert!(!hist.is_clean());
        hist.undo();
        assert!(hist.is_clean());
    }

    #[test]
    fn clean_after_mark_clean() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.commit();
        hist.mark_clean();
        assert!(hist.is_clean());
    }

    #[test]
    fn clean_after_undo_redo_to_clean_point() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.commit();
        hist.mark_clean();
        hist.record_insert(1, "b");
        hist.commit();
        assert!(!hist.is_clean());
        hist.undo();
        assert!(hist.is_clean());
    }

    #[test]
    fn dirty_after_branch_diverges() {
        let mut hist = History::new();
        hist.record_insert(0, "a");
        hist.commit();
        hist.mark_clean(); // clean at version 1
        hist.undo(); // back to version 0
        hist.record_insert(0, "b"); // new branch, redo cleared
        hist.commit(); // version 2, but on different branch
                       // clean_version was 1 on old branch, now unreachable
        assert!(!hist.is_clean());
    }
}
