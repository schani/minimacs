use super::*;

// Explicit imports for everything the tests use, so this file does not
// silently borrow `app.rs`'s private `use` list via `use super::*` (which
// would let later import pruning in app.rs break the tests at a distance).
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::editor::Editor;
use crate::event::{EventSource, Poll, TestEventSource};

mod completions;
mod editing;
mod input_state;
mod isearch;
mod minibuffer;
mod mouse;
mod visual;

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
fn render_with_content() {
    let (mut app, mut events) =
        test_app_with_text(40, 10, "line one\nline two\nline three", vec![]);
    app.run_until_idle(&mut events).unwrap();
    let screen = capture_screen(&app.terminal);
    insta::assert_snapshot!(screen);
}

#[test]
fn background_syntax_completion_is_applied_without_an_input_event() {
    use std::time::{Duration, Instant};

    let (mut app, _) = test_app_with_text(40, 10, "fn main() {}\n", vec![]);
    app.editor
        .enable_buffer_syntax_for_test(0, crate::syntax::Language::Rust);
    app.update_viewport();
    app.render().unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !app.apply_syntax_completions() {
        assert!(
            Instant::now() < deadline,
            "background syntax completion timed out"
        );
        std::thread::yield_now();
    }

    let syntax = app.editor.buffers()[0].syntax().unwrap();
    let cached = syntax.background_spans(0..2, app.editor.buffers()[0].edit_generation());
    assert!(cached.exact);
    assert!(!cached.spans.is_empty());
}

// Shared mouse-event helpers (used by the visual, input_state, and mouse
// topic modules).
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
