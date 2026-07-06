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
            self.dispatch_event(event);
            if self.editor.should_quit {
                break;
            }

            self.update_viewport();
            self.render()?;
        }
        Ok(())
    }

    /// Run until all events are consumed (for tests).
    #[cfg(test)]
    pub fn run_until_idle(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        while let Poll::Event(event) = event_source.next_event() {
            self.dispatch_event(event);
            if self.editor.should_quit {
                break;
            }
            self.update_viewport();
            self.render()?;
        }
        Ok(())
    }

    /// Route one input event to its handler. This is the single place that
    /// decides, per event kind, what happens to the pending input state
    /// (`InputState`): key events consume or reset it inside `handle_key`;
    /// paste and mouse events cancel any pending chord and pending ESC
    /// before being handled (cancel-then-handle, so a click mid-chord both
    /// cancels the chord and performs the click); a resize intentionally
    /// leaves a chord in progress alone.
    fn dispatch_event(&mut self, event: Event) {
        match event {
            Event::Key(key_event) => {
                // Act only on Press and Repeat (a held key must still
                // repeat). Windows and kitty-protocol terminals also report
                // Release events; letting those through would execute every
                // keystroke twice.
                if key_event.kind != KeyEventKind::Release {
                    self.handle_key(key_event);
                }
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
            }
            Event::Mouse(mouse_event) => {
                self.input.reset(&mut self.editor);
                self.handle_mouse(mouse_event);
            }
            Event::Resize(_, _) => {}
            _ => {}
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
        let editor = &self.editor;
        self.terminal.draw(|frame| {
            render::render(frame, editor);
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Poll, TestEventSource};
    use ratatui::backend::TestBackend;

    fn test_app(width: u16, height: u16, events: Vec<Event>) -> (App<TestBackend>, TestEventSource) {
        let backend = TestBackend::new(width, height);
        let terminal = Terminal::new(backend).unwrap();
        let editor = Editor::new();
        let app = App::new(terminal, editor);
        let event_source = TestEventSource::new(events);
        (app, event_source)
    }

    fn test_app_with_text(
        width: u16,
        height: u16,
        text: &str,
        events: Vec<Event>,
    ) -> (App<TestBackend>, TestEventSource) {
        let backend = TestBackend::new(width, height);
        let terminal = Terminal::new(backend).unwrap();
        let editor = Editor::new_with_text(text);
        let app = App::new(terminal, editor);
        let event_source = TestEventSource::new(events);
        (app, event_source)
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn alt(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::ALT))
    }

    fn char_key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }

    fn release(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new_with_kind(
            code,
            modifiers,
            KeyEventKind::Release,
        ))
    }

    fn key_events(s: &str) -> Vec<Event> {
        s.chars().map(char_key).collect()
    }

    /// Capture the rendered screen as a multi-line string.
    fn capture_screen(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let mut result = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                result.push_str(cell.symbol());
            }
            let trimmed = result.trim_end();
            result = trimmed.to_string();
            result.push('\n');
        }
        if result.ends_with('\n') {
            result.pop();
        }
        result
    }

    #[test]
    fn renders_scratch_buffer() {
        let (mut app, mut events) = test_app(40, 10, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn typing_shows_in_buffer() {
        let events = key_events("hello");
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "hello");
        let screen = capture_screen(&app.terminal);
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn key_release_events_are_ignored() {
        // Kitty-protocol terminals (and Windows) report release events;
        // acting on them would execute every keystroke twice.
        let events = vec![
            char_key('a'),
            release(KeyCode::Char('a'), KeyModifiers::NONE),
            char_key('b'),
            release(KeyCode::Char('b'), KeyModifiers::NONE),
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "ab");
    }

    #[test]
    fn key_release_does_not_start_isearch() {
        let events = vec![release(KeyCode::Char('s'), KeyModifiers::CONTROL)];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.isearch.is_none());
    }

    #[test]
    fn key_release_does_not_advance_pending_chord() {
        // C-x pressed, then a released C-c: the chord must neither complete
        // (no quit) nor be consumed — a following C-c press still quits.
        let events = vec![
            ctrl('x'),
            release(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(!app.editor.should_quit);
        assert_eq!(app.editor.pending_keys, "C-x ");

        let mut events = TestEventSource::new(vec![ctrl('c')]);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.should_quit);
    }

    #[test]
    fn typing_non_ascii_renders_and_edits_correctly() {
        let mut events = key_events("héllo wörld");
        events.push(key(KeyCode::Backspace)); // delete 'd'
        events.push(ctrl('a'));
        events.push(char_key('à')); // insert at beginning
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "àhéllo wörl");
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains("àhéllo wörl"), "screen: {screen}");
    }

    #[test]
    fn navigation_with_ctrl_keys() {
        let mut events = key_events("hello");
        events.push(ctrl('a')); // beginning of line
        events.push(ctrl('f')); // forward one
        events.push(ctrl('f')); // forward one
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 2);
    }

    #[test]
    fn cx_cc_quits() {
        let events = vec![ctrl('x'), ctrl('c')];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.should_quit);
    }

    /// Event source scripted with an explicit sequence of poll outcomes,
    /// including timeouts and closure — unlike `TestEventSource`, which only
    /// ever yields events until it closes.
    struct ScriptedEventSource {
        polls: std::collections::VecDeque<Poll>,
    }

    impl EventSource for ScriptedEventSource {
        fn next_event(&mut self) -> Poll {
            self.polls.pop_front().unwrap_or(Poll::Closed)
        }
    }

    #[test]
    fn run_returns_error_when_event_source_closes() {
        // A dead terminal (poll/read error) must terminate the main loop
        // with an error, not be treated as a timeout — the old code spun
        // forever at 10 polls/s on a hung-up tty.
        let (mut app, _) = test_app(40, 10, vec![]);
        let mut source = ScriptedEventSource {
            polls: std::collections::VecDeque::new(),
        };
        let err = app.run(&mut source).unwrap_err();
        assert!(
            err.to_string().contains("event source closed"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn run_continues_after_timeout_and_errors_on_close() {
        // A timeout is idle, not death: the loop must keep going and
        // process later events; only Closed ends it.
        let (mut app, _) = test_app(40, 10, vec![]);
        let mut source = ScriptedEventSource {
            polls: [Poll::Timeout, Poll::Event(char_key('a')), Poll::Closed].into(),
        };
        let result = app.run(&mut source);
        assert!(result.is_err());
        assert_eq!(app.editor.buffer_text(), "a");
    }

    #[test]
    fn run_exits_cleanly_on_quit_before_source_closes() {
        // A normal quit must still return Ok — the queue behind it never
        // gets a chance to close the source.
        let (mut app, mut events) = test_app(40, 10, vec![ctrl('x'), ctrl('c')]);
        app.run(&mut events).unwrap();
        assert!(app.editor.should_quit);
    }

    #[test]
    fn arrow_keys_work() {
        let mut events = key_events("hi");
        events.push(key(KeyCode::Left));
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 1);
    }

    #[test]
    fn multiline_navigation() {
        let mut events = key_events("abc");
        events.push(key(KeyCode::Enter));
        events.extend(key_events("def"));
        events.push(ctrl('p')); // up
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 0);
        assert_eq!(col, 3);
    }

    #[test]
    fn backspace_deletes() {
        let mut events = key_events("helo");
        events.push(key(KeyCode::Backspace));
        events.extend(key_events("lo"));
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "hello");
    }

    #[test]
    fn render_with_content() {
        let (mut app, mut events) =
            test_app_with_text(40, 10, "line one\nline two\nline three", vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        insta::assert_snapshot!(screen);
    }

    #[test]
    fn literal_tab_at_line_start_displays_as_spaces_to_tab_stop() {
        let (mut app, mut events) = test_app_with_text(20, 6, "\tfoo", vec![]);
        app.run_until_idle(&mut events).unwrap();

        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "    foo");
        assert_eq!(app.editor.buffer_text(), "\tfoo");
    }

    #[test]
    fn literal_tabs_display_by_snapping_to_next_tab_stop() {
        let text = "a\tfoo\nabcd\tfoo\nab\tcd\te";
        let (mut app, mut events) = test_app_with_text(20, 8, text, vec![]);
        app.run_until_idle(&mut events).unwrap();

        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "a   foo");
        assert_eq!(lines[1], "abcd    foo");
        assert_eq!(lines[2], "ab  cd  e");
        assert_eq!(app.editor.buffer_text(), text);
    }

    #[test]
    fn cursor_after_literal_tab_uses_expanded_visual_column() {
        let events = vec![ctrl('f')];
        let (mut app, mut events) = test_app_with_text(20, 6, "\tfoo", events);
        app.run_until_idle(&mut events).unwrap();

        assert_eq!(app.editor.point(), 1, "tab should remain one buffer character");
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (4, 0));
    }

    #[test]
    fn cursor_after_literal_tab_snaps_to_next_tab_stop() {
        let events = vec![ctrl('f'), ctrl('f')];
        let (mut app, mut events) = test_app_with_text(20, 6, "a\tfoo", events);
        app.run_until_idle(&mut events).unwrap();

        assert_eq!(app.editor.point(), 2, "tab should remain one buffer character");
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (4, 0));
    }

    #[test]
    fn literal_tabs_wrap_using_expanded_visual_width() {
        let (mut app, mut events) = test_app_with_text(8, 6, "abcdef\tgh", vec![]);
        app.run_until_idle(&mut events).unwrap();

        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "abcdef \\");
        assert_eq!(lines[1], " gh");
    }

    #[test]
    fn tab_inserts_four_spaces() {
        let events = vec![key(KeyCode::Tab)];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "    ");
        assert_eq!(app.editor.point(), 4);
    }

    #[test]
    fn cg_cancels_pending_keys() {
        let events = vec![ctrl('x'), ctrl('g')];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(!app.editor.should_quit);
        assert_eq!(app.editor.pending_keys, "");
    }

    #[test]
    fn unbound_key_after_prefix_does_not_self_insert() {
        let mut events = vec![ctrl('x'), char_key('j')];
        events.extend(key_events("")); // no-op, keeps style consistent
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        // The dead-end chord must not insert a literal 'j' or modify the buffer.
        assert_eq!(app.editor.buffer_text(), "");
        assert!(!app.editor.current_buffer().modified);
        // The user gets feedback instead.
        let screen = capture_screen(&app.terminal);
        assert!(
            screen.contains("C-x j is undefined"),
            "screen: {screen}"
        );
    }

    #[test]
    fn pending_keys_show_in_mode_line() {
        let events = vec![ctrl('x')];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(!app.editor.pending_keys.is_empty());
        let screen = capture_screen(&app.terminal);
        // Mode line should contain pending keys
        assert!(screen.contains("C-x"));
    }

    #[test]
    fn undo_via_ctrl_slash() {
        let mut events = key_events("hello");
        // Need to commit the group, then undo. Moving the cursor commits.
        events.push(ctrl('a')); // movement commits insert group
        events.push(ctrl('/')); // undo
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "");
    }

    #[test]
    fn paste_inserts_text() {
        let events = vec![Event::Paste("pasted text".to_string())];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "pasted text");
        assert_eq!(app.editor.point(), 11);
    }

    #[test]
    fn bracketed_paste_converts_crlf_to_buffer_ending() {
        let events = vec![Event::Paste("x\r\ny".to_string())];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "x\ny");
        assert_eq!(app.editor.point(), 3);
    }

    #[test]
    fn paste_is_single_undo_group() {
        let events = vec![
            Event::Paste("hello world".to_string()),
            ctrl('/'), // undo
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "");
    }

    // === Word wrap tests ===

    #[test]
    fn long_line_wraps_with_continuation_marker() {
        // Terminal: 20 wide, 6 tall (4 text rows + 1 mode line + 1 minibuffer)
        // Text width: 20
        // Chars per wrapped visual line: 19 (20 - 1 for '\')
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars > 20
        let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // First visual line: 19 chars + "\"
        assert!(
            lines[0].ends_with('\\'),
            "Expected continuation marker '\\', got: '{}'",
            lines[0]
        );
        // Second visual line: remaining 7 chars "tuvwxyz"
        assert!(
            lines[1].contains("tuvwxyz"),
            "Expected wrapped text 'tuvwxyz', got: '{}'",
            lines[1]
        );
    }

    #[test]
    fn continuation_line_has_no_gutter() {
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars
        let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // First line starts directly with text content (no gutter)
        assert!(
            lines[0].starts_with("abcdefg"),
            "Expected text content, got: '{}'",
            lines[0]
        );
        // Continuation line also starts directly with text
        assert!(
            lines[1].starts_with("tuvwxyz"),
            "Expected continuation text, got: '{}'",
            lines[1]
        );
    }

    #[test]
    fn wrapped_line_uses_multiple_visual_rows() {
        // Terminal: 20 wide, 8 tall (6 text rows + 1 mode line + 1 minibuffer)
        // Text width: 20, chars per segment: 19
        // Two buffer lines, first one wraps into 2 visual lines
        let text = "abcdefghijklmnopqrstuvwxyz\nshort";
        let (mut app, mut events) = test_app_with_text(20, 8, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // Line 1 visual row 1: 19 chars + "\"
        // Line 1 visual row 2: remaining "tuvwxyz"
        // Line 2 visual row 1: "short"
        assert!(
            lines[0].contains("abcdefg"),
            "First visual line should have start of long line: '{}'",
            lines[0]
        );
        assert!(
            lines[2].contains("short"),
            "Second buffer line should contain 'short': '{}'",
            lines[2]
        );
    }

    #[test]
    fn cursor_on_wrapped_portion() {
        // Place cursor at end of a long line, which is on the wrapped part
        // Terminal: 20 wide, 6 tall
        // Text width: 20, chars per wrap: 19
        let text = "abcdefghijklmnopqrstuvwxyz";
        let events = vec![ctrl('e')]; // go to end of line (char 25)
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();

        // "abcdefghijklmnopqrstuvwxyz" has 26 chars, C-e goes to position 26
        assert_eq!(app.editor.point(), 26);

        // Verify the wrapped text renders correctly
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert!(lines[0].ends_with('\\'), "Line should wrap: '{}'", lines[0]);
    }

    #[test]
    fn cursor_after_cjk_chars_uses_double_width_columns() {
        let text = "你好ab";
        let events = vec![ctrl('e')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        let pos = app.terminal.get_cursor_position().unwrap();
        // 你(2) 好(2) a(1) b(1) => cursor at visual column 6.
        assert_eq!((pos.x, pos.y), (6, 0));
    }

    #[test]
    fn cursor_after_combining_mark_does_not_advance_extra_column() {
        let text = "e\u{301}x"; // e + combining acute (1 column) + x
        let events = vec![ctrl('e')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (2, 0));
    }

    #[test]
    fn wide_chars_wrap_by_visual_width() {
        // 6 CJK chars = 12 visual columns; terminal width 7 (cps 6) wraps
        // after 3 chars. The first row must end with the continuation marker.
        let text = "你好你好你好";
        let (mut app, mut events) = test_app_with_text(7, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let first = screen.lines().next().unwrap();
        assert!(first.contains('\\'), "first row should wrap: {first:?}");
        // TestBackend dumps the continuation cell of a wide char as a space.
        let condensed: String = first.chars().filter(|c| *c != ' ').collect();
        assert!(condensed.starts_with("你好你"), "first row: {first:?}");
    }

    #[test]
    fn cursor_at_eol_of_full_last_wrap_segment_stays_on_that_row() {
        // Terminal 10 wide: text_width=10, chars-per-segment=9.
        // An 18-char line renders as rows [0..9)+'\' and [9..18).
        // C-e (visual col 18) must put the cursor on visual row 1, col 9 —
        // not on top of the next buffer line's first column.
        let text = "abcdefghijklmnopqr\nZZZ";
        let events = vec![ctrl('e')];
        let (mut app, mut events) = test_app_with_text(10, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 18);
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (9, 1));
    }

    #[test]
    fn cursor_at_eol_of_exactly_full_line_wraps_to_next_row() {
        // Terminal 20 wide: a 20-char line exactly fills the pane width.
        // C-e puts point at EOL (visual col 20 == text_width, one past the
        // last cell), so the cursor wraps to column 0 of the next visual
        // row (emacs behavior) instead of being hidden.
        let text = "aaaaaaaaaaaaaaaaaaaa\nnext"; // 20 a's
        let events = vec![ctrl('e')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 20);
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 1));
    }

    #[test]
    fn cursor_wrap_at_viewport_bottom_scrolls_one_row() {
        // 20x6 => 4 text rows. The exactly-full line sits on the last text
        // row; its EOL cursor wraps to a row below the viewport, so the
        // pane must scroll one visual row to keep the cursor visible.
        let text = "one\ntwo\nthree\naaaaaaaaaaaaaaaaaaaa\nnext";
        let events = vec![ctrl('n'), ctrl('n'), ctrl('n'), ctrl('e')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        let pane = app.editor.pane_tree.focused_pane();
        assert_eq!((pane.scroll_top, pane.scroll_row_offset), (1, 0));
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 3));
        let screen = capture_screen(&app.terminal);
        assert!(
            screen.lines().nth(3).unwrap().starts_with("next"),
            "row under the cursor must show the next line: {screen}"
        );
    }

    #[test]
    fn cursor_wrap_past_last_buffer_line_still_visible() {
        // The exactly-full line is the LAST buffer line: the cursor's
        // wrapped row is past all content (a blank row), but it is still
        // within the text area and must be drawn there.
        let text = "one\naaaaaaaaaaaaaaaaaaaa"; // 20 a's, no trailing newline
        let events = vec![ctrl('n'), ctrl('e')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 24);
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 2));
    }

    #[test]
    fn cursor_at_eol_of_exactly_full_cjk_line_wraps_to_next_row() {
        // 10 CJK chars = 20 visual columns, exactly filling a 20-wide pane.
        let text = "你好你好你好你好你好\nnext";
        let events = vec![ctrl('e')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 10);
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 1));
    }

    #[test]
    fn cursor_at_eol_of_exactly_full_final_wrap_segment_wraps_to_next_row() {
        // Terminal 10 wide: cps=9, so a 19-char line renders as rows
        // [0..9)+'\' and [9..19) — the final segment exactly fills all 10
        // columns. EOL wraps the cursor to row 2, column 0.
        let text = "abcdefghijklmnopqrs\nZZZ"; // 19 chars
        let events = vec![ctrl('e')];
        let (mut app, mut events) = test_app_with_text(10, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 19);
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 2));
    }

    #[test]
    fn cursor_in_right_pane_never_drawn_outside_pane() {
        // Terminal 21 wide; C-x 3 gives left pane 10, separator, right pane 10.
        // A line exactly filling the right pane's width puts EOL at col 10,
        // which has no cell; the cursor wraps to column 0 of the pane's next
        // row (x=11, the right pane's origin) and must never be drawn at
        // x=21 (outside the terminal/pane).
        let text = "abcdefghij"; // 10 chars
        let events = vec![ctrl('x'), char_key('3'), ctrl('x'), char_key('o'), ctrl('e')];
        let (mut app, mut events) = test_app_with_text(21, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        let pos = app.terminal.get_cursor_position().unwrap();
        assert!(pos.x < 21, "cursor drawn outside the terminal: {pos:?}");
        assert_eq!((pos.x, pos.y), (11, 1));
    }

    #[test]
    fn scroll_accounts_for_tab_expanded_visual_width() {
        // Terminal 20x6 => 4 text rows. Each tabby line is 8 chars but 23
        // visual columns, so it occupies 2 visual rows. Two of them fill the
        // viewport; moving to the third line must scroll.
        let text = "\t\t\t\t\taaa\n\t\t\t\t\tbbb\nccc";
        let events = vec![ctrl('n'), ctrl('n')];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        let (line, _) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 2);
        // Scrolling is visual-row granular: sub-line scrolling within the
        // wrapped first line counts, as long as the cursor's line shows.
        let pane = app.editor.pane_tree.focused_pane();
        assert!(
            pane.scroll_top > 0 || pane.scroll_row_offset > 0,
            "viewport must scroll when wrapped tab lines push the cursor below it"
        );
        let screen = capture_screen(&app.terminal);
        assert!(
            screen.contains("ccc"),
            "cursor's line must be visible: {screen}"
        );
    }

    #[test]
    fn triple_wrap_line() {
        // A very long line that wraps 3 times
        // Terminal: 15 wide, 8 tall (6 text rows + mode + minibuf)
        // Text width: 15, chars per wrap: 14
        let text = "abcdefghijklmnopqrstuvwxyz0123456789"; // 36 chars
        let (mut app, mut events) = test_app_with_text(15, 8, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // Visual line 0: 14 chars + "\"
        assert!(lines[0].ends_with('\\'), "First wrap: '{}'", lines[0]);
        // Visual line 1: 14 chars + "\"
        assert!(lines[1].ends_with('\\'), "Second wrap: '{}'", lines[1]);
        // Visual line 2: remaining 8 chars (no \)
        assert!(!lines[2].ends_with('\\'), "Last segment shouldn't wrap: '{}'", lines[2]);
    }

    #[test]
    fn scroll_accounts_for_wrapped_lines() {
        // If the first line wraps and takes 2 visual rows, the second buffer
        // line should appear on visual row 2, reducing visible buffer lines
        // Terminal: 20 wide, 6 tall (4 text rows + mode + minibuf)
        let text = "abcdefghijklmnopqrstuvwxyz\nline2\nline3\nline4\nline5";
        let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        // The long first line takes 2 visual rows, so only 2 more buffer lines fit
        // in the 4 text rows (4 - 2 = 2 rows for lines 2 and 3)
        assert!(
            screen.contains("line2"),
            "line2 should be visible: {}",
            screen
        );
        assert!(
            screen.contains("line3"),
            "line3 should be visible: {}",
            screen
        );
        // line4 should NOT fit in the 4 visible text rows
        assert!(
            !screen.contains("line4"),
            "line4 should NOT be visible (pushed out by wrapping): {}",
            screen
        );
    }

    #[test]
    fn scroll_down_accounts_for_wrapped_lines() {
        // Terminal: 20 wide, 6 tall (4 text rows + mode + minibuf)
        // Line 0 wraps to 2 visual rows, so only 3 buffer lines fit on screen.
        // Moving cursor to line 3 should trigger scroll.
        let text = "abcdefghijklmnopqrstuvwxyz\nline2\nline3\nline4\nline5";
        let events = vec![
            ctrl('n'), // move to line 1
            ctrl('n'), // move to line 2
            ctrl('n'), // move to line 3 -- should scroll
        ];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        // Cursor is on line 3 ("line4"), which must be visible
        assert!(
            screen.contains("line4"),
            "line4 should be visible after scrolling down: {}",
            screen
        );
    }

    // === Sub-line scrolling tests (lines that wrap taller than the viewport) ===
    //
    // Terminal 20x6 => 4 text rows, wrap width 20 (19 chars per wrapped
    // segment + '\'). A 200-char line occupies 11 visual rows.

    /// A line of repeating digits: the char at index i is i % 10, so any
    /// off-by-one in cursor/scroll mapping shows up as a digit mismatch.
    fn digit_line(len: usize) -> String {
        "0123456789".chars().cycle().take(len).collect()
    }

    /// The character under the terminal cursor.
    fn char_under_cursor(app: &mut App<TestBackend>) -> String {
        let pos = app.terminal.get_cursor_position().unwrap();
        app.terminal.backend().buffer()[(pos.x, pos.y)]
            .symbol()
            .to_string()
    }

    #[test]
    fn meta_end_in_one_line_buffer_scrolls_cursor_into_view() {
        let text = digit_line(200);
        let events = vec![alt(KeyCode::Char('>'))];
        let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 200);

        // Point is on the line's last visual row (chars 190..200, so col 10);
        // that row must be scrolled into view as the bottom text row.
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!(
            (pos.x, pos.y),
            (10, 3),
            "cursor must be visible on the last text row"
        );
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(
            lines[3], "0123456789",
            "bottom row must show the line's tail"
        );
    }

    #[test]
    fn meta_beginning_recovers_from_sub_line_scroll() {
        let text = digit_line(200);
        let events = vec![alt(KeyCode::Char('>')), alt(KeyCode::Char('<'))];
        let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 0);

        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 0));
        let screen = capture_screen(&app.terminal);
        let first_row: String = text.chars().take(19).collect();
        assert_eq!(
            screen.lines().next().unwrap(),
            format!("{first_row}\\"),
            "view must be back at the top of the line"
        );
    }

    #[test]
    fn ctrl_f_steps_through_giant_wrapped_line_keeping_cursor_visible() {
        // 100 chars wrap to 6 visual rows — taller than the 4-row viewport.
        let len = 100;
        let text = digit_line(len);
        let (mut app, mut events) = test_app_with_text(20, 6, &text, vec![]);
        app.run_until_idle(&mut events).unwrap();

        for step in 1..=len {
            let mut es = TestEventSource::new(vec![ctrl('f')]);
            app.run_until_idle(&mut es).unwrap();
            assert_eq!(app.editor.point(), step);
            let pos = app.terminal.get_cursor_position().unwrap();
            assert!(pos.y < 4, "cursor row {} off-screen at point {step}", pos.y);
            if step < len {
                assert_eq!(
                    char_under_cursor(&mut app),
                    (step % 10).to_string(),
                    "cursor not over the char at point {step}"
                );
            }
        }

        // And back again: every C-b step must keep the cursor visible too.
        for step in (0..len).rev() {
            let mut es = TestEventSource::new(vec![ctrl('b')]);
            app.run_until_idle(&mut es).unwrap();
            assert_eq!(app.editor.point(), step);
            let pos = app.terminal.get_cursor_position().unwrap();
            assert!(
                pos.y < 4,
                "cursor row {} off-screen at point {step} going back",
                pos.y
            );
            assert_eq!(
                char_under_cursor(&mut app),
                (step % 10).to_string(),
                "cursor not over the char at point {step} going back"
            );
        }
    }

    #[test]
    fn mouse_wheel_scrolls_within_one_line_buffer() {
        let text = digit_line(200);
        let events = vec![mouse_scroll_down(5, 2)];
        let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // One notch scrolls 3 visual rows into the line: the top text row
        // now shows chars 57..76.
        let screen = capture_screen(&app.terminal);
        let scrolled_row: String = text.chars().skip(57).take(19).collect();
        assert_eq!(
            screen.lines().next().unwrap(),
            format!("{scrolled_row}\\"),
            "wheel must scroll within the single wrapped line"
        );

        // Wheel-up recovers to the top of the line.
        let mut es = TestEventSource::new(vec![mouse_scroll_up(5, 2)]);
        app.run_until_idle(&mut es).unwrap();
        let screen = capture_screen(&app.terminal);
        let first_row: String = text.chars().take(19).collect();
        assert_eq!(screen.lines().next().unwrap(), format!("{first_row}\\"));
    }

    #[test]
    fn mouse_click_on_wrapped_row_accounts_for_sub_line_scroll() {
        let text = digit_line(200);
        // One wheel notch scrolls 3 visual rows into the line, so a click on
        // text row 1, column 2 lands on visual row 4 of the line.
        let events = vec![mouse_scroll_down(5, 2), mouse_click(2, 1)];
        let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 4 * 19 + 2);
    }

    #[test]
    fn giant_line_after_normal_lines_scrolls_down_and_back() {
        let text = format!("one\ntwo\nthree\n{}", digit_line(200));
        let events = vec![alt(KeyCode::Char('>'))];
        let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // Cursor at the end of the giant line: its last visual row must be
        // the bottom text row.
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (10, 3));
        let screen = capture_screen(&app.terminal);
        assert_eq!(screen.lines().nth(3).unwrap(), "0123456789");

        // M-< brings the view all the way back up.
        let mut es = TestEventSource::new(vec![alt(KeyCode::Char('<'))]);
        app.run_until_idle(&mut es).unwrap();
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y), (0, 0));
        let screen = capture_screen(&app.terminal);
        assert_eq!(screen.lines().next().unwrap(), "one");

        // Two wheel notches scroll 6 visual rows: past the three short lines
        // and 3 rows into the giant line.
        let mut es = TestEventSource::new(vec![mouse_scroll_down(5, 2), mouse_scroll_down(5, 2)]);
        app.run_until_idle(&mut es).unwrap();
        let screen = capture_screen(&app.terminal);
        let giant = digit_line(200);
        let scrolled_row: String = giant.chars().skip(57).take(19).collect();
        assert_eq!(screen.lines().next().unwrap(), format!("{scrolled_row}\\"));

        // And two notches back up restore the top of the file.
        let mut es = TestEventSource::new(vec![mouse_scroll_up(5, 2), mouse_scroll_up(5, 2)]);
        app.run_until_idle(&mut es).unwrap();
        let screen = capture_screen(&app.terminal);
        assert_eq!(screen.lines().next().unwrap(), "one");
    }

    #[test]
    fn recenter_keeps_cursor_visible_in_giant_wrapped_line() {
        let text = digit_line(200);
        let events = vec![alt(KeyCode::Char('>')), ctrl('l')];
        let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // After C-l the cursor's visual row (the line's tail, rendered as a
        // bare "0123456789" row) must still be on screen, with the cursor on
        // it at column 10.
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        let tail_row = lines.iter().take(4).position(|l| *l == "0123456789");
        let tail_row = tail_row.expect("the cursor's visual row must remain visible after C-l");
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_eq!((pos.x, pos.y as usize), (10, tail_row));
    }

    // === Recenter integration tests ===

    #[test]
    fn cl_recenter_via_app() {
        // 20 lines, terminal is 12 tall (10 text rows + mode + minibuf)
        let text = (0..20).map(|i| format!("line{}", i)).collect::<Vec<_>>().join("\n");
        // Move cursor to line 10 (C-n 10 times), then C-l
        let mut events: Vec<Event> = (0..10).map(|_| ctrl('n')).collect();
        events.push(ctrl('l')); // recenter
        let (mut app, mut events) = test_app_with_text(40, 12, &text, events);
        app.run_until_idle(&mut events).unwrap();
        // Cursor should be on line 10
        let (line, _) = app.editor.current_buffer().char_to_line_col(app.editor.point());
        assert_eq!(line, 10);
        // After center: scroll_top = 10 - 10/2 = 5
        assert_eq!(app.editor.pane_tree.focused_pane().scroll_top, 5);
    }

    // === Isearch integration tests ===

    #[test]
    fn isearch_forward_via_app() {
        let text = "hello world hello";
        let events = vec![
            ctrl('s'),       // start isearch
            char_key('h'),   // type 'h'
            char_key('e'),   // type 'e'
            char_key('l'),   // type 'l'
            char_key('l'),   // type 'l'
            char_key('o'),   // type 'o'
            ctrl('s'),       // cycle to next match
            key(KeyCode::Enter), // accept
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        // Should be at the second "hello" (position 12)
        assert_eq!(app.editor.point(), 12);
        assert!(app.editor.isearch.is_none()); // isearch ended
    }

    #[test]
    fn isearch_backward_via_app() {
        let text = "hello world hello";
        let mut events = vec![ctrl('e')]; // go to end
        events.push(ctrl('r'));            // start backward isearch
        events.extend(key_events("hello"));
        events.push(key(KeyCode::Enter)); // accept
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        // rfind from position 17 finds the last "hello" before that = position 12
        assert_eq!(app.editor.point(), 12);
    }

    #[test]
    fn isearch_backspace_refines_query() {
        let text = "abc abcd abcde";
        let events = vec![
            ctrl('s'),
            char_key('a'), char_key('b'), char_key('c'), char_key('d'), char_key('e'),
            key(KeyCode::Backspace), // remove 'e' from query → "abcd"
            key(KeyCode::Enter),
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        // "abcd" matches at position 4
        assert_eq!(app.editor.point(), 4);
    }

    #[test]
    fn isearch_other_key_accepts_and_processes() {
        let text = "hello world";
        let events = vec![
            ctrl('s'),
            char_key('w'), char_key('o'), char_key('r'), char_key('l'), char_key('d'),
            ctrl('a'), // not an isearch key → accept search, then beginning-of-line
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        // C-a after accept should go to beginning of line
        assert_eq!(app.editor.point(), 0);
        assert!(app.editor.isearch.is_none());
    }

    #[test]
    fn isearch_cancel_restores_via_app() {
        let text = "hello world";
        let events = vec![
            ctrl('s'),
            char_key('w'), char_key('o'), char_key('r'), char_key('l'), char_key('d'),
            ctrl('g'), // cancel isearch
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        // Should restore to original position (0)
        assert_eq!(app.editor.point(), 0);
    }

    #[test]
    fn failing_isearch_shows_failing_label() {
        let mut events = vec![ctrl('s')];
        events.extend(key_events("zzz"));
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains("Failing I-search: zzz"), "screen: {screen}");
    }

    #[test]
    fn failing_isearch_backward_shows_failing_label() {
        let mut events = vec![ctrl('r')];
        events.extend(key_events("zzz"));
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        assert!(
            screen.contains("Failing I-search backward: zzz"),
            "screen: {screen}"
        );
    }

    #[test]
    fn isearch_backspace_to_match_restores_normal_label() {
        let mut events = vec![ctrl('s')];
        events.extend(key_events("hez")); // "he" matches, "hez" fails
        events.push(key(KeyCode::Backspace)); // back to "he"
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains("I-search: he"), "screen: {screen}");
        assert!(!screen.contains("Failing"), "screen: {screen}");
    }

    #[test]
    fn no_stale_failing_message_after_isearch_accepts() {
        let mut events = vec![ctrl('s')];
        events.extend(key_events("z")); // fails
        events.push(key(KeyCode::Backspace));
        events.extend(key_events("h")); // matches
        events.push(key(KeyCode::Enter)); // accept
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.isearch.is_none());
        assert_eq!(app.editor.minibuffer.message, None);
        let screen = capture_screen(&app.terminal);
        assert!(!screen.contains("Failing"), "screen: {screen}");
    }

    #[test]
    fn mark_set_during_prompt_does_not_reappear_after_finish() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("marked.txt");
        std::fs::write(&file, "content").unwrap();

        let mut events = vec![ctrl('x'), ctrl('f')]; // find-file prompt
        events.push(ctrl(' ')); // SetMark queues "Mark set" while prompt is up
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace)); // clear the default input
        }
        for c in file.to_string_lossy().chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Enter)); // submit

        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "content");
        // The legit post-finish message shows; the stale "Mark set" doesn't.
        assert_eq!(
            app.editor.minibuffer.message,
            Some("Opened marked.txt".to_string())
        );
        let screen = capture_screen(&app.terminal);
        assert!(!screen.contains("Mark set"), "screen: {screen}");
    }

    // === Minibuffer integration tests ===

    #[test]
    fn minibuffer_find_file_via_app() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test_file.txt");
        std::fs::write(&file, "file content").unwrap();

        let mut events = vec![ctrl('x'), ctrl('f')]; // C-x C-f to open find-file
        // Clear the default input (cwd/) with many backspaces
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        // Type the file path
        for c in file.to_string_lossy().chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Enter)); // submit

        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "file content");
    }

    #[test]
    fn minibuffer_navigation_via_app() {
        // Test C-f, C-b, C-a, C-e, Backspace in minibuffer
        let events = vec![
            ctrl('x'), ctrl('f'),    // open find-file prompt
            ctrl('a'),               // go to start of input
            ctrl('e'),               // go to end of input
            ctrl('b'),               // back one char
            ctrl('f'),               // forward one char
            key(KeyCode::Backspace), // delete backward
            ctrl('g'),               // cancel
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(!app.editor.minibuffer.is_active());
    }

    #[test]
    fn minibuffer_tab_completion_via_app() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("unique_file.txt"), "hello").unwrap();

        let mut events = vec![ctrl('x'), ctrl('f')]; // open find-file
        // Clear the default input
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        for c in format!("{}/uni", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab)); // tab complete
        events.push(key(KeyCode::Enter)); // submit
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "hello");
    }

    #[test]
    fn resize_event_handled() {
        let events = vec![Event::Resize(100, 50)];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        // Should not crash — resize is a no-op
        assert!(!app.editor.should_quit);
    }

    // === Multi-pane rendering ===

    #[test]
    fn split_pane_renders_two_mode_lines() {
        let events = vec![ctrl('x'), char_key('2')]; // C-x 2 to split
        let (mut app, mut events) = test_app_with_text(40, 12, "hello\nworld", events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        // Should have two mode lines (both showing *scratch* or similar)
        let mode_line_count = screen.lines().filter(|l| l.contains("--")).count();
        assert!(mode_line_count >= 2, "Expected 2 mode lines, screen:\n{}", screen);
    }

    #[test]
    fn kill_buffer_after_split_does_not_crash() {
        // C-x 2 splits the window (both panes view same buffer), then C-x k kills it.
        // This used to crash because do_kill_buffer only updated the focused pane,
        // leaving the other pane pointing to a deleted buffer.
        let events = vec![
            ctrl('x'), char_key('2'), // split window
            ctrl('x'), char_key('k'), // kill buffer
        ];
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        // Should not crash, and both panes should be valid
        let screen = capture_screen(&app.terminal);
        assert!(!screen.is_empty());
    }

    // === Region rendering ===

    #[test]
    fn region_renders_in_buffer() {
        // Set mark, move forward, then render — should exercise region highlighting
        let events = vec![
            ctrl(' '),               // set mark
            ctrl('f'), ctrl('f'), ctrl('f'), // move forward 3
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        // Verify mark is set and region exists
        assert!(app.editor.region().is_some());
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains("hello"), "Screen should show text: {}", screen);
    }

    #[test]
    fn cx_cs_saves_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();

        let events = vec![
            // Type at beginning
            char_key('X'),
            // C-x C-s to save
            ctrl('x'),
            ctrl('s'),
        ];

        let backend = TestBackend::new(40, 10);
        let terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(terminal, editor);
        let mut event_source = TestEventSource::new(events);
        app.run_until_idle(&mut event_source).unwrap();

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "Xoriginal");
    }

    // === Minibuffer-as-real-buffer integration tests ===

    #[test]
    fn minibuffer_word_movement() {
        // Open prompt, type "hello world", M-b to go back one word
        let mut events = vec![ctrl('x'), ctrl('f')]; // open find-file
        // Clear default input
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.extend(key_events("hello world"));
        events.push(alt(KeyCode::Char('b'))); // backward word
        events.push(ctrl('g'));               // cancel
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        // After M-b from end of "hello world", cursor at 6 (start of "world")
        // But we cancelled, so minibuffer is inactive. Check that it was at 6.
        // We can verify indirectly: cancel restores, buffer text is still "hello"
        assert!(!app.editor.minibuffer.is_active());
    }

    #[test]
    fn minibuffer_kill_line() {
        // Open prompt, type "hello world", C-a then C-k
        let mut events = vec![ctrl('x'), ctrl('f')]; // open find-file
        // Clear default input
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.extend(key_events("hello world"));
        events.push(ctrl('a')); // beginning of line
        events.push(ctrl('k')); // kill line
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "");
        assert_eq!(app.editor.clipboard, "hello world");
    }

    #[test]
    fn minibuffer_delete_forward() {
        // Open prompt, type "hello", C-a then C-d
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.extend(key_events("hello"));
        events.push(ctrl('a')); // beginning
        events.push(ctrl('d')); // delete forward
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "ello");
    }

    #[test]
    fn minibuffer_undo() {
        // Open prompt, type "abc", C-/, verify text is undone
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.extend(key_events("abc"));
        // Move to commit the insert group
        events.push(ctrl('a'));
        events.push(ctrl('/')); // undo
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "");
    }

    #[test]
    fn minibuffer_undo_tab_completion() {
        // Open prompt, type prefix, Tab complete, C-/, verify original prefix restored
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("unique_file.txt"), "").unwrap();

        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        let prefix = format!("{}/uni", dir.path().display());
        for c in prefix.chars() {
            events.push(char_key(c));
        }
        // Commit group before tab
        events.push(ctrl('a'));
        events.push(ctrl('e'));
        events.push(key(KeyCode::Tab)); // complete to full path
        events.push(ctrl('/')); // undo tab completion
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), prefix);
    }

    #[test]
    fn minibuffer_mark_and_cut() {
        // Open prompt, type "hello", C-a, C-SPC, C-f, C-f, C-w
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.extend(key_events("hello"));
        events.push(ctrl('a'));  // beginning
        events.push(ctrl(' ')); // set mark
        events.push(ctrl('f')); // forward
        events.push(ctrl('f')); // forward
        events.push(ctrl('w')); // cut
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "llo");
        assert_eq!(app.editor.clipboard, "he");
    }

    #[test]
    fn minibuffer_paste() {
        // Set clipboard, open prompt, C-y pastes
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        let (mut app, mut events_src) = test_app(40, 10, events);
        app.editor.clipboard = "pasted".to_string();
        app.run_until_idle(&mut events_src).unwrap();
        // Now open prompt is active with empty input. Send C-y.
        let paste_events = vec![ctrl('y')];
        let mut paste_src = TestEventSource::new(paste_events);
        app.run_until_idle(&mut paste_src).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "pasted");
    }

    #[test]
    fn minibuffer_prompt_guard_prevents_nesting() {
        // Open prompt (C-x C-f), then C-x C-f again, verify still in original prompt
        let mut events = vec![ctrl('x'), ctrl('f')]; // first prompt
        events.push(ctrl('x'));
        events.push(ctrl('f')); // try to open another prompt
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        let prompt = app.editor.minibuffer.prompt().unwrap();
        assert_eq!(prompt.kind, crate::minibuffer::PromptKind::FindFile);
    }

    #[test]
    fn minibuffer_isearch_guard() {
        // Open prompt, then C-s should NOT activate isearch
        let events = vec![
            ctrl('x'), ctrl('f'), // open find-file
            ctrl('s'),            // try isearch
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert!(app.editor.isearch.is_none());
    }

    #[test]
    fn minibuffer_kill_buffer_guard() {
        // Open prompt, then C-x k should NOT kill buffer
        let events = vec![
            ctrl('x'), ctrl('f'), // open find-file
            ctrl('x'), char_key('k'), // try kill buffer
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        // Buffer should still be there
        assert_eq!(app.editor.buffers.len(), 1);
    }

    #[test]
    fn minibuffer_quit_guard() {
        // Open prompt, then C-x C-c should NOT quit
        let events = vec![
            ctrl('x'), ctrl('f'), // open find-file
            ctrl('x'), ctrl('c'), // try quit
        ];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(!app.editor.should_quit);
        assert!(app.editor.minibuffer.is_active());
    }

    #[test]
    fn minibuffer_paste_sanitizes_newlines() {
        // Paste multi-line text into minibuffer, newlines become spaces
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.push(Event::Paste("hello\nworld".to_string()));
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "hello world");
    }

    #[test]
    fn minibuffer_editing_doesnt_affect_main_buffer() {
        // Type in main buffer, open prompt, edit in minibuffer, cancel, verify main buffer unchanged
        let mut events = key_events("hello");
        events.push(ctrl('x'));
        events.push(ctrl('f'));
        // Type something in the prompt
        events.extend(key_events("some text"));
        events.push(ctrl('g')); // cancel
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        // Main buffer should still have "hello"
        assert_eq!(app.editor.buffer_text(), "hello");
    }

    #[test]
    fn minibuffer_delete_word_backward() {
        // Open prompt, type "hello world", M-Backspace
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events.extend(key_events("hello world"));
        events.push(alt(KeyCode::Backspace)); // delete word backward
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.is_active());
        assert_eq!(app.editor.minibuffer_text(), "hello ");
    }

    // === Completion list integration tests ===

    fn open_find_file_with_clear() -> Vec<Event> {
        let mut events = vec![ctrl('x'), ctrl('f')];
        for _ in 0..200 {
            events.push(key(KeyCode::Backspace));
        }
        events
    }

    #[test]
    fn tab_with_multiple_matches_shows_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/foo", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_some());
        let completions = app.editor.minibuffer.completions.as_ref().unwrap();
        assert_eq!(completions.len(), 2);
    }

    #[test]
    fn repeated_tab_keeps_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/foo", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_some());
    }

    #[test]
    fn typing_after_tab_dismisses_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/foo", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(char_key('a')); // type a char
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_none());
    }

    #[test]
    fn cg_dismisses_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/foo", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(ctrl('g'));
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_none());
    }

    #[test]
    fn enter_dismisses_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/foob", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Enter));
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_none());
    }

    #[test]
    fn paste_dismisses_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("foobar.txt"), "").unwrap();
        std::fs::write(dir.path().join("foobaz.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/foo", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(Event::Paste("x".to_string()));
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_none());
    }

    #[test]
    fn tab_with_unique_match_no_completions() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("unique.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/uni", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(60, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_none());
    }

    #[test]
    fn completions_render_shows_candidates() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("ants.txt"), "").unwrap();
        std::fs::write(dir.path().join("apple.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/a", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_some());
        let screen = capture_screen(&app.terminal);
        // Completions should appear in the rendered output
        assert!(screen.contains("alpha.txt"), "should show alpha.txt: {}", screen);
        assert!(screen.contains("ants.txt"), "should show ants.txt: {}", screen);
        assert!(screen.contains("apple.txt"), "should show apple.txt: {}", screen);
        // Minibuffer prompt should still be visible
        assert!(screen.contains("Find file:"), "should show prompt: {}", screen);
    }

    #[test]
    fn completions_with_non_ascii_names_in_narrow_terminal_do_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // Candidate names are longer (in bytes and chars) than the terminal
        // width, with multibyte chars at every truncation point.
        std::fs::write(dir.path().join("ééééééééé.txt"), "").unwrap();
        std::fs::write(dir.path().join("ééééééééü.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/é", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(13, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_some());
        // Rendering truncated the names without slicing mid-char.
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains('é'), "screen: {screen}");
    }

    #[test]
    fn completion_page_indicator_with_non_ascii_names_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        // Enough unicode candidates to overflow one page in a tiny terminal.
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("éée{i:02}.txt")), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/é", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Tab)); // advance a page
        let (mut app, mut events) = test_app(14, 8, events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains("[Page"), "screen: {screen}");
    }

    #[test]
    fn multi_column_completions_in_wide_terminal() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            std::fs::write(dir.path().join(format!("a{}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/a", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(80, 24, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completions.is_some());
        let screen = capture_screen(&app.terminal);
        // With 80 cols and short names, multiple candidates should appear on the same line
        // Find a line that contains more than one candidate
        let has_multi = screen.lines().any(|line| {
            let count = (0..10)
                .filter(|i| line.contains(&format!("a{}.txt", i)))
                .count();
            count > 1
        });
        assert!(has_multi, "should show multiple candidates per line:\n{}", screen);
    }

    #[test]
    fn repeated_tab_advances_page() {
        let dir = tempfile::tempdir().unwrap();
        // Create enough files to require paging in a small terminal
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file{:02}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/file", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Tab)); // second tab should advance page
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.minibuffer.completion_page > 0, "page should advance on repeated tab");
    }

    #[test]
    fn repeated_tab_wraps_around() {
        let dir = tempfile::tempdir().unwrap();
        // Create files that will span a small number of pages
        for i in 0..6 {
            std::fs::write(dir.path().join(format!("longfilename{}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/long", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab)); // show completions
        // Tab many times to cycle through all pages and wrap around
        for _ in 0..100 {
            events.push(key(KeyCode::Tab));
        }
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        // The page counter keeps incrementing but render wraps via modulo,
        // so the rendering still works. Verify completions still showing.
        assert!(app.editor.minibuffer.completions.is_some());
    }

    #[test]
    fn typing_after_tab_resets_page() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file{:02}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/file", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Tab)); // advance page
        events.push(char_key('0')); // type a char
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.minibuffer.completion_page, 0);
    }

    #[test]
    fn cg_resets_page() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file{:02}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/file", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Tab)); // advance page
        events.push(ctrl('g'));
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.minibuffer.completion_page, 0);
    }

    #[test]
    fn paste_resets_page() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("file{:02}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/file", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        events.push(key(KeyCode::Tab)); // advance page
        events.push(Event::Paste("x".to_string()));
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.minibuffer.completion_page, 0);
    }

    #[test]
    fn page_indicator_shown_when_multiple_pages() {
        let dir = tempfile::tempdir().unwrap();
        // Create many files with long names so they don't fit in one page
        for i in 0..30 {
            std::fs::write(dir.path().join(format!("longfilename{:02}.txt", i)), "").unwrap();
        }

        let mut events = open_find_file_with_clear();
        for c in format!("{}/long", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(40, 12, events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        assert!(screen.contains("[Page"), "should show page indicator:\n{}", screen);
    }

    #[test]
    fn no_page_indicator_when_single_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
        std::fs::write(dir.path().join("ants.txt"), "").unwrap();

        let mut events = open_find_file_with_clear();
        for c in format!("{}/a", dir.path().display()).chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Tab));
        let (mut app, mut events) = test_app(80, 24, events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        assert!(!screen.contains("[Page"), "should not show page indicator:\n{}", screen);
    }

    // === Mouse click tests ===

    fn mouse_click(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn mouse_scroll_down(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn mouse_scroll_up(x: u16, y: u16) -> Event {
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn mouse_click_places_cursor() {
        // 3-line text. Click on line 2, column 2.
        let text = "hello\nworld\nfoo";
        let events = vec![mouse_click(2, 1)]; // x=2 => col 2 in text
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();

        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 1, "should be on line 1");
        assert_eq!(col, 2, "should be at column 2");
    }

    #[test]
    fn mouse_click_on_wrapped_eol_cursor_row_maps_to_next_line_start() {
        // A 20-char line exactly fills the 20-wide pane; the EOL cursor is
        // drawn at column 0 of the next visual row, which shows the next
        // buffer line. Clicking there maps to the next line's start — the
        // position that row actually displays.
        let text = "aaaaaaaaaaaaaaaaaaaa\nnext";
        let events = vec![mouse_click(0, 1)];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 21); // start of "next"
    }

    #[test]
    fn mouse_click_on_wrapped_eol_cursor_row_after_last_line_maps_to_eol() {
        // Same, but the exactly-full line is the last buffer line: the
        // wrapped cursor row is below all content, so clicking it places
        // point at the end of the buffer — that line's EOL.
        let text = "aaaaaaaaaaaaaaaaaaaa";
        let events = vec![mouse_click(0, 1)];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 20);
    }

    #[test]
    fn mouse_click_on_first_line() {
        let text = "hello\nworld";
        let events = vec![mouse_click(0, 0)]; // x=0 => col 0
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();

        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 0);
        assert_eq!(col, 0);
    }

    #[test]
    fn mouse_click_beyond_line_end_places_cursor_past_last_char() {
        let text = "hi\nworld";
        // Click far right on the first line (line "hi" has 2 chars)
        let events = vec![mouse_click(30, 0)]; // way past end of "hi"
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();

        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 0);
        // Should place cursor past the last char (at col 2 for "hi")
        assert_eq!(col, 2, "col should be past end of line, got {}", col);
    }

    #[test]
    fn mouse_click_beyond_eol_of_form_feed_line_stops_before_break() {
        // FF is a line break: "one" / "two". Clicking way past EOL of
        // line 0 must place point before the FF, not on or after it.
        let text = "one\u{0c}two\n";
        let events = vec![mouse_click(10, 0)];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 3, "point must stop before the FF");
    }

    #[test]
    fn mouse_click_on_line_after_form_feed_break_maps_to_that_line() {
        let text = "one\u{0c}two\n";
        let events = vec![mouse_click(1, 1)];
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 5); // col 1 of "two"
    }

    #[test]
    fn mouse_click_inside_zwj_sequence_snaps_to_cluster_start() {
        // "x" then family emoji (man ZWJ woman ZWJ girl, chars 1..6, one
        // cluster) then "z". Visual col 3 is the first cell of the woman
        // emoji; raw mapping would land at char 3 (between ZWJ and woman),
        // mid-cluster. Point must snap to the cluster start instead.
        let text = "x\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}z";
        let events = vec![mouse_click(3, 0)];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 1, "point must not rest mid-cluster");
    }

    #[test]
    fn lone_cr_line_breaks_render_as_separate_lines() {
        // Old-Mac style file: the \r breaks must not leak into the
        // rendered rows.
        let text = "x\ry\r";
        let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert_eq!(lines[0], "x");
        assert_eq!(lines[1], "y");
    }

    #[test]
    fn mouse_click_after_leading_tab_uses_visual_column() {
        let text = "\tfoo";
        let events = vec![mouse_click(4, 0)]; // visual column 4 is after the tab
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();

        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 0);
        assert_eq!(col, 1, "tab should count as one buffer character");
    }

    #[test]
    fn mouse_click_after_mid_line_tab_uses_visual_column() {
        let text = "a\tfoo";
        let events = vec![mouse_click(4, 0)]; // visual column 4 is after "a\t"
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();

        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 0);
        assert_eq!(col, 2, "tab should count as one buffer character");
    }

    #[test]
    fn mouse_click_wrapped_line_with_tab_uses_visual_column() {
        let text = "abcdef\tgh";
        let events = vec![mouse_click(1, 1)]; // row 1 col 1 is before 'g' after wrapped tab spaces
        let (mut app, mut events) = test_app_with_text(8, 6, text, events);
        app.run_until_idle(&mut events).unwrap();

        let (line, col) = app
            .editor
            .current_buffer()
            .char_to_line_col(app.editor.point());
        assert_eq!(line, 0);
        assert_eq!(col, 7, "click should land before 'g', after the single tab character");
    }

    #[test]
    fn mouse_click_below_content_goes_to_end() {
        let text = "hello";
        // Click on row 5, well below the single line of content
        let events = vec![mouse_click(2, 5)];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();

        assert_eq!(app.editor.point(), text.len());
    }

    #[test]
    fn mouse_click_ignored_when_minibuffer_active() {
        let text = "hello\nworld";
        let mut events = vec![ctrl('x'), ctrl('f')]; // open find-file prompt
        events.push(mouse_click(4, 1)); // click on line 2
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();

        // Minibuffer should still be active
        assert!(app.editor.minibuffer.is_active());
        // Cursor should not have moved (still at 0 since we were in the minibuffer)
        assert_eq!(app.editor.pane_tree.focused_pane().point, 0);
    }

    #[test]
    fn mouse_click_switches_pane_focus() {
        let text = "hello\nworld";
        let mut events = vec![
            ctrl('x'), key(KeyCode::Char('2')), // split horizontal
        ];
        // After split, top pane is focused. Click on the bottom half
        // to switch focus. In a 10-row terminal with 1-row minibuffer,
        // pane area is 9 rows, each pane gets ~4.5 rows.
        // Bottom pane starts around row 5.
        events.push(mouse_click(3, 6));
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();

        // Should have switched focus to the second pane
        let focus = app.editor.pane_tree.focus_path();
        assert_eq!(focus, &[1], "should focus the second pane, got {:?}", focus);
    }

    #[test]
    fn mouse_scroll_down_scrolls_pane() {
        // Create a buffer with enough lines to scroll
        let text = (0..30).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        // Scroll down 3 times over the pane
        let events = vec![
            mouse_scroll_down(5, 3),
            mouse_scroll_down(5, 3),
            mouse_scroll_down(5, 3),
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // scroll_top should have advanced (3 scroll events * 3 lines each = 9)
        assert_eq!(app.editor.pane_tree.focused_pane().scroll_top, 9);
    }

    #[test]
    fn mouse_scroll_up_scrolls_pane() {
        let text = (0..30).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        // First scroll down, then scroll back up
        let events = vec![
            mouse_scroll_down(5, 3),
            mouse_scroll_down(5, 3),
            mouse_scroll_up(5, 3),
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // 2 down (6 lines) - 1 up (3 lines) = 3
        assert_eq!(app.editor.pane_tree.focused_pane().scroll_top, 3);
    }

    #[test]
    fn mouse_scroll_down_hides_cursor_when_point_above_viewport() {
        // Move cursor to line 5 first (C-n * 5), then scroll down past it.
        // With the bug, the cursor snaps to row 0; with the fix, it stays
        // at the last-set position (row 5 from the previous render frame).
        let text = (0..30).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let mut events: Vec<Event> = vec![];
        // Move cursor down 5 lines
        for _ in 0..5 {
            events.push(ctrl('n'));
        }
        // Scroll down past line 5 (3 scroll events * 3 lines = scroll_top 9)
        events.push(mouse_scroll_down(5, 3));
        events.push(mouse_scroll_down(5, 3));
        events.push(mouse_scroll_down(5, 3));

        let (mut app, mut events) = test_app_with_text(40, 10, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // scroll_top=9, point is at line 5 — cursor is above the viewport
        assert_eq!(app.editor.pane_tree.focused_pane().scroll_top, 9);
        let (cursor_line, _) = app.editor.current_buffer().char_to_line_col(app.editor.point());
        assert_eq!(cursor_line, 5);

        // With the fix, the cursor is hidden (not set during draw), so
        // get_cursor_position returns the position from the last frame where
        // the cursor WAS visible (row 5). With the bug, it would be row 0.
        let pos = app.terminal.get_cursor_position().unwrap();
        assert_ne!(
            pos.y, 0,
            "cursor should not appear at row 0 when point is scrolled above the viewport, got {:?}",
            pos
        );
    }

    // === Esc-as-Meta prefix tests ===

    #[test]
    fn esc_less_than_moves_to_buffer_beginning() {
        let text = "hello\nworld\nfoo";
        let events = vec![
            ctrl('e'),           // go to end of first line
            key(KeyCode::Esc),   // Esc prefix
            char_key('<'),       // < — should become M-<
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 0);
    }

    #[test]
    fn esc_greater_than_moves_to_buffer_end() {
        let text = "hello\nworld\nfoo";
        let events = vec![
            key(KeyCode::Esc),   // Esc prefix
            char_key('>'),       // > — should become M->
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 15); // "hello\nworld\nfoo" = 15 chars
    }

    #[test]
    fn esc_f_moves_forward_word() {
        let text = "hello world";
        let events = vec![
            key(KeyCode::Esc),   // Esc prefix
            char_key('f'),       // f — should become M-f
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, text, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.point(), 5); // end of "hello"
    }

    #[test]
    fn esc_shows_pending_indicator() {
        let events = vec![key(KeyCode::Esc)];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert!(app.editor.pending_keys.contains("ESC"));
    }

    #[test]
    fn mouse_scroll_does_not_change_focus() {
        let text = (0..30).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let mut events = vec![
            ctrl('x'), key(KeyCode::Char('2')), // split vertically (top/bottom)
        ];
        // Focus is on top pane [0]. Scroll on the bottom pane area.
        // In 10-row terminal: pane area = 9 rows, each pane ~4-5 rows.
        // Bottom pane starts around row 5.
        events.push(mouse_scroll_down(5, 6));
        let (mut app, mut events) = test_app_with_text(40, 10, &text, events);
        app.run_until_idle(&mut events).unwrap();

        // Focus should still be on the first pane
        let focus = app.editor.pane_tree.focus_path();
        assert_eq!(focus, &[0], "focus should not change on scroll, got {:?}", focus);

        // But the second pane should have scrolled
        let second_pane = app.editor.pane_tree.pane_at_focus_path(&[1]);
        assert!(second_pane.scroll_top > 0, "second pane should have scrolled");
    }

    #[test]
    fn find_file_starts_at_current_buffer_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let file = sub.join("hello.txt");
        std::fs::write(&file, "hello").unwrap();

        // Open the file, then trigger C-x C-f
        let events = vec![ctrl('x'), ctrl('f')];
        let (mut app, mut events) = test_app(60, 10, events);
        app.editor.open_file(&file).unwrap();
        app.run_until_idle(&mut events).unwrap();

        // The minibuffer should start with the file's parent directory
        // (open_file canonicalizes the path, so we must canonicalize too)
        let canonical_sub = std::fs::canonicalize(&sub).unwrap();
        let expected = format!("{}/", canonical_sub.display());
        assert_eq!(app.editor.minibuffer_text(), expected);
    }

    #[test]
    fn find_file_relative_dotdot_resolves_against_editor_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        // Two files with the same name: `../notes.txt` typed while the
        // editor's cwd is `sub` must open the parent directory's copy.
        std::fs::write(dir.path().join("notes.txt"), "parent copy").unwrap();
        std::fs::write(sub.join("notes.txt"), "sub copy").unwrap();

        let mut events = vec![ctrl('x'), ctrl('f')];
        // Clear the pre-filled directory
        for _ in 0..300 {
            events.push(key(KeyCode::Backspace));
        }
        for c in "../notes.txt".chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Enter));

        let (mut app, mut events) = test_app(60, 10, events);
        app.editor.cwd = sub.clone();
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "parent copy");
    }

    #[test]
    fn write_file_relative_dotdot_resolves_against_editor_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();

        let mut events = vec![ctrl('x'), ctrl('w')];
        // Clear the pre-filled directory
        for _ in 0..300 {
            events.push(key(KeyCode::Backspace));
        }
        for c in "../saved.txt".chars() {
            events.push(char_key(c));
        }
        events.push(key(KeyCode::Enter));

        let (mut app, mut events) = test_app_with_text(60, 10, "the text", events);
        app.editor.cwd = sub.clone();
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("saved.txt")).unwrap(),
            "the text"
        );
        assert!(!sub.join("saved.txt").exists());
    }

    #[test]
    fn find_file_falls_back_to_cwd_for_scratch_buffer() {
        // Scratch buffer has no path, so find-file should use cwd
        let events = vec![ctrl('x'), ctrl('f')];
        let (mut app, mut events) = test_app(60, 10, events);
        app.run_until_idle(&mut events).unwrap();

        let expected = format!("{}/", app.editor.cwd.display());
        assert_eq!(app.editor.minibuffer_text(), expected);
    }

    // === Input-state tests ===
    //
    // These pin down how the pending-chord (`C-x ...`) and pending-ESC state
    // interact with the other event kinds: paste and mouse events cancel any
    // pending input (like C-g does), while a resize leaves it alone.

    #[test]
    fn cg_cancelled_chord_key_self_inserts() {
        // After C-g cancels a pending C-x chord, the next key must go through
        // the keymap from the root: 's' self-inserts instead of completing
        // C-x C-s.
        let events = vec![ctrl('x'), ctrl('g'), char_key('s')];
        let (mut app, mut events) = test_app(40, 10, events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "s");
        assert_eq!(app.editor.pending_keys, "");
    }

    #[test]
    fn cg_cancels_pending_esc() {
        // C-g clears a pending ESC prefix: the following 'f' self-inserts
        // instead of running M-f.
        let events = vec![key(KeyCode::Esc), ctrl('g'), char_key('f')];
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "fhello world");
        assert_eq!(app.editor.pending_keys, "");
    }

    #[test]
    fn pending_chord_survives_resize() {
        // A resize must not cancel a chord in progress: C-x <resize> 2 still
        // splits the window.
        let events = vec![ctrl('x'), Event::Resize(40, 10), char_key('2')];
        let (mut app, mut events) = test_app_with_text(40, 12, "hello", events);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let mode_line_count = screen.lines().filter(|l| l.contains("--")).count();
        assert!(
            mode_line_count >= 2,
            "C-x 2 across a resize should split, screen:\n{screen}"
        );
    }

    #[test]
    fn paste_cancels_pending_chord() {
        // A paste event cancels a pending chord: C-x <paste> C-s must NOT
        // complete C-x C-s (save-file); the paste is inserted and the C-s
        // starts an incremental search from the keymap root.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "original").unwrap();

        let mut editor = Editor::new();
        editor.open_file(&file).unwrap();

        let events = vec![ctrl('x'), Event::Paste("Y".to_string()), ctrl('s')];
        let backend = TestBackend::new(40, 10);
        let terminal = Terminal::new(backend).unwrap();
        let mut app = App::new(terminal, editor);
        let mut event_source = TestEventSource::new(events);
        app.run_until_idle(&mut event_source).unwrap();

        assert_eq!(app.editor.buffer_text(), "Yoriginal", "paste was inserted");
        assert!(
            app.editor.isearch.is_some(),
            "C-s after the paste starts isearch instead of completing C-x C-s"
        );
        assert_eq!(app.editor.pending_keys, "", "no pending prefix remains");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "original",
            "file must not have been saved"
        );
    }

    #[test]
    fn mouse_click_cancels_pending_chord() {
        // A mouse click mid-chord cancels the chord AND performs the click
        // (cancel-then-handle): after C-x <click on other pane>, '2' goes
        // through the keymap from the root and self-inserts into the clicked
        // pane instead of completing C-x 2.
        let mut events = vec![ctrl('x'), char_key('2')]; // split first
        events.push(ctrl('x')); // start a chord
        events.push(mouse_click(3, 6)); // click the bottom pane mid-chord
        events.push(char_key('2'));
        let (mut app, mut events) = test_app_with_text(40, 10, "hello\nworld", events);
        app.run_until_idle(&mut events).unwrap();

        assert_eq!(app.editor.pending_keys, "", "the click cancelled the chord");
        assert_eq!(
            app.editor.pane_tree.focus_path(),
            &[1],
            "the click still switched focus to the second pane"
        );
        assert!(
            app.editor.buffer_text().contains('2'),
            "'2' self-inserted instead of completing C-x 2, text: {:?}",
            app.editor.buffer_text()
        );
    }

    #[test]
    fn mouse_click_cancels_pending_esc() {
        // A mouse click cancels a pending ESC: ESC <click> 'f' self-inserts
        // 'f' at the clicked position instead of running M-f (forward-word).
        let events = vec![key(KeyCode::Esc), mouse_click(0, 0), char_key('f')];
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "fhello world");
        assert_eq!(app.editor.point(), 1, "point after the self-inserted 'f'");
    }

    #[test]
    fn paste_cancels_pending_esc() {
        // A paste cancels a pending ESC: ESC <paste> 'f' self-inserts 'f'
        // after the pasted text instead of running M-f.
        let events = vec![
            key(KeyCode::Esc),
            Event::Paste("xy".to_string()),
            char_key('f'),
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        assert_eq!(app.editor.buffer_text(), "xyfhello world");
        assert_eq!(app.editor.point(), 3, "point after the self-inserted 'f'");
    }

    #[test]
    fn paste_during_isearch_extends_query() {
        // Paste during isearch appends to the query (emacs isearch-yank),
        // keeps the minibuffer display in sync, and re-runs the search.
        let events = vec![
            ctrl('s'),
            char_key('w'),
            char_key('o'),
            char_key('r'),
            Event::Paste("ld".to_string()),
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        let isearch = app.editor.isearch.as_ref().expect("isearch still active");
        assert_eq!(isearch.query, "world");
        assert_eq!(app.editor.minibuffer_text(), "world");
        assert_eq!(
            app.editor.point(),
            6,
            "point at the match for the full query"
        );
    }

    #[test]
    fn multiline_paste_during_isearch_normalizes_breaks_to_spaces() {
        // The isearch query is a single line: pasted line breaks (any of
        // \r\n, \r, \n) become spaces, like any minibuffer paste.
        let events = vec![ctrl('s'), Event::Paste("héllo\r\nwörld\nnow".to_string())];
        let (mut app, mut events) = test_app_with_text(40, 10, "say héllo wörld now", events);
        app.run_until_idle(&mut events).unwrap();
        let isearch = app.editor.isearch.as_ref().expect("isearch still active");
        assert_eq!(isearch.query, "héllo wörld now");
        assert_eq!(app.editor.minibuffer_text(), "héllo wörld now");
        assert_eq!(
            app.editor.point(),
            4,
            "point at the match for the pasted query"
        );
    }

    #[test]
    fn backspace_after_isearch_paste_keeps_query_and_display_in_sync() {
        // Backspace pops one char of the query, including chars that
        // arrived via paste; the minibuffer display follows.
        let events = vec![
            ctrl('s'),
            char_key('w'),
            Event::Paste("orld".to_string()),
            key(KeyCode::Backspace),
        ];
        let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
        app.run_until_idle(&mut events).unwrap();
        let isearch = app.editor.isearch.as_ref().expect("isearch still active");
        assert_eq!(isearch.query, "worl");
        assert_eq!(app.editor.minibuffer_text(), "worl");
        assert_eq!(
            app.editor.point(),
            6,
            "point at the match for the refined query"
        );
    }
}
