use super::*;

#[test]
fn isearch_no_match() {
    let mut editor = Editor::new_with_text("hello world");
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("xyz");
    }
    editor.isearch_update();
    // A failing search shows in the prompt label, not as a queued message.
    assert_eq!(editor.minibuffer.message(), None);
    assert!(editor.isearch.as_ref().unwrap().is_failing());
    assert_eq!(
        editor.minibuffer.prompt().unwrap().label(),
        "Failing I-search: "
    );
}

#[test]
fn isearch_next_no_more_matches() {
    let mut editor = Editor::new_with_text("hello world");
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("hello");
    }
    editor.isearch_update();
    assert_eq!(editor.minibuffer.prompt().unwrap().label(), "I-search: ");
    // Try to cycle — no more matches
    editor.isearch_next();
    assert!(editor.isearch.as_ref().unwrap().is_failing());
    assert!(editor
        .minibuffer
        .prompt()
        .unwrap()
        .label()
        .contains("Failing"));
}

#[test]
fn isearch_backward_finds_match() {
    let mut editor = Editor::new_with_text("hello world hello");
    // Move to end
    editor.pane_tree.set_focused_point(17);
    editor.execute(Command::ISearchBackward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("hello");
    }
    editor.isearch_update();
    // rfind from position 17 finds the last "hello" before cursor = position 12
    assert_eq!(editor.point(), 12);
}

#[test]
fn isearch_next_backward() {
    let mut editor = Editor::new_with_text("ab ab ab");
    editor.pane_tree.set_focused_point(8);
    editor.execute(Command::ISearchBackward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("ab");
    }
    editor.isearch_update();
    // First backward match from end
    let first = editor.point();
    editor.isearch_next();
    // Should find an earlier match
    assert!(
        editor.point() < first
            || editor
                .minibuffer
                .message()
                .as_ref()
                .unwrap()
                .contains("Failing")
    );
}

#[test]
fn isearch_empty_query_restores() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.set_focused_point(5);
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("world");
    }
    editor.isearch_update();
    assert_eq!(editor.point(), 6);
    // Clear query → should restore
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("");
    }
    editor.isearch_update();
    assert_eq!(editor.point(), 5);
}

#[test]
fn buffer_names_list() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    let names = editor.buffer_names();
    assert!(names.contains(&"*scratch*".to_string()));
    assert!(names.contains(&"test.txt".to_string()));
}

#[test]
fn undo_restores_unmodified_state() {
    let mut editor = Editor::new_with_text("hello");
    assert!(!editor.current_buffer().is_modified());
    editor.execute(Command::InsertChar('X'));
    assert!(editor.current_buffer().is_modified());
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert!(
        !editor.current_buffer().is_modified(),
        "Buffer should be unmodified after undoing to original state"
    );
}

#[test]
fn undo_redo_preserves_modified_after_save() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    assert!(!editor.current_buffer().is_modified());

    // Make an edit and save
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Save);
    assert!(!editor.current_buffer().is_modified());

    // Make another edit
    editor.execute(Command::InsertChar('Y'));
    assert!(editor.current_buffer().is_modified());

    // Undo back to saved state
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert!(
        !editor.current_buffer().is_modified(),
        "Should be unmodified after undoing to last save point"
    );
}

// === Indentation tests ===

#[test]
fn insert_newline_copies_indentation_across_rope_chunks() {
    let indentation = " ".repeat(2_000);
    let source = format!("{indentation}value");
    let mut editor = Editor::new_with_text(&source);
    assert!(editor.current_buffer().text().chunks().count() > 1);
    editor.pane_tree.set_focused_point(source.chars().count());

    editor.execute(Command::InsertNewline);

    assert_eq!(editor.buffer_text(), format!("{source}\n{indentation}"));
}

#[test]
fn insert_newline_copies_indentation() {
    let mut editor = Editor::new_with_text("    hello");
    // Place cursor at end of "hello"
    editor.pane_tree.set_focused_point(9);
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "    hello\n    ");
    assert_eq!(editor.point(), 14); // after the 4 spaces on new line
}

#[test]
fn insert_newline_no_indent_at_column_zero() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(5);
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "hello\n");
    assert_eq!(editor.point(), 6);
}

#[test]
fn insert_newline_mid_line_preserves_indent() {
    let mut editor = Editor::new_with_text("    helloworld");
    editor.pane_tree.set_focused_point(9); // between "hello" and "world"
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "    hello\n    world");
    assert_eq!(editor.point(), 14);
}

#[test]
fn indent_line_single() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    hello");
    assert_eq!(editor.point(), 6); // 2 + 4
}

#[test]
fn indent_line_with_region() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    // Select all three lines
    editor
        .pane_tree
        .set_focused_point_mark_and_preferred(0, Some(11), None); // end of "ccc"
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    aaa\n    bbb\n    ccc");
}

#[test]
fn dedent_line_single() {
    let mut editor = Editor::new_with_text("    hello");
    editor.pane_tree.set_focused_point(6); // on 'l'
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2); // 6 - 4
}

#[test]
fn dedent_line_partial_spaces() {
    let mut editor = Editor::new_with_text("  hello");
    editor.pane_tree.set_focused_point(4);
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2); // 4 - 2
}

