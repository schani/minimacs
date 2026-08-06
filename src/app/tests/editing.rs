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
    assert!(app.editor.should_quit());
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
fn cy_paste_is_its_own_undo_group_after_typing() {
    let (mut app, _) = test_app(40, 10, vec![]);
    app.editor.set_clipboard_for_test("hé");
    let mut events = TestEventSource::new(vec![char_key('x'), ctrl('y'), ctrl('/')]);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.buffer_text(), "x");
    assert_eq!(app.editor.point(), 1);
}

#[test]
fn bracketed_paste_matches_cy_for_unicode_normalization_and_point() {
    let supplied = "hé\r\n你";
    let (mut bracketed, mut events) = test_app(40, 10, vec![Event::Paste(supplied.to_string())]);
    bracketed.run_until_idle(&mut events).unwrap();

    let (mut yanked, _) = test_app(40, 10, vec![]);
    yanked.editor.set_clipboard_for_test(supplied);
    let mut events = TestEventSource::new(vec![ctrl('y')]);
    yanked.run_until_idle(&mut events).unwrap();

    assert_eq!(bracketed.editor.buffer_text(), "hé\n你");
    assert_eq!(bracketed.editor.buffer_text(), yanked.editor.buffer_text());
    assert_eq!(bracketed.editor.point(), 4);
    assert_eq!(bracketed.editor.point(), yanked.editor.point());
}

#[test]
fn large_multiline_bracketed_paste_keeps_cursor_visible_and_resets_goal_column() {
    let pasted = (0..40)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut app, _) = test_app(20, 6, vec![]);
    app.editor.set_focused_preferred_column_for_test(Some(17));
    let mut events = TestEventSource::new(vec![Event::Paste(pasted)]);
    app.run_until_idle(&mut events).unwrap();

    let pane = app.editor.pane_tree().focused_pane();
    let (cursor_line, _) = app.editor.current_buffer().char_to_line_col(pane.point());
    assert_eq!(cursor_line, 39);
    assert!(
        pane.scroll_top() > 0,
        "large paste must reveal its final line"
    );
    assert_eq!(pane.preferred_column(), None);
    let screen = capture_screen(&app.terminal);
    assert!(screen.contains("line 39"), "screen: {screen}");
}

#[test]
fn resize_event_handled() {
    let events = vec![Event::Resize(100, 50)];
    let (mut app, mut events) = test_app(40, 10, events);
    app.run_until_idle(&mut events).unwrap();
    // Should not crash — resize is a no-op
    assert!(!app.editor.should_quit());
}

#[test]
fn resize_applies_new_viewport_before_revealing_cursor() {
    let text = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
    app.editor.set_focused_point(text.chars().count());
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.pane_tree().focused_pane().scroll_row_offset(), 0);

    app.terminal.backend_mut().resize(8, 6);
    let mut events = TestEventSource::new(vec![Event::Resize(8, 6)]);
    app.run_until_idle(&mut events).unwrap();

    let pane = app.editor.pane_tree().focused_pane();
    assert_eq!(pane.viewport_width(), 8);
    assert!(
        pane.scroll_row_offset() > 0,
        "cursor must be reflowed into the narrower viewport"
    );
    let cursor = app.terminal.get_cursor_position().unwrap();
    assert!(
        cursor.y < 5,
        "cursor must remain in the text viewport: {cursor:?}"
    );
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
