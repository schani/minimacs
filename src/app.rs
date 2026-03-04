use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::Backend;
use ratatui::Terminal;

use crate::command::Command;
use crate::editor::Editor;
use crate::event::EventSource;
use crate::keymap::{KeymapResult, KeymapState, default_keymap};
use crate::minibuffer::PromptKind;
use crate::render;

pub struct App<B: Backend> {
    pub editor: Editor,
    pub terminal: Terminal<B>,
    keymap_state: KeymapState,
}

impl<B: Backend> App<B>
where
    B::Error: Send + Sync + 'static,
{
    pub fn new(terminal: Terminal<B>, editor: Editor) -> Self {
        Self {
            editor,
            terminal,
            keymap_state: KeymapState::new(default_keymap()),
        }
    }

    pub fn run(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        loop {
            if let Some(event) = event_source.next_event() {
                match event {
                    Event::Key(key_event) => {
                        self.handle_key(key_event);
                        if self.editor.should_quit {
                            break;
                        }
                    }
                    Event::Paste(text) => {
                        self.handle_paste(&text);
                    }
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }

            self.update_viewport();
            self.render()?;

            if self.editor.should_quit {
                break;
            }
        }
        Ok(())
    }

    /// Run until all events are consumed (for tests).
    #[allow(dead_code)]
    pub fn run_until_idle(&mut self, event_source: &mut dyn EventSource) -> Result<()> {
        self.update_viewport();
        self.render()?;

        while let Some(event) = event_source.next_event() {
            match event {
                Event::Key(key_event) => {
                    self.handle_key(key_event);
                    if self.editor.should_quit {
                        break;
                    }
                }
                Event::Paste(text) => {
                    self.handle_paste(&text);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
            self.update_viewport();
            self.render()?;
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // C-g always cancels
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.keymap_state.clear();
            self.editor.pending_keys.clear();
            self.editor.execute(Command::Cancel);
            return;
        }

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
                    // Fall through to normal keymap processing
                }
            }
        }

        match self.keymap_state.process_key(key) {
            KeymapResult::Matched(cmd) => {
                self.editor.pending_keys.clear();
                self.editor.execute(cmd);
            }
            KeymapResult::Pending => {
                self.editor.pending_keys = self.keymap_state.pending_display();
            }
            KeymapResult::NotFound => {
                self.editor.pending_keys.clear();
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

        let kind = self.editor.minibuffer.prompt().map(|p| p.kind.clone());
        let input = self.editor.minibuffer_text();
        let (completed, candidates) = match kind {
            Some(PromptKind::FindFile) | Some(PromptKind::WriteFile) => {
                complete_path_with_candidates(&input)
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
            let buf = &mut self.editor.minibuffer_buffer;
            buf.history.commit();
            let old_len = buf.char_count();
            buf.history.record_replace(0, &input, &completed);
            buf.remove(0, old_len);
            buf.insert(0, &completed);
            buf.history.commit();
            self.editor.minibuffer_pane.point = completed.chars().count();
        }
    }

    fn handle_paste(&mut self, text: &str) {
        // Sanitize: replace newlines with spaces when pasting into minibuffer
        let text = if self.editor.minibuffer.is_active() {
            self.editor.minibuffer.completions = None;
            text.replace("\r\n", " ").replace('\n', " ")
        } else {
            text.to_string()
        };
        // Insert pasted text as a single undo group
        self.editor.active_buffer_mut().history.commit();
        let point = self.editor.active_pane().point;
        let buf = self.editor.active_buffer_mut();
        buf.history.record_insert(point, &text);
        buf.insert(point, &text);
        let new_point = point + text.chars().count();
        self.editor.active_pane_mut().point = new_point;
        self.editor.active_buffer_mut().history.commit();
    }

    fn update_viewport(&mut self) {
        let size = self.terminal.size().unwrap_or_default();
        let comp_height = render::completions_height(&self.editor, size.height);
        // Calculate the pane area (full area minus 1 row for minibuffer minus completions)
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1 + comp_height),
        };

        let pane_rects = self.editor.pane_tree.calculate_rects(pane_area);
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
    use crate::event::TestEventSource;
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
        // Gutter for 1 line: 2 chars ("1 ")
        // Available text width: 18
        // Chars per wrapped visual line: 17 (18 - 1 for '\')
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars > 17
        let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // First visual line: gutter + 17 chars + "\"
        assert!(
            lines[0].ends_with('\\'),
            "Expected continuation marker '\\', got: '{}'",
            lines[0]
        );
        // Second visual line: blank gutter + remaining 9 chars "rstuvwxyz"
        assert!(
            lines[1].contains("rstuvwxyz"),
            "Expected wrapped text 'rstuvwxyz', got: '{}'",
            lines[1]
        );
    }

    #[test]
    fn continuation_line_has_blank_gutter() {
        // Same setup as above
        let text = "abcdefghijklmnopqrstuvwxyz"; // 26 chars
        let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // First line starts with "1 " (line number)
        assert!(
            lines[0].starts_with("1 "),
            "Expected line number, got: '{}'",
            lines[0]
        );
        // Continuation line starts with "  " (blank gutter)
        assert!(
            lines[1].starts_with("  "),
            "Expected blank gutter on continuation line, got: '{}'",
            lines[1]
        );
        // But the continuation line should NOT start with a digit
        assert!(
            !lines[1].trim_start().starts_with(|c: char| c.is_ascii_digit()),
            "Continuation line should not have a line number: '{}'",
            lines[1]
        );
    }

    #[test]
    fn wrapped_line_uses_multiple_visual_rows() {
        // Terminal: 20 wide, 8 tall (6 text rows + 1 mode line + 1 minibuffer)
        // Two buffer lines, first one wraps into 2 visual lines
        let text = "abcdefghijklmnopqrstuvwxyz\nshort";
        let (mut app, mut events) = test_app_with_text(20, 8, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // Line 1 visual row 1: "1 abcdefghijklmnopq\" (gutter 2, 17 chars, \)
        // Line 1 visual row 2: "  rstuvwxyz" (blank gutter, remaining)
        // Line 2 visual row 1: "2 short"
        assert!(
            lines[0].contains("abcdefg"),
            "First visual line should have start of long line: '{}'",
            lines[0]
        );
        assert!(
            lines[2].starts_with("2 "),
            "Second buffer line should appear on 3rd visual row with line number '2': '{}'",
            lines[2]
        );
        assert!(
            lines[2].contains("short"),
            "Second buffer line should contain 'short': '{}'",
            lines[2]
        );
    }

    #[test]
    fn cursor_on_wrapped_portion() {
        // Place cursor at char 20 of a long line, which is on the wrapped part
        // Terminal: 20 wide, 6 tall
        // Gutter: 2, text width: 18, chars per wrap: 17
        let text = "abcdefghijklmnopqrstuvwxyz";
        let events = vec![ctrl('e')]; // go to end of line (char 25)
        let (mut app, mut events) = test_app_with_text(20, 6, text, events);
        app.run_until_idle(&mut events).unwrap();

        // The cursor should be on visual row 1 (the wrapped part),
        // not on visual row 0
        let _buf = app.terminal.backend().buffer();
        // Check that the cursor was set (we can verify point is at end)
        // "abcdefghijklmnopqrstuvwxyz" has 26 chars, C-e goes to position 26
        assert_eq!(app.editor.point(), 26);

        // Verify the wrapped text renders correctly
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        assert!(lines[0].ends_with('\\'), "Line should wrap: '{}'", lines[0]);
    }

    #[test]
    fn triple_wrap_line() {
        // A very long line that wraps 3 times
        // Terminal: 15 wide, 8 tall (6 text rows + mode + minibuf)
        // Gutter for 1 line: 2 chars
        // Text width: 13, chars per wrap: 12
        let text = "abcdefghijklmnopqrstuvwxyz0123456789"; // 36 chars
        // wraps into ceil(36/12) = 3 visual lines
        let (mut app, mut events) = test_app_with_text(15, 8, text, vec![]);
        app.run_until_idle(&mut events).unwrap();
        let screen = capture_screen(&app.terminal);
        let lines: Vec<&str> = screen.lines().collect();
        // Visual line 0: "1 " + 12 chars + "\"
        assert!(lines[0].ends_with('\\'), "First wrap: '{}'", lines[0]);
        // Visual line 1: "  " + 12 chars + "\"
        assert!(lines[1].ends_with('\\'), "Second wrap: '{}'", lines[1]);
        // Visual line 2: "  " + remaining 12 chars (no \)
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
}
