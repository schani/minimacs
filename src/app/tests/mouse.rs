use super::*;

#[test]
fn quit_save_after_mouse_focus_marks_saved_undo_version_clean() {
    let dir = tempfile::tempdir().unwrap();
    let file_a = dir.path().join("a.txt");
    let file_b = dir.path().join("b.txt");
    std::fs::write(&file_a, "aaa").unwrap();
    std::fs::write(&file_b, "bbb").unwrap();

    // Put A in the left pane and B in the right pane. Unlike a keyboard
    // command, mouse focus does not commit the edit group of the pane it
    // leaves, which is the important part of this reproduction.
    let mut editor = Editor::new();
    editor.open_file(&file_a).unwrap();
    editor.execute(crate::command::Command::SplitHorizontal);
    editor.execute(crate::command::Command::CycleFocus);
    editor.open_file(&file_b).unwrap();
    editor.execute(crate::command::Command::CycleFocus);

    let events = vec![
        char_key('A'),
        mouse_click(25, 0),
        char_key('B'),
        ctrl('x'),
        ctrl('c'),
        char_key('y'),
        key(KeyCode::Enter),
        char_key('q'),
        key(KeyCode::Enter),
        mouse_click(0, 0),
        ctrl('/'),
    ];
    let backend = TestBackend::new(40, 10);
    let terminal = Terminal::new(backend).unwrap();
    let mut app = App::new(terminal, editor);
    let mut events = TestEventSource::new(events);
    app.run_until_idle(&mut events).unwrap();

    assert!(!app.editor.should_quit, "q cancels while handling B");
    assert_eq!(std::fs::read_to_string(&file_a).unwrap(), "Aaaa");
    assert_eq!(
        app.editor.current_buffer().path().as_deref(),
        Some(std::fs::canonicalize(&file_a).unwrap().as_path())
    );
    assert_eq!(
        app.editor.buffer_text(),
        "aaa",
        "undo removes A's saved edit"
    );
    assert!(
        app.editor.current_buffer().is_modified(),
        "after undo, A differs from its saved contents and must be modified"
    );
}

#[test]
fn mouse_click_in_multiline_idle_message_does_not_hit_pane() {
    let text = (0..20)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut app, _) = test_app_with_text(8, 9, &text, vec![]);
    app.editor
        .minibuffer
        .show_message("a message that wraps over rows".to_string());
    let mut events = TestEventSource::new(vec![mouse_click(2, 6)]);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.point(), 0, "message rows are outside pane input");
}

#[test]
fn mouse_scroll_in_grown_prompt_or_completions_does_not_scroll_pane() {
    let text = (0..30)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (mut app, _) = test_app_with_text(12, 12, &text, vec![]);
    app.editor.execute(crate::command::Command::FindFile);
    app.editor
        .minibuffer_buffer
        .reset_transient_text("a/very/long/prompt/value");
    app.editor.minibuffer_pane.point = app.editor.minibuffer_buffer.char_count();
    app.editor.minibuffer.completions = Some(vec!["alpha".into(), "alpine".into()]);
    let mut events = TestEventSource::new(vec![mouse_scroll_down(2, 8)]);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.pane_tree.focused_pane().scroll_top, 0);
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
fn mouse_click_beyond_eol_of_form_feed_line_lands_at_eol() {
    // FF is content: "one\u{0c}two" is one line of seven chars.
    // Clicking way past its EOL clamps to the end of the line's text.
    let text = "one\u{0c}two\n";
    let events = vec![mouse_click(10, 0)];
    let (mut app, mut events) = test_app_with_text(20, 6, text, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.point(), 7, "point lands after \"two\"");
}

#[test]
fn mouse_click_on_row_below_form_feed_line_maps_past_newline() {
    // Row 1 is the empty final line after the \n (the FF broke nothing).
    let text = "one\u{0c}two\n";
    let events = vec![mouse_click(1, 1)];
    let (mut app, mut events) = test_app_with_text(20, 6, text, events);
    app.run_until_idle(&mut events).unwrap();
    assert_eq!(app.editor.point(), 8);
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
    assert_eq!(
        col, 7,
        "click should land before 'g', after the single tab character"
    );
}

#[test]
fn mouse_click_maps_atomic_wide_wrap_row_to_third_glyph() {
    // Width 6 reserves the fifth content cell rather than splitting the
    // third CJK glyph. The second visual row therefore begins at buffer
    // column 2, and clicking its first cell must land there.
    let events = vec![mouse_click(0, 1)];
    let (mut app, mut events) = test_app_with_text(6, 5, "你你你你", events);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.point(), 2);
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
        ctrl('x'),
        key(KeyCode::Char('2')), // split horizontal
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
    let text = (0..30)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
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
    let text = (0..30)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
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
    let text = (0..30)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
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
    let (cursor_line, _) = app
        .editor
        .current_buffer()
        .char_to_line_col(app.editor.point());
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

#[test]
fn mouse_scroll_does_not_change_focus() {
    let text = (0..30)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    let mut events = vec![
        ctrl('x'),
        key(KeyCode::Char('2')), // split vertically (top/bottom)
    ];
    // Focus is on top pane [0]. Scroll on the bottom pane area.
    // In 10-row terminal: pane area = 9 rows, each pane ~4-5 rows.
    // Bottom pane starts around row 5.
    events.push(mouse_scroll_down(5, 6));
    let (mut app, mut events) = test_app_with_text(40, 10, &text, events);
    app.run_until_idle(&mut events).unwrap();

    // Focus should still be on the first pane
    let focus = app.editor.pane_tree.focus_path();
    assert_eq!(
        focus,
        &[0],
        "focus should not change on scroll, got {:?}",
        focus
    );

    // But the second pane should have scrolled
    let second_pane = app.editor.pane_tree.pane_at_focus_path(&[1]);
    assert!(
        second_pane.scroll_top > 0,
        "second pane should have scrolled"
    );
}
