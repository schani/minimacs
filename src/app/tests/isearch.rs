use super::*;

// === Isearch integration tests ===

#[test]
fn isearch_reveal_matches_normal_cursor_reveal_geometry() {
    let text = format!("{}\t你好e\u{301} needle", "ascii".repeat(30));
    let match_pos = text.find("needle").unwrap();
    let match_char = text[..match_pos].chars().count();

    let (mut normal, mut normal_events) = test_app_with_text(12, 6, &text, vec![]);
    normal.run_until_idle(&mut normal_events).unwrap();
    normal.editor.pane_tree.set_focused_point(match_char);
    normal.editor.ensure_cursor_visible();
    let normal_scroll = {
        let pane = normal.editor.pane_tree.focused_pane();
        (pane.scroll_top(), pane.scroll_row_offset())
    };

    // Keep the prompt on one row so both cases have the same pane viewport;
    // this test compares only their wrapped-line reveal geometry.
    let events = vec![ctrl('s'), Event::Paste("n".to_string())];
    let (mut searching, mut search_events) = test_app_with_text(12, 6, &text, events);
    searching.run_until_idle(&mut search_events).unwrap();
    let search_scroll = {
        let pane = searching.editor.pane_tree.focused_pane();
        (pane.scroll_top(), pane.scroll_row_offset())
    };

    assert!(searching.editor.minibuffer.is_active());
    assert_eq!(searching.editor.point(), match_char);
    assert_eq!(search_scroll, normal_scroll);
}

#[test]
fn isearch_match_remains_visible_when_wrapped_line_reflows_after_resize() {
    let text = format!("{}needle", "x".repeat(46));
    let match_pos = 46;
    let events = vec![ctrl('s'), Event::Paste("needle".to_string())];
    let (mut app, mut events) = test_app_with_text(20, 6, &text, events);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.point(), match_pos);
    assert!(app.editor.isearch.is_some());
    let original_offset = app.editor.pane_tree.focused_pane().scroll_row_offset();

    app.terminal.backend_mut().resize(10, 6);
    let mut events = TestEventSource::new(vec![Event::Resize(10, 6)]);
    app.run_until_idle(&mut events).unwrap();

    let pane = app.editor.pane_tree.focused_pane();
    assert!(
        pane.scroll_row_offset() > original_offset,
        "the narrower wrapping must reveal the selected match"
    );
    let screen = capture_screen(&app.terminal);
    assert!(
        screen.contains("needle"),
        "active match must be visible: {screen}"
    );
}

#[test]
fn ordinary_prompt_reflow_does_not_scroll_the_underlying_pane() {
    let text = "x".repeat(58);
    let (mut app, mut events) = test_app_with_text(20, 6, &text, vec![]);
    app.editor.pane_tree.set_focused_point(text.len());
    app.run_until_idle(&mut events).unwrap();
    app.editor.ensure_cursor_visible();
    let original_scroll = {
        let pane = app.editor.pane_tree.focused_pane();
        (pane.scroll_top(), pane.scroll_row_offset())
    };

    let mut events = TestEventSource::new(vec![
        ctrl('x'),
        char_key('b'),
        char_key('a'),
        char_key('b'),
        char_key('c'),
        char_key('d'),
    ]);
    app.run_until_idle(&mut events).unwrap();

    assert!(app.editor.minibuffer.is_active());
    assert!(app.editor.isearch.is_none());
    let pane = app.editor.pane_tree.focused_pane();
    assert_eq!(
        (pane.scroll_top(), pane.scroll_row_offset()),
        original_scroll,
        "editing an ordinary prompt must not reveal the underlying pane"
    );
}

#[test]
fn isearch_forward_via_app() {
    let text = "hello world hello";
    let events = vec![
        ctrl('s'),           // start isearch
        char_key('h'),       // type 'h'
        char_key('e'),       // type 'e'
        char_key('l'),       // type 'l'
        char_key('l'),       // type 'l'
        char_key('o'),       // type 'o'
        ctrl('s'),           // cycle to next match
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
    events.push(ctrl('r')); // start backward isearch
    events.extend(key_events("hello"));
    events.push(key(KeyCode::Enter)); // accept
    let (mut app, mut events) = test_app_with_text(40, 10, text, events);
    app.run_until_idle(&mut events).unwrap();
    // rfind from position 17 finds the last "hello" before that = position 12
    assert_eq!(app.editor.point(), 12);
}

#[test]
fn isearch_forward_past_final_match_retains_failing_label() {
    let events = vec![ctrl('s'), char_key('a'), ctrl('s'), ctrl('s')];
    let (mut app, mut events) = test_app_with_text(40, 10, "a a", events);
    app.run_until_idle(&mut events).unwrap();

    let screen = capture_screen(&app.terminal);
    assert!(screen.contains("Failing I-search: a"), "screen: {screen}");
    assert_eq!(app.editor.point(), 2, "the final match remains selected");
}

#[test]
fn isearch_backward_past_final_match_retains_failing_label() {
    let events = vec![
        key(KeyCode::End),
        ctrl('r'),
        char_key('a'),
        ctrl('r'),
        ctrl('r'),
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, "a a", events);
    app.run_until_idle(&mut events).unwrap();

    let screen = capture_screen(&app.terminal);
    assert!(
        screen.contains("Failing I-search backward: a"),
        "screen: {screen}"
    );
    assert_eq!(app.editor.point(), 0, "the final match remains selected");
}

#[test]
fn isearch_backspace_refines_query() {
    let text = "abc abcd abcde";
    let events = vec![
        ctrl('s'),
        char_key('a'),
        char_key('b'),
        char_key('c'),
        char_key('d'),
        char_key('e'),
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
        char_key('w'),
        char_key('o'),
        char_key('r'),
        char_key('l'),
        char_key('d'),
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
        char_key('w'),
        char_key('o'),
        char_key('r'),
        char_key('l'),
        char_key('d'),
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
    assert_eq!(app.editor.minibuffer.message(), None);
    let screen = capture_screen(&app.terminal);
    assert!(!screen.contains("Failing"), "screen: {screen}");
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
    assert_eq!(isearch.query(), "world");
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
    assert_eq!(isearch.query(), "héllo wörld now");
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
    assert_eq!(isearch.query(), "worl");
    assert_eq!(app.editor.minibuffer_text(), "worl");
    assert_eq!(
        app.editor.point(),
        6,
        "point at the match for the refined query"
    );
}

#[test]
fn isearch_backspace_removes_one_grapheme_cluster() {
    let events = vec![
        ctrl('s'),
        Event::Paste("e\u{301}".to_string()),
        key(KeyCode::Backspace),
    ];
    let (mut app, mut events) = test_app_with_text(40, 10, "e\u{301}x", events);
    app.run_until_idle(&mut events).unwrap();

    let isearch = app.editor.isearch.as_ref().expect("isearch still active");
    assert_eq!(isearch.query(), "");
    assert_eq!(app.editor.minibuffer_text(), "");
    assert_eq!(app.editor.point(), 0);
}
