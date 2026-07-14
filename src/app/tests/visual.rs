use super::*;

/// Manual performance benchmark for the long-line rendering path. Kept out
/// of normal test runs because timing assertions are machine-dependent:
///
/// cargo test --release benchmark_five_megabyte_single_line -- --ignored --nocapture
#[test]
#[ignore = "manual performance benchmark"]
fn benchmark_five_megabyte_single_line() {
    use std::time::{Duration, Instant};

    const BYTES: usize = 5 * 1024 * 1024;
    const ITERATIONS: u32 = 5;
    let prefix = "{\"data\":\"";
    let suffix = "\"}";
    let text = format!(
        "{prefix}{}{suffix}",
        "a".repeat(BYTES - prefix.len() - suffix.len())
    );
    assert_eq!(text.len(), BYTES);
    assert!(!text.contains('\n'));

    let (mut app, mut events) = test_app_with_text(120, 40, &text, vec![]);
    app.editor.buffers[0].syntax =
        crate::syntax::SyntaxState::new(crate::syntax::Language::Json);
    let cold_start = Instant::now();
    app.run_until_idle(&mut events).unwrap();
    let cold_start = cold_start.elapsed();

    let repeated_start = Instant::now();
    for _ in 0..ITERATIONS {
        app.render().unwrap();
    }
    let repeated_start = repeated_start.elapsed() / ITERATIONS;

    let end = app.editor.current_buffer().char_count();
    app.editor.pane_tree.focused_pane_mut().point = end;
    let end_layout = Instant::now();
    app.editor.ensure_cursor_visible();
    let end_layout = end_layout.elapsed();

    let repeated_end = Instant::now();
    for _ in 0..ITERATIONS {
        app.render().unwrap();
    }
    let repeated_end = repeated_end.elapsed() / ITERATIONS;

    let mut backward_command = Duration::ZERO;
    let mut backward_interaction = Duration::ZERO;
    for _ in 0..ITERATIONS {
        app.editor.pane_tree.focused_pane_mut().point = end;
        let command = Instant::now();
        app.editor.execute(crate::command::Command::BackwardChar);
        backward_command += command.elapsed();

        app.editor.pane_tree.focused_pane_mut().point = end;
        let interaction = Instant::now();
        app.editor.execute(crate::command::Command::BackwardChar);
        app.render().unwrap();
        backward_interaction += interaction.elapsed();
    }
    let backward_command = backward_command / ITERATIONS;
    let backward_interaction = backward_interaction / ITERATIONS;

    eprintln!(
        "5 MiB single line: cold-start={cold_start:.2?}, repeated-start={repeated_start:.2?}, end-layout={end_layout:.2?}, repeated-end={repeated_end:.2?}, backward-command={backward_command:.2?}, backward-interaction={backward_interaction:.2?}"
    );
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
fn wide_glyph_wrap_boundary_is_atomic_through_the_app() {
    // Exact regression for a five-cell continued-row budget: the third
    // double-width glyph cannot straddle the marker boundary. Four CJK
    // glyphs render as 2 + marker, then 2; C-e lands after the final glyph.
    let events = vec![ctrl('e')];
    let (mut app, mut events) = test_app_with_text(6, 5, "你你你你", events);
    app.run_until_idle(&mut events).unwrap();

    let screen = capture_screen(&app.terminal);
    let rows = screen.lines().take(2).collect::<Vec<_>>();
    let first: String = rows[0].chars().filter(|ch| *ch != ' ').collect();
    let second: String = rows[1].chars().filter(|ch| *ch != ' ').collect();
    assert_eq!(first, "你你\\");
    assert_eq!(second, "你你");
    assert!(
        rows[1].starts_with('你'),
        "second row must start with a glyph, not its empty continuation cell: {:?}",
        rows[1]
    );

    let pos = app.terminal.get_cursor_position().unwrap();
    assert_eq!((pos.x, pos.y), (4, 1));
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

#[test]
fn lone_cr_is_content_and_renders_on_one_row() {
    // Old-Mac style content: \r is not a line break, so "x" and "y"
    // share the first row. Control characters are rendered visibly rather
    // than being passed through to the terminal.
    let text = "x\ry\r";
    let (mut app, mut events) = test_app_with_text(20, 6, text, vec![]);
    app.run_until_idle(&mut events).unwrap();
    let screen = capture_screen(&app.terminal);
    let lines: Vec<&str> = screen.lines().collect();
    assert!(
        lines[0].contains('x') && lines[0].contains('y'),
        "x and y must share row 0, got {:?}",
        lines[0]
    );
    assert_eq!(lines[0].matches('␍').count(), 2);
    assert!(
        !lines[1].contains('y'),
        "nothing must spill to row 1, got {:?}",
        lines[1]
    );
}

#[test]
fn terminal_control_sequences_are_rendered_as_visible_text() {
    let payload = "safe\u{1b}]52;c;clipboard\u{7}tail\u{85}";
    let (mut app, mut events) = test_app_with_text(80, 6, payload, vec![]);
    app.editor.buffers[0].name = "name\u{1b}]0;owned\u{7}.txt".to_string();

    app.run_until_idle(&mut events).unwrap();

    let screen = capture_screen(&app.terminal);
    assert!(
        !screen.chars().any(|ch| ch != '\n' && ch.is_control()),
        "got {screen:?}"
    );
    assert!(screen.contains("safe␛]52;c;clipboard␇tail�"), "got {screen:?}");
    assert!(screen.contains("name␛]0;owned␇.txt"), "got {screen:?}");
}
