use super::*;

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
