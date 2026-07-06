use std::path::{Path, PathBuf};

use anyhow::bail;

use crate::buffer::Buffer;
use crate::minibuffer::PromptKind;

use super::Editor;

/// The physical write target of a buffer save. Kept separate from the
/// buffer's logical `path` so a write can be redirected (e.g. resolving
/// symlinks at write time) without changing buffer identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum WriteTarget {
    /// Write to the buffer's own path (`C-x C-s` and quit-time saves).
    BufferPath,
    /// Write to an explicit path (`C-x C-w`); the buffer adopts it as its
    /// new identity (path, uniquified name, re-detected syntax) only after
    /// the write succeeds.
    Path(PathBuf),
}

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
        let buffer_id = self.current_buffer().id;
        if self.external_modification_guard(buffer_id, false) {
            return;
        }
        self.write_buffer_reporting(buffer_id, WriteTarget::BufferPath);
    }

    /// The external-modification guard shared by every flow that writes a
    /// buffer to its own path (`C-x C-s`, `C-x C-w` to the buffer's path,
    /// quit-time saves): if the file changed on disk since we last loaded
    /// or saved it, start the "changed on disk; save anyway?" prompt
    /// instead of writing, and return true. Answering "y" is the one
    /// bypass — the confirm handler writes via [`Editor::write_buffer`]
    /// directly. `resume_quit` marks a save that is part of the quit
    /// sequence, so the confirm handler resumes or cancels the quit.
    pub(super) fn external_modification_guard(
        &mut self,
        buffer_id: usize,
        resume_quit: bool,
    ) -> bool {
        let Some(buf) = self.buffers.iter().find(|b| b.id == buffer_id) else {
            return false;
        };
        if !buf.externally_modified() {
            return false;
        }
        let name = buf.name.clone();
        self.start_minibuffer_prompt(
            PromptKind::SaveAnywayConfirm {
                buffer_id,
                resume_quit,
            },
            &format!("{name} changed on disk; save anyway? (y/n) "),
        );
        true
    }

    /// The single choke point every file-writing flow goes through:
    /// `C-x C-s`, `C-x C-w`, and the save-anyway / overwrite / quit-save
    /// confirmation handlers. Cross-cutting save concerns (e.g. the
    /// external-modification guard) belong here, in exactly one place.
    ///
    /// On a successful write to an explicit target, the buffer adopts the
    /// target as its identity: path, uniquified name, re-detected syntax.
    /// A failed save never changes buffer identity.
    pub(super) fn write_buffer(
        &mut self,
        buffer_id: usize,
        target: WriteTarget,
    ) -> anyhow::Result<()> {
        let path = match target {
            WriteTarget::BufferPath => {
                let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buffer_id) else {
                    bail!("no buffer with id {buffer_id}");
                };
                return buf.save();
            }
            WriteTarget::Path(path) => path,
        };
        {
            let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buffer_id) else {
                bail!("no buffer with id {buffer_id}");
            };
            buf.save_as(&path)?;
            buf.redetect_syntax();
        }
        let base = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let name = self.unique_buffer_name_excluding(&base, Some(&path), Some(buffer_id));
        if let Some(buf) = self.buffers.iter_mut().find(|b| b.id == buffer_id) {
            buf.name = name;
        }
        Ok(())
    }

    /// Write a buffer via [`Editor::write_buffer`] and report the outcome
    /// in the minibuffer ("Wrote {name}" / "Error saving: {e}").
    pub(super) fn write_buffer_reporting(&mut self, buffer_id: usize, target: WriteTarget) {
        match self.write_buffer(buffer_id, target) {
            Ok(()) => {
                let name = self.buffer_by_id(buffer_id).name.clone();
                self.minibuffer.show_message(format!("Wrote {name}"));
            }
            Err(e) => {
                self.minibuffer.show_message(format!("Error saving: {e}"));
            }
        }
    }
}
