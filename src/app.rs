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

        // If minibuffer is active, route keys there
        if self.editor.minibuffer.is_active() {
            self.handle_minibuffer_key(key);
            return;
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
                // If search direction is backward, switch to forward
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.direction = SearchDirection::Forward;
                }
                self.editor.isearch_next();
                // Update the prompt label
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
                    if let Some(p) = self.editor.minibuffer.prompt_mut() {
                        p.input = isearch.query.clone();
                        p.cursor = p.input.len();
                    }
                }
                self.editor.isearch_update();
            }
            // Printable char: add to query and search
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                if let Some(ref mut isearch) = self.editor.isearch {
                    isearch.query.push(c);
                    if let Some(p) = self.editor.minibuffer.prompt_mut() {
                        p.input = isearch.query.clone();
                        p.cursor = p.input.len();
                    }
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

    fn handle_minibuffer_key(&mut self, key: KeyEvent) {
        match (key.modifiers, key.code) {
            (KeyModifiers::NONE, KeyCode::Enter) => {
                self.editor.submit_prompt();
            }
            (KeyModifiers::NONE, KeyCode::Backspace) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.delete_backward();
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('f')) | (KeyModifiers::NONE, KeyCode::Right) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.forward_char();
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('b')) | (KeyModifiers::NONE, KeyCode::Left) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.backward_char();
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('a')) | (KeyModifiers::NONE, KeyCode::Home) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.beginning();
                }
            }
            (KeyModifiers::CONTROL, KeyCode::Char('e')) | (KeyModifiers::NONE, KeyCode::End) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.end();
                }
            }
            (KeyModifiers::NONE, KeyCode::Tab) => {
                self.handle_minibuffer_tab();
            }
            (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.insert_char(c);
                }
            }
            _ => {}
        }
    }

    fn handle_minibuffer_tab(&mut self) {
        let kind = self.editor.minibuffer.prompt().map(|p| p.kind.clone());
        match kind {
            Some(PromptKind::FindFile) | Some(PromptKind::WriteFile) => {
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.complete_path();
                }
            }
            Some(PromptKind::SwitchBuffer) => {
                let names = self.editor.buffer_names();
                if let Some(p) = self.editor.minibuffer.prompt_mut() {
                    p.complete_buffer(&names);
                }
            }
            _ => {}
        }
    }

    fn handle_paste(&mut self, text: &str) {
        // Insert pasted text as a single undo group
        self.editor.current_buffer_mut().history.commit();
        let point = self.editor.pane_tree.focused_pane().point;
        let buf = self.editor.current_buffer_mut();
        buf.history.record_insert(point, text);
        buf.insert(point, text);
        let new_point = point + text.chars().count();
        self.editor.pane_tree.focused_pane_mut().point = new_point;
        self.editor.current_buffer_mut().history.commit();
    }

    fn update_viewport(&mut self) {
        let size = self.terminal.size().unwrap_or_default();
        // Calculate the pane area (full area minus 1 row for minibuffer)
        let pane_area = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: size.width,
            height: size.height.saturating_sub(1),
        };

        let pane_rects = self.editor.pane_tree.calculate_rects(pane_area);
        for (path, rect) in &pane_rects {
            // Each pane rect includes 1 row for mode line
            let text_height = rect.height.saturating_sub(1) as usize;
            let text_width = rect.width as usize;
            self.editor.pane_tree.update_pane_viewport(path, text_height, text_width);
        }
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
}
