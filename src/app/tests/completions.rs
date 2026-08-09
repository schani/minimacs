use super::*;

// === Completion list integration tests ===
fn open_find_file_with_clear() -> Vec<Event> {
    let mut events = vec![ctrl('x'), ctrl('f')];
    for _ in 0..200 {
        events.push(key(KeyCode::Backspace));
    }
    events
}

#[test]
fn mx_tab_reuses_minibuffer_completion_for_command_names() {
    let mut events = vec![alt(KeyCode::Char('x'))];
    events.extend(key_events("delete-"));
    events.push(key(KeyCode::Tab));
    let (mut app, mut events) = test_app(60, 12, events);
    app.run_until_idle(&mut events).unwrap();

    let completions = app.editor.minibuffer().completions().unwrap();
    assert!(completions.contains(&"delete-backward-char".to_string()));
    assert!(completions.contains(&"delete-forward-char".to_string()));
    assert_eq!(app.editor.minibuffer_text(), "delete-");
}

#[test]
fn mx_tab_uniquely_completes_command_name() {
    let mut events = vec![alt(KeyCode::Char('x'))];
    events.extend(key_events("describe-b"));
    events.push(key(KeyCode::Tab));
    let (mut app, mut events) = test_app(60, 12, events);
    app.run_until_idle(&mut events).unwrap();

    assert_eq!(app.editor.minibuffer_text(), "describe-bindings");
    assert!(app.editor.minibuffer().completions().is_none());
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
    assert!(app.editor.minibuffer().completions().is_some());
    let completions = app.editor.minibuffer().completions().unwrap();
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
    assert!(app.editor.minibuffer().completions().is_some());
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
    assert!(app.editor.minibuffer().completions().is_none());
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
    assert!(app.editor.minibuffer().completions().is_none());
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
    assert!(app.editor.minibuffer().completions().is_none());
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
    assert!(app.editor.minibuffer().completions().is_none());
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
    assert!(app.editor.minibuffer().completions().is_none());
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
    assert!(app.editor.minibuffer().completions().is_some());
    let screen = capture_screen(&app.terminal);
    // Completions should appear in the rendered output
    assert!(
        screen.contains("alpha.txt"),
        "should show alpha.txt: {}",
        screen
    );
    assert!(
        screen.contains("ants.txt"),
        "should show ants.txt: {}",
        screen
    );
    assert!(
        screen.contains("apple.txt"),
        "should show apple.txt: {}",
        screen
    );
    // Minibuffer prompt should still be visible
    assert!(
        screen.contains("Find file:"),
        "should show prompt: {}",
        screen
    );
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
    assert!(app.editor.minibuffer().completions().is_some());
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
fn completions_with_wide_names_align_columns_by_display_width() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["zaa.txt", "zbb.txt", "zcc.txt", "z你你你你你你.txt"] {
        std::fs::write(dir.path().join(name), "").unwrap();
    }

    let mut events = open_find_file_with_clear();
    for c in format!("{}/z", dir.path().display()).chars() {
        events.push(char_key(c));
    }
    events.push(key(KeyCode::Tab));
    let (mut app, mut events) = test_app(40, 12, events);
    app.run_until_idle(&mut events).unwrap();
    assert!(app.editor.minibuffer().completions().is_some());
    let screen = capture_screen(&app.terminal);
    // The widest candidate "z你你你你你你.txt" is 17 display columns
    // (11 chars), so col_width = 19 and 40 columns fit 2 columns of the
    // 4 sorted candidates (column-major: rows are [zaa, zcc], [zbb, z你…]).
    // The second column must start at display column 19 in both rows.
    // (capture_screen dumps a wide char's continuation cell as a space,
    // so string index == display column.)
    let row0 = screen
        .lines()
        .find(|l| l.contains("zaa.txt"))
        .expect("row with zaa.txt");
    assert_eq!(row0.find("zcc.txt"), Some(19), "row: {row0:?}");
    let row1 = screen
        .lines()
        .find(|l| l.contains("zbb.txt"))
        .expect("row with zbb.txt");
    assert_eq!(row1.find("z你"), Some(19), "row: {row1:?}");
}

#[test]
fn completion_page_indicator_flush_right_with_wide_names() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..30 {
        std::fs::write(dir.path().join(format!("你你e{i:02}.txt")), "").unwrap();
    }

    let mut events = open_find_file_with_clear();
    for c in format!("{}/你", dir.path().display()).chars() {
        events.push(char_key(c));
    }
    events.push(key(KeyCode::Tab));
    let (mut app, mut events) = test_app(14, 8, events);
    app.run_until_idle(&mut events).unwrap();
    let screen = capture_screen(&app.terminal);
    // 30 candidates, 1 column x 2 visible rows => 15 pages. The splice
    // budget before "[Page 1/15]" (11 columns) is 3 columns, which fits
    // only "你" (2 columns) plus one pad space, so the indicator lands
    // flush against the right edge without splitting a glyph.
    let line = screen
        .lines()
        .find(|l| l.contains("[Page"))
        .expect("indicator line");
    assert!(line.ends_with("[Page 1/15]"), "line: {line:?}");
    assert_eq!(line.chars().count(), 14, "flush right: {line:?}");
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
    assert!(app.editor.minibuffer().completions().is_some());
    let screen = capture_screen(&app.terminal);
    // With 80 cols and short names, multiple candidates should appear on the same line
    // Find a line that contains more than one candidate
    let has_multi = screen.lines().any(|line| {
        let count = (0..10)
            .filter(|i| line.contains(&format!("a{}.txt", i)))
            .count();
        count > 1
    });
    assert!(
        has_multi,
        "should show multiple candidates per line:\n{}",
        screen
    );
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
    assert!(
        app.editor.minibuffer().completion_page() > 0,
        "page should advance on repeated tab"
    );
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
    assert!(app.editor.minibuffer().completions().is_some());
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
    assert_eq!(app.editor.minibuffer().completion_page(), 0);
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
    assert_eq!(app.editor.minibuffer().completion_page(), 0);
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
    assert_eq!(app.editor.minibuffer().completion_page(), 0);
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
    assert!(
        screen.contains("[Page"),
        "should show page indicator:\n{}",
        screen
    );
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
    assert!(
        !screen.contains("[Page"),
        "should not show page indicator:\n{}",
        screen
    );
}
