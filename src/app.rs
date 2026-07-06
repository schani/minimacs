use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::command::Command;
use crate::editor::{EditRecord, Editor};
use crate::event::{EventSource, Poll};
use crate::keymap::{default_keymap, Key, KeymapResult, KeymapState};
use crate::minibuffer::PromptKind;
use crate::render;

pub struct App<B: Backend> {
    pub editor: Editor,
    pub terminal: Terminal<B>,
    input: InputState,
    /// Number of `render()` calls, so tests can assert that discarded
    /// events (mouse motion, key releases, focus changes) skip the redraw.
    #[cfg(test)]
    renders: usize,
}

/// All partially-consumed input state, gathered in one place:
///
/// - `keymap`: the chord-in-progress walk of the keymap trie (`C-x ...`).
/// - `esc_pending`: a bare ESC was seen; the next key gets the ALT modifier.
///
/// `Editor::pending_keys` — the mode-line display of the pending input — is
/// a mirror of this state (it lives on `Editor` because rendering only sees
/// `&Editor`). Every mutation goes through these methods so the mirror stays
/// in sync, and `reset` is the single point that clears everything at once.
struct InputState {
    keymap: KeymapState,
    esc_pending: bool,
}

impl InputState {
    fn new() -> Self {
        Self {
            keymap: KeymapState::new(default_keymap()),
            esc_pending: false,
        }
    }

    /// The single reset point for all pending input state: cancels any chord
    /// in progress, any pending ESC, and the mode-line indicator.
    fn reset(&mut self, editor: &mut Editor) {
        self.keymap.clear();
        self.esc_pending = false;
        editor.pending_keys.clear();
    }

    /// Record a bare ESC: the next key will be treated as Meta-modified.
    fn set_esc_pending(&mut self, editor: &mut Editor) {
        self.esc_pending = true;
        editor.pending_keys = format!("{}ESC ", self.keymap.pending_display());
    }

    /// Consume the pending-ESC flag, returning whether it was set.
    fn take_esc_pending(&mut self) -> bool {
        std::mem::take(&mut self.esc_pending)
    }

    fn pending_display(&self) -> String {
        self.keymap.pending_display()
    }

    /// Feed a key to the chord trie, keeping the mode-line display in sync:
    /// `Pending` shows the accumulated prefix, anything else clears it.
    fn process_key(&mut self, editor: &mut Editor, key: KeyEvent) -> KeymapResult {
        let result = self.keymap.process_key(key);
        editor.pending_keys = match result {
            KeymapResult::Pending => self.keymap.pending_display(),
            _ => String::new(),
        };
        result
    }
}

