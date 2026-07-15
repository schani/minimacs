use super::*;

#[test]
fn isearch_forward_basic() {
    let mut editor = Editor::new_with_text("hello world hello");
    editor.execute(Command::ISearchForward);
    assert!(editor.isearch.is_some());
    assert!(editor.minibuffer.is_active());

    // Type "world" into search
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "world".to_string();
    }
    editor.isearch_update();
    // Should find "world" at char position 6
    assert_eq!(editor.point(), 6);
}

#[test]
fn isearch_snapshots_a_multi_chunk_buffer_once() {
    let prefix = "λ".repeat(2_000);
    let source = format!("{prefix} needle終");
    let mut editor = Editor::new_with_text(&source);
    assert!(editor.current_buffer().text.chunks().count() > 1);

    editor.execute(Command::ISearchForward);
    assert_eq!(editor.isearch.as_ref().unwrap().text_snapshot, source);

    editor.isearch.as_mut().unwrap().query = "needle終".to_string();
    editor.isearch_update();
    assert_eq!(editor.point(), prefix.chars().count() + 1);
}

#[test]
fn isearch_backward_basic() {
    let mut editor = Editor::new_with_text("hello world hello");
    // Start at end
    editor.pane_tree.focused_pane_mut().point = 17;
    editor.execute(Command::ISearchBackward);
    assert!(editor.isearch.is_some());

    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "hello".to_string();
    }
    editor.isearch_update();
    // Should find "hello" at position 12 (second occurrence, backward from 17)
    assert_eq!(editor.point(), 12);
}

#[test]
fn isearch_cancel_restores_position() {
    let mut editor = Editor::new_with_text("hello world");
    assert_eq!(editor.point(), 0);
    editor.execute(Command::ISearchForward);

    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "world".to_string();
    }
    editor.isearch_update();
    assert_eq!(editor.point(), 6); // Found at "world"

    // Cancel should restore
    editor.execute(Command::Cancel);
    assert_eq!(editor.point(), 0);
    assert!(editor.isearch.is_none());
}

#[test]
fn isearch_accept_keeps_position() {
    let mut editor = Editor::new_with_text("hello world");
    editor.execute(Command::ISearchForward);

    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "world".to_string();
    }
    editor.isearch_update();
    assert_eq!(editor.point(), 6);

    editor.isearch_accept();
    assert_eq!(editor.point(), 6); // Position kept
    assert!(editor.isearch.is_none());
}

#[test]
fn isearch_next_cycles() {
    let mut editor = Editor::new_with_text("aaa bbb aaa bbb aaa");
    editor.execute(Command::ISearchForward);

    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "aaa".to_string();
    }
    editor.isearch_update();
    assert_eq!(editor.point(), 0); // First "aaa"

    editor.isearch_next();
    assert_eq!(editor.point(), 8); // Second "aaa"

    editor.isearch_next();
    assert_eq!(editor.point(), 16); // Third "aaa"
}

#[test]
fn isearch_matches_returns_all() {
    let mut editor = Editor::new_with_text("abcabcabc");
    editor.execute(Command::ISearchForward);

    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "abc".to_string();
    }
    editor.isearch_update();

    let matches = editor.isearch_matches();
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0], (0, 3));
    assert_eq!(matches[1], (3, 3));
    assert_eq!(matches[2], (6, 3));
}

// === Word movement tests ===

#[test]
fn forward_word_basic() {
    let mut editor = Editor::new_with_text("hello world");
    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), 5); // End of "hello"
}

#[test]
fn forward_word_skips_non_word() {
    let mut editor = Editor::new_with_text("hello   world");
    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), 5); // End of "hello"
    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), 13); // End of "world"
}

#[test]
fn forward_word_at_end() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), 5); // Stays at end
}

#[test]
fn forward_word_with_underscore() {
    let mut editor = Editor::new_with_text("foo_bar baz");
    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), 7); // End of "foo_bar"
}

#[test]
fn backward_word_basic() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 6); // Start of "world"
}

#[test]
fn backward_word_skips_non_word() {
    let mut editor = Editor::new_with_text("hello   world");
    editor.pane_tree.focused_pane_mut().point = 13;
    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 8); // Start of "world"
    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 0); // Start of "hello"
}

#[test]
fn backward_word_at_start() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 0); // Stays at start
}

#[test]
fn backward_word_with_underscore() {
    let mut editor = Editor::new_with_text("foo_bar baz");
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 8); // Start of "baz"
    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 0); // Start of "foo_bar"
}

#[test]
fn word_commands_cross_rope_chunks() {
    let punctuation = ".".repeat(2_000);
    let source = format!("{punctuation}λ_word");
    let mut editor = Editor::new_with_text(&source);
    assert!(
        editor.current_buffer().text.chunks().count() > 1,
        "fixture must span rope chunks"
    );

    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), source.chars().count());

    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), punctuation.chars().count());

    editor.pane_tree.focused_pane_mut().point = source.chars().count();
    editor.execute(Command::DeleteWordBackward);
    assert_eq!(editor.buffer_text(), punctuation);
}

#[test]
fn word_commands_treat_decomposed_graphemes_as_atomic_word_text() {
    let source = "!e\u{301}x";
    let mut editor = Editor::new_with_text(source);

    editor.execute(Command::ForwardWord);
    assert_eq!(editor.point(), source.chars().count());

    editor.execute(Command::BackwardWord);
    assert_eq!(editor.point(), 1);

    editor.pane_tree.focused_pane_mut().point = source.chars().count();
    editor.execute(Command::DeleteWordBackward);
    assert_eq!(editor.buffer_text(), "!");
    assert_eq!(editor.point(), 1);
}

// === Delete word backward tests ===

#[test]
fn delete_word_backward_basic() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::DeleteWordBackward);
    assert_eq!(editor.buffer_text(), "hello ");
    assert_eq!(editor.point(), 6);
}

#[test]
fn delete_word_backward_with_spaces() {
    let mut editor = Editor::new_with_text("hello   world");
    editor.pane_tree.focused_pane_mut().point = 13;
    editor.execute(Command::DeleteWordBackward);
    assert_eq!(editor.buffer_text(), "hello   ");
    assert_eq!(editor.point(), 8);
}

#[test]
fn delete_word_backward_at_start() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::DeleteWordBackward);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 0);
}

// === Non-existent file tests ===