#[test]
fn dedent_line_no_leading_spaces() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2); // unchanged
}

#[test]
fn dedent_line_with_region() {
    let mut editor = Editor::new_with_text("    aaa\n    bbb\n    ccc");
    editor
        .pane_tree
        .set_focused_point_mark_and_preferred(0, Some(23), None); // end of text
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
}

#[test]
fn region_end_at_col0_excludes_last_line() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    // Region covers first two lines, but end is at start of "ccc"
    editor
        .pane_tree
        .set_focused_point_mark_and_preferred(0, Some(8), None); // start of "ccc" line (col 0)
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    aaa\n    bbb\nccc");
}

#[test]
fn undo_reverses_indent() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    hello");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn undo_reverses_region_indent() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    editor
        .pane_tree
        .set_focused_point_mark_and_preferred(0, Some(11), None);
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    aaa\n    bbb\n    ccc");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
}

#[test]
fn preferred_column_cleared_after_indent() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_preferred_column(Some(5));
    editor.execute(Command::IndentLine);
    assert_eq!(editor.pane_tree.focused_pane().preferred_column(), None);
}

#[test]
fn preferred_column_cleared_after_dedent() {
    let mut editor = Editor::new_with_text("    hello");
    editor.pane_tree.set_focused_preferred_column(Some(5));
    editor.pane_tree.set_focused_point(6);
    editor.execute(Command::DedentLine);
    assert_eq!(editor.pane_tree.focused_pane().preferred_column(), None);
}

// === Consecutive kill-line accumulation tests ===

#[test]
fn kill_line_consecutive_accumulates() {
    // "hello\nworld\n" — three C-k's from the start should accumulate
    let mut editor = Editor::new_with_text("hello\nworld\n");
    // C-k 1: kills "hello" (rest of line), clipboard = "hello"
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello");
    // C-k 2: kills "\n" (at EOL), clipboard = "hello\n"
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello\n");
    // C-k 3: kills "world" (rest of line), clipboard = "hello\nworld"
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello\nworld");
}

#[test]
fn kill_line_non_consecutive_resets() {
    let mut editor = Editor::new_with_text("hello\nworld");
    // First C-k: kills "hello"
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello");
    // Non-kill command breaks the chain
    editor.execute(Command::ForwardChar);
    // After killing "hello", buffer is "\nworld", point is at 0.
    // ForwardChar moves to pos 1 ('w'). Kill_line kills "world".
    // Since last_command was ForwardChar, clipboard is replaced.
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "world");
}

#[test]
fn kill_line_accumulate_then_paste() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    // Kill first two segments: "aaa" then "\n"
    editor.execute(Command::KillLine);
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "aaa\n");
    // Paste should insert the accumulated text
    editor.execute(Command::Paste);
    assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
}

#[test]
fn noop_kill_line_touches_neither_clipboard() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello");
    assert_eq!(editor.os_clipboard.last_set_text.as_deref(), Some("hello"));

    // C-k at end of buffer kills nothing: neither the internal clipboard
    // nor the OS clipboard (which another program may own by now) may be
    // written.
    editor.os_clipboard.last_set_text = None;
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello");
    assert_eq!(editor.os_clipboard.last_set_text, None);
}

#[test]
fn noop_kill_line_does_not_count_as_a_kill_for_appending() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::KillLine);
    // A C-k that killed nothing must not keep the kill chain armed
    // (a real kill chain is otherwise broken only by another command).
    editor.execute(Command::KillLine);
    assert_eq!(editor.last_command, None);
    assert_eq!(editor.clipboard, "hello");
}

#[test]
fn kill_chain_does_not_survive_prompt_submit() {
    // The review repro: C-k in a find-file prompt, Enter, C-k in the opened
    // buffer must not append the buffer line to the killed prompt text.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "file line\nrest").unwrap();

    let mut editor = Editor::new();
    editor.execute(Command::FindFile);
    editor.set_minibuffer_text("junk-path");
    editor.execute(Command::BeginningOfLine);
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "junk-path");

    editor.set_minibuffer_text(&file.to_string_lossy());
    editor.submit_prompt();
    assert!(!editor.minibuffer.is_active());

    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "file line");
}

#[test]
fn kill_chain_does_not_survive_isearch_accept() {
    let mut editor = Editor::new_with_text("hello there\nworld line");
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello there");

    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.set_query_for_test("world");
    }
    editor.isearch_update();
    editor.isearch_accept();
    assert_eq!(editor.last_command, None);

    // Point is at the start of the match; C-k kills the rest of that line
    // and must not append it to the pre-search kill.
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "world line");
}

#[test]
fn kill_chain_does_not_survive_prompt_cancel() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.execute(Command::GotoLine);
    editor.set_minibuffer_text("abc");
    editor.execute(Command::BeginningOfLine);
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "abc");

    // C-g: cancel the prompt, then C-k in the buffer must not append.
    editor.execute(Command::Cancel);
    assert!(!editor.minibuffer.is_active());
    editor.execute(Command::KillLine);
    assert_eq!(editor.clipboard, "hello");
}
