use std::path::Path;

use crate::buffer::Buffer;
use crate::minibuffer::PromptKind;

use super::Editor;

impl Editor {
    pub fn open_file(&mut self, path: &Path) -> anyhow::Result<()> {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        // Check if file is already open
        let existing_buffer_id = self.buffers.iter().find_map(|buf| {
            let bp = buf.path.as_ref()?;
            let buf_canonical = std::fs::canonicalize(bp).unwrap_or_else(|_| bp.clone());
            (buf_canonical == canonical).then_some(buf.id)
        });
        if let Some(buffer_id) = existing_buffer_id {
            let name = self.buffer_by_id(buffer_id).name.clone();
            self.switch_focused_pane_to_buffer(buffer_id);
            self.minibuffer
                .show_message(format!("Switched to buffer {name}"));
            return Ok(());
        }

        let id = self.next_buffer_id;
        self.next_buffer_id += 1;
        let mut buf = match Buffer::from_file(id, &canonical) {
            Ok(buf) => buf,
            Err(_) if !canonical.exists() => {
                // File doesn't exist yet — create a new empty buffer with the path
                Buffer::new_for_path(id, &canonical)
            }
            Err(e) => return Err(e),
        };
        buf.name = self.unique_buffer_name(&buf.name, buf.path.as_deref());
        let name = buf.name.clone();
        let msg = if buf.path.as_ref().is_some_and(|p| p.exists()) {
            format!("Opened {name}")
        } else {
            format!("(New file) {name}")
        };
        self.buffers.push(buf);
        self.switch_focused_pane_to_buffer(id);
        self.minibuffer.show_message(msg);
        Ok(())
    }

    /// Disambiguate a buffer name against the existing buffers, emacs-style:
    /// `mod.rs` collides → `mod.rs<lib>` (trailing path components), falling
    /// back to `mod.rs<2>` when paths can't tell them apart.
    fn unique_buffer_name(&self, base: &str, path: Option<&Path>) -> String {
        self.unique_buffer_name_excluding(base, path, None)
    }

    /// Like [`unique_buffer_name`], but ignores the buffer with id `exclude`
    /// (used when renaming an existing buffer).
    pub(super) fn unique_buffer_name_excluding(
        &self,
        base: &str,
        path: Option<&Path>,
        exclude: Option<usize>,
    ) -> String {
        let taken = |name: &str| {
            self.buffers
                .iter()
                .any(|b| b.name == name && Some(b.id) != exclude)
        };
        if !taken(base) {
            return base.to_string();
        }
        if let Some(parent) = path.and_then(|p| p.parent()) {
            let components: Vec<String> = parent
                .components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            for n in 1..=components.len() {
                let qualifier = components[components.len() - n..].join("/");
                let candidate = format!("{base}<{qualifier}>");
                if !taken(&candidate) {
                    return candidate;
                }
            }
        }
        let mut i = 2;
        loop {
            let candidate = format!("{base}<{i}>");
            if !taken(&candidate) {
                return candidate;
            }
            i += 1;
        }
    }

    fn switch_focused_pane_to_buffer(&mut self, buffer_id: usize) {
        let buffer_len = self.buffer_by_id(buffer_id).char_count();
        self.pane_tree
            .focused_pane_mut()
            .switch_buffer(buffer_id, buffer_len);
    }

    pub(super) fn switch_to_buffer(&mut self, name: &str) {
        let buffer_id = if name.is_empty() {
            self.pane_tree.focused_pane().alternate_buffer_id()
        } else {
            self.buffers.iter().find(|b| b.name == name).map(|b| b.id)
        };

        if let Some(buffer_id) = buffer_id {
            self.switch_focused_pane_to_buffer(buffer_id);
        } else if !name.is_empty() {
            self.minibuffer
                .show_message(format!("No buffer named '{name}'"));
        }
    }

    pub(super) fn kill_buffer(&mut self) {
        if self.minibuffer.is_active() {
            return;
        }
        let buffer_id = self.pane_tree.focused_pane().buffer_id;
        let is_modified = self.current_buffer().modified;
        let name = self.current_buffer().name.clone();

        if is_modified {
            self.start_minibuffer_prompt(
                PromptKind::KillConfirm { buffer_id },
                &format!("Buffer {name} modified; kill anyway? (y/n) "),
            );
            return;
        }

        self.do_kill_buffer(buffer_id);
    }

    pub(super) fn do_kill_buffer(&mut self, buffer_id: usize) {
        self.buffers.retain(|b| b.id != buffer_id);

        let new_id = if self.buffers.is_empty() {
            let buf = Buffer::new_scratch(self.next_buffer_id);
            self.next_buffer_id += 1;
            let id = buf.id;
            self.buffers.push(buf);
            id
        } else {
            self.buffers[0].id
        };
        let new_buffer_len = self.buffer_by_id(new_id).char_count();

        // Update all panes that referenced the killed buffer.
        self.pane_tree.for_each_pane_mut(&mut |pane| {
            pane.forget_buffer(buffer_id);
            if pane.buffer_id == buffer_id {
                pane.restore_buffer_state(new_id, new_buffer_len);
            }
        });
    }

    // === File operations ===

    pub(super) fn save(&mut self) {
        let has_path = self.current_buffer().path.is_some();
        if !has_path {
            self.write_file_prompt();
            return;
        }
        if self.current_buffer().externally_modified() {
            let buffer_id = self.current_buffer().id;
            let name = self.current_buffer().name.clone();
            self.start_minibuffer_prompt(
                PromptKind::SaveAnywayConfirm { buffer_id },
                &format!("{name} changed on disk; save anyway? (y/n) "),
            );
            return;
        }
        match self.current_buffer_mut().save() {
            Ok(()) => {
                let name = self.current_buffer().name.clone();
                self.minibuffer.show_message(format!("Wrote {name}"));
            }
            Err(e) => {
                self.minibuffer.show_message(format!("Error saving: {e}"));
            }
        }
    }
}