impl<B: Backend> App<B>
where
    B::Error: Send + Sync + 'static,
{
    pub fn new(terminal: Terminal<B>, editor: Editor) -> Self {
        Self {
            editor,
            terminal,
            input: InputState::new(),
            #[cfg(test)]
            renders: 0,
        }
    }

    pub fn run(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        loop {
            let event = match event_source.next_event() {
                Poll::Event(event) => event,
                // Timeouts deliver no event; nothing can have changed, so
                // skip the re-render instead of redrawing ~10×/s while idle.
                Poll::Timeout => continue,
                // The terminal is gone (tty hangup): no further input can
                // arrive, so exit instead of spinning on a dead source.
                // We can't prompt about unsaved buffers — there is no input
                // to answer with — so the editor just quits; main still
                // restores the terminal on this error path.
                Poll::Closed => anyhow::bail!("event source closed"),
            };
            let state_changed = self.dispatch_event(event);
            if self.editor.should_quit {
                break;
            }

            // Discarded events (bare mouse motion, key releases, focus
            // changes) change nothing, so skip the redraw. Any-motion mouse
            // tracking (mode 1003) floods `Moved` events on bare movement;
            // rendering each one is a render storm.
            if state_changed {
                self.update_viewport();
                self.render()?;
            }
        }
        Ok(())
    }

    /// Run until all events are consumed (for tests). Mirrors `run()`'s
    /// render gating so tests can assert which events cause a redraw.
    #[cfg(test)]
    pub fn run_until_idle(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        while let Poll::Event(event) = event_source.next_event() {
            let state_changed = self.dispatch_event(event);
            if self.editor.should_quit {
                break;
            }
            if state_changed {
                self.update_viewport();
                self.render()?;
            }
        }
        Ok(())
    }

    /// Route one input event to its handler. This is the single place that
    /// decides, per event kind, what happens to the pending input state
    /// (`InputState`): key events consume or reset it inside `handle_key`;
    /// paste and acted-on mouse events (left click, scroll) cancel any
    /// pending chord and pending ESC before being handled
    /// (cancel-then-handle, so a click mid-chord both cancels the chord and
    /// performs the click); discarded mouse events (bare motion, drags,
    /// button releases, non-left buttons) touch nothing — merely moving the
    /// mouse over the terminal must not cancel a chord — and a resize
    /// intentionally leaves a chord in progress alone.
    ///
    /// Returns whether the event may have changed visible state; `run()`
    /// skips the redraw when it did not. Key presses conservatively report
    /// true — whether a command actually changed anything is the editor's
    /// business, and over-rendering a keystroke is cheap.
    fn dispatch_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key_event) => {
                // Act only on Press and Repeat (a held key must still
                // repeat). Windows and kitty-protocol terminals also report
                // Release events; letting those through would execute every
                // keystroke twice.
                if key_event.kind == KeyEventKind::Release {
                    return false;
                }
                self.handle_key(key_event);
                true
            }
            Event::Paste(text) => {
                self.input.reset(&mut self.editor);
                // Paste during isearch extends the query (isearch-yank)
                // instead of inserting into a buffer.
                if self.editor.isearch.is_some() {
                    self.editor.isearch_yank(&text);
                } else {
                    self.handle_paste(&text);
                }
                true
            }
            Event::Mouse(mouse_event) => match mouse_event.kind {
                MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown => {
                    self.input.reset(&mut self.editor);
                    self.handle_mouse(mouse_event);
                    true
                }
                // Everything else is discarded without touching any state:
                // any-motion tracking (mode 1003) reports every bare mouse
                // movement, so motion must neither cancel a pending chord
                // nor trigger a render.
                _ => false,
            },
            // ratatui needs a redraw to re-layout after a resize.
            Event::Resize(_, _) => true,
            // FocusGained/FocusLost: no handler, nothing changed.
            _ => false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // C-g always cancels: reset all pending input state, then Cancel.
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.input.reset(&mut self.editor);
            self.editor.execute(Command::Cancel);
            return;
        }

        // Handle Esc-as-Meta prefix: bare Esc sets a flag so the next key gets ALT
        if key.code == KeyCode::Esc
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            self.input.set_esc_pending(&mut self.editor);
            return;
        }

        // If Esc was pending, add ALT modifier to this key
        let key = if self.input.take_esc_pending() {
            KeyEvent::new(key.code, key.modifiers | KeyModifiers::ALT)
        } else {
            key
        };

        // If isearch is active, route keys to isearch handler
        if self.editor.isearch.is_some() {
            self.handle_isearch_key(key);
            return;
        }

        // If minibuffer is active, intercept Enter/Tab then route through keymap
        if self.editor.minibuffer.is_active() {
            match (key.modifiers, key.code) {
                (KeyModifiers::NONE, KeyCode::Enter) => {
                    self.editor.submit_prompt();
                    return;
                }
                (KeyModifiers::NONE, KeyCode::Tab) => {
                    self.handle_minibuffer_tab();
                    return;
                }
                _ => {
                    self.editor.minibuffer.completions = None;
                    self.editor.minibuffer.completion_page = 0;
                    // Fall through to normal keymap processing
                }
            }
        }

        let pending_before = self.input.pending_display();
        match self.input.process_key(&mut self.editor, key) {
            KeymapResult::Matched(cmd) => {
                self.editor.execute(cmd);
            }
            KeymapResult::Pending => {}
            KeymapResult::NotFound => {
                if !pending_before.is_empty() {
                    // A dead-end chord (e.g. C-x j) must not self-insert.
                    let chord = format!("{pending_before}{}", Key::from_event(key).display());
                    self.editor
                        .minibuffer
                        .show_message(format!("{chord} is undefined"));
                    return;
                }
                // Self-insert fallback for printable chars
                if let KeyCode::Char(c) = key.code {
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT)
                    {
                        self.editor.execute(Command::InsertChar(c));
                    }
                }
            }
        }
    }

    fn handle_isearch_key(&mut self, key: KeyEvent) {
        use crate::editor::SearchDirection;

        match (key.modifiers, key.code) {
            // C-s during isearch: cycle forward
            (KeyModifiers::CONTROL, KeyCode::Char('s')) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.direction = SearchDirection::Forward;
                }
                self.editor.isearch_next();
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.label = "I-search: ".to_string();
                }
            }
            // C-r during isearch: cycle backward
            (KeyModifiers::CONTROL, KeyCode::Char('r')) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.direction = SearchDirection::Backward;
                }
                self.editor.isearch_next();
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.label = "I-search backward: ".to_string();
                }
            }
            // Enter: accept search position
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.editor.isearch_accept();
            }
            // Backspace: delete char from query
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.query.pop();
                    // Sync minibuffer buffer to query
                    let query = isearch.query.clone();
                    self.editor.minibuffer_buffer.text = ropey::Rope::from_str(&query);
                    self.editor.minibuffer_pane.point = query.chars().count();
                }
                self.editor.isearch_update();
            }
            // Printable char: add to query and search
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.query.push(c);
                    // Sync minibuffer buffer to query
                    let query = isearch.query.clone();
                    self.editor.minibuffer_buffer.text = ropey::Rope::from_str(&query);
                    self.editor.minibuffer_pane.point = query.chars().count();
                }
                self.editor.isearch_update();
            }
            // Any other key: accept search, then process the key normally
            _ => {
                self.editor.isearch_accept();
                self.handle_key(key);
            }
        }
    }

    fn handle_minibuffer_tab(&mut self) {
        use crate::minibuffer::{complete_path_with_candidates, complete_buffer_with_candidates};

        let had_completions = self.editor.minibuffer.completions.is_some();
        let kind = self.editor.minibuffer.prompt().map(|p| p.kind.clone());
        let input = self.editor.minibuffer_text();
        let (completed, candidates) = match kind {
            Some(PromptKind::FindFile) | Some(PromptKind::WriteFile) => {
                complete_path_with_candidates(&input, &self.editor.cwd)
            }
            Some(PromptKind::SwitchBuffer) => {
                let names = self.editor.buffer_names();
                complete_buffer_with_candidates(&input, &names)
            }
            _ => return,
        };

        // Always update completions list
        self.editor.minibuffer.completions = if candidates.is_empty() {
            None
        } else {
            Some(candidates)
        };

        // Only replace buffer text if the completion advanced the prefix
        if completed != input {
            self.editor.minibuffer_buffer.history.commit();
            let old_len = self.editor.minibuffer_buffer.char_count();
            self.editor
                .apply_edit(0, old_len, &completed, EditRecord::Replace);
            self.editor.minibuffer_buffer.history.commit();
            self.editor.minibuffer_pane.point = completed.chars().count();
            self.editor.minibuffer.completion_page = 0;
        } else if had_completions && self.editor.minibuffer.completions.is_some() {
            // Completions were already showing and prefix didn't change: advance page
            self.editor.minibuffer.completion_page += 1;
        } else {
            self.editor.minibuffer.completion_page = 0;
        }
    }

    fn handle_paste(&mut self, text: &str) {
        self.editor.clear_last_command();
        if self.editor.minibuffer.is_active() {
            self.editor.minibuffer.completions = None;
            self.editor.minibuffer.completion_page = 0;
        }
        let text = self.editor.normalized_paste(text);
        // Insert pasted text as a single undo group
        self.editor.active_buffer_mut().history.commit();
        let point = self.editor.active_pane().point;
        self.editor.apply_edit(point, point, &text, EditRecord::Insert);
        self.editor.active_pane_mut().point = point + text.chars().count();
        self.editor.active_buffer_mut().history.commit();
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {}
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                self.handle_mouse_scroll(mouse);
                return;
            }
            _ => return,
        }

        // Ignore clicks when the minibuffer is active
        if self.editor.minibuffer.is_active() {
            return;
        }

        self.editor.clear_last_command();

        let click_x = mouse.column;
        let click_y = mouse.row;

        // Calculate pane areas (same logic as update_viewport/render)
        let size = self.terminal.size().unwrap_or_default();
        let comp_height = render::completions_height(&self.editor, size.height, size.width);
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1 + comp_height),
        };

        let (pane_rects, _separators) = self.editor.pane_tree.calculate_rects(pane_area);

        // Find which pane was clicked
        for (path, rect) in &pane_rects {
            // Text area is the pane rect minus the 1-row mode line
            let text_area = ratatui::layout::Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height.saturating_sub(1),
            };

            if click_x >= text_area.x
                && click_x < text_area.x + text_area.width
                && click_y >= text_area.y
                && click_y < text_area.y + text_area.height
            {
                // Focus this pane
                self.editor.pane_tree.set_focus_path(path.clone());

                let pane = self.editor.pane_tree.focused_pane();
                let buf = self.editor.buffer_by_id(pane.buffer_id);
                let text_width = text_area.width as usize;

                let rel_x = (click_x - text_area.x) as usize;
                // The clicked screen row plus any visual rows of the top
                // line scrolled off above the viewport gives the visual row
                // counted from the top of the scroll_top line.
                let rel_y = (click_y - text_area.y) as usize
                    + render::clamped_row_offset(pane, buf, text_width);

                let col_in_text = rel_x;

                // Walk buffer lines from scroll_top to find which line the visual row maps to
                let scroll_top = pane.scroll_top;
                let total_lines = buf.line_count();
                let mut visual_row: usize = 0;
                let mut target_line = scroll_top;
                let mut target_visual_col = col_in_text;

                let mut line_idx = scroll_top;
                while line_idx < total_lines {
                    let line_visual_width = render::line_visual_width(buf, line_idx);
                    let num_visual = crate::pane::visual_lines_for_length(line_visual_width, text_width);

                    if visual_row + num_visual > rel_y {
                        // The click is within this line's visual rows
                        target_line = line_idx;
                        let row_within_line = rel_y - visual_row;

                        if text_width > 1 && line_visual_width > text_width {
                            // Wrapped line: compute visual column from segment
                            let chars_per_segment = text_width - 1;
                            target_visual_col = row_within_line * chars_per_segment + col_in_text;
                        } else {
                            target_visual_col = col_in_text;
                        }
                        break;
                    }

                    visual_row += num_visual;
                    line_idx += 1;
                }

                if line_idx >= total_lines {
                    // Clicked below all content — place at end of buffer
                    let char_count = buf.char_count();
                    self.editor.pane_tree.focused_pane_mut().point = char_count;
                } else {
                    let target_col = render::buffer_col_for_visual_col(buf, target_line, target_visual_col);
                    // buffer_col_for_visual_col skips zero-width chars, so
                    // it never lands on a combining mark, but it can land
                    // between a ZWJ and the next emoji of one cluster; snap
                    // out of the cluster.
                    let char_pos = buf
                        .snap_to_grapheme_boundary(buf.line_col_to_char(target_line, target_col));
                    self.editor.pane_tree.focused_pane_mut().point = char_pos;
                }

                self.editor.pane_tree.focused_pane_mut().preferred_column = None;
                return;
            }
        }
    }

    fn handle_mouse_scroll(&mut self, mouse: MouseEvent) {
        let scroll_x = mouse.column;
        let scroll_y = mouse.row;

        let size = self.terminal.size().unwrap_or_default();
        let comp_height = render::completions_height(&self.editor, size.height, size.width);
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1 + comp_height),
        };

        let (pane_rects, _separators) = self.editor.pane_tree.calculate_rects(pane_area);
        // One wheel notch scrolls 3 visual rows, so wrapped lines — even a
        // single line taller than the viewport — scroll through smoothly.
        let scroll_rows: usize = 3;

        for (path, rect) in &pane_rects {
            if scroll_x >= rect.x
                && scroll_x < rect.x + rect.width
                && scroll_y >= rect.y
                && scroll_y < rect.y + rect.height
            {
                let pane = self.editor.pane_tree.pane_at_focus_path(path);
                let buf = self.editor.buffer_by_id(pane.buffer_id);
                let scroll_top = pane.scroll_top;
                let scroll_row_offset = pane.scroll_row_offset;
                let text_width = rect.width as usize;
                let total_lines = buf.line_count();
                let line_len = |l: usize| render::line_visual_width(buf, l);

                let (new_top, new_offset) = match mouse.kind {
                    MouseEventKind::ScrollDown => crate::pane::scroll_down_visual_rows(
                        scroll_top,
                        scroll_row_offset,
                        scroll_rows,
                        total_lines,
                        text_width,
                        line_len,
                    ),
                    MouseEventKind::ScrollUp => crate::pane::scroll_up_visual_rows(
                        scroll_top,
                        scroll_row_offset,
                        scroll_rows,
                        total_lines,
                        text_width,
                        line_len,
                    ),
                    _ => return,
                };

                let pane = self.editor.pane_tree.pane_at_path_pub_mut(path);
                pane.scroll_top = new_top;
                pane.scroll_row_offset = new_offset;
                return;
            }
        }
    }

    fn update_viewport(&mut self) {
        let size = self.terminal.size().unwrap_or_default();
        let comp_height = render::completions_height(&self.editor, size.height, size.width);
        // Calculate the pane area (full area minus 1 row for minibuffer minus completions)
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1 + comp_height),
        };

        let (pane_rects, _separators) = self.editor.pane_tree.calculate_rects(pane_area);
        for (path, rect) in &pane_rects {
            // Each pane rect includes 1 row for mode line
            let text_height = rect.height.saturating_sub(1) as usize;
            let text_width = rect.width as usize;
            self.editor.pane_tree.update_pane_viewport(path, text_height, text_width);
        }

        // Update minibuffer pane viewport width
        self.editor.minibuffer_pane.viewport_width = size.width as usize;
    }

    pub fn render(&mut self) -> Result<()> {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        let editor = &self.editor;
        self.terminal.draw(|frame| {
            render::render(frame, editor);
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
