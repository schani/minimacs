use super::*;

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
fn meta_angle_brackets_move_to_buffer_ends_in_kitty_form() {
    // Option-as-meta under the kitty keyboard protocol without alternate-key
    // reporting delivers M-> as the base key '.' with ALT|SHIFT (and M-< as
    // ',' with ALT|SHIFT), not as the shifted character.
    let alt_shift = |c: char| {
        Event::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::ALT | KeyModifiers::SHIFT,
        ))
    };
    let (mut app, mut events) = test_app_with_text(40, 10, "hello\nworld", vec![alt_shift('.')]);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.point(), 11, "M-> must move point to buffer end");

    let (mut app, mut events) = test_app_with_text(
        40,
        10,
        "hello\nworld",
        vec![key(KeyCode::End), alt_shift(',')],
    );
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(
        app.editor.point(),
        0,
        "M-< must move point to buffer beginning"
    );
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
fn tab_inserts_four_spaces() {
    let events = vec![key(KeyCode::Tab)];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.buffer_text(), "    ");
    assert_eq!(app.editor.point(), 4);
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
    assert!(
        mode_line_count >= 2,
        "Expected 2 mode lines, screen:\n{}",
        screen
    );
}

#[test]
fn kill_buffer_after_split_does_not_crash() {
    // C-x 2 splits the window (both panes view same buffer), then C-x k kills it.
    // This used to crash because do_kill_buffer only updated the focused pane,
    // leaving the other pane pointing to a deleted buffer.
    let events = vec![
        ctrl('x'),
        char_key('2'), // split window
        ctrl('x'),
        char_key('k'), // kill buffer
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
        ctrl(' '), // set mark
        ctrl('f'),
        ctrl('f'),
        ctrl('f'), // move forward 3
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, "hello world", events);
    app.run_until_idle(&mut events).unwrap();
    // Verify mark is set and region exists
    assert!(app.editor.region().is_some());
    let screen = capture_screen(&app.terminal);
    assert!(
        screen.contains("hello"),
        "Screen should show text: {}",
        screen
    );
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
