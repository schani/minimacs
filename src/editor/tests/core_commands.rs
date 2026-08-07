use super::*;

#[test]
fn forward_char_moves_one() {
    let mut editor = Editor::new_with_text("hello");
    assert_eq!(editor.point(), 0);
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 1);
}

#[test]
fn forward_char_stops_at_end() {
    let mut editor = Editor::new_with_text("hi");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 2);
}

#[test]
fn backward_char_moves_one() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(3);
    editor.execute(Command::BackwardChar);
    assert_eq!(editor.point(), 2);
}

#[test]
fn backward_char_stops_at_start() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::BackwardChar);
    assert_eq!(editor.point(), 0);
}

#[test]
fn next_line_basic() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 8);
}

#[test]
fn next_line_clamps_to_shorter_line() {
    let mut editor = Editor::new_with_text("hello\nhi");
    editor.pane_tree.set_focused_point(4);
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 8);
}

#[test]
fn next_line_preserves_preferred_column() {
    let mut editor = Editor::new_with_text("hello\nhi\nworld");
    editor.pane_tree.set_focused_point(4);
    editor.execute(Command::NextLine);
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 13);
}

#[test]
fn previous_line_basic() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(8);
    editor.execute(Command::PreviousLine);
    assert_eq!(editor.point(), 2);
}

#[test]
fn beginning_of_line() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(8);
    editor.execute(Command::BeginningOfLine);
    assert_eq!(editor.point(), 6);
}

#[test]
fn end_of_line() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(6);
    editor.execute(Command::EndOfLine);
    assert_eq!(editor.point(), 11);
}

#[test]
fn insert_char_basic() {
    let mut editor = Editor::new_with_text("hllo");
    editor.pane_tree.set_focused_point(1);
    editor.execute(Command::InsertChar('e'));
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2);
}

#[test]
fn insert_newline() {
    let mut editor = Editor::new_with_text("helloworld");
    editor.pane_tree.set_focused_point(5);
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "hello\nworld");
    assert_eq!(editor.point(), 6);
}

#[test]
fn delete_backward_basic() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(3);
    editor.execute(Command::DeleteBackward);
    assert_eq!(editor.buffer_text(), "helo");
    assert_eq!(editor.point(), 2);
}

#[test]
fn delete_backward_at_start_does_nothing() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::DeleteBackward);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 0);
}

#[test]
fn delete_forward_basic() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "helo");
    assert_eq!(editor.point(), 2);
}

#[test]
fn delete_forward_at_end_does_nothing() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.set_focused_point(5);
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn scroll_follows_cursor_down() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk");
    editor.pane_tree.set_focused_viewport_height(3);
    editor.pane_tree.set_focused_scroll_top(0);
    for _ in 0..8 {
        editor.execute(Command::NextLine);
    }
    let (line, _) = editor.current_buffer().char_to_line_col(editor.point());
    assert_eq!(line, 8);
    let pane = editor.pane_tree.focused_pane();
    assert!(pane.scroll_top() <= line);
    assert!(line < pane.scroll_top() + pane.viewport_height());
}

#[test]
fn undo_reverses_insert() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('a'));
    editor.execute(Command::InsertChar('b'));
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "");
}

#[test]
fn undo_redo_roundtrip() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('x'));
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "");
    editor.execute(Command::Redo);
    assert_eq!(editor.buffer_text(), "x");
}

#[test]
fn undo_reverses_delete() {
    let mut editor = Editor::new_with_text("abc");
    editor.pane_tree.set_focused_point(3);
    editor.execute(Command::DeleteBackward);
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "abc");
}

#[test]
fn kill_line_from_middle() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "he\nworld");
    assert_eq!(editor.clipboard, "llo");
}

#[test]
fn kill_line_at_eol() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(5);
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "helloworld");
    assert_eq!(editor.clipboard, "\n");
}

#[test]
fn buffer_beginning_and_end() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.set_focused_point(5);
    editor.execute(Command::BufferBeginning);
    assert_eq!(editor.point(), 0);
    editor.execute(Command::BufferEnd);
    assert_eq!(editor.point(), 11);
}

#[test]
fn page_down_and_up() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.set_focused_viewport_height(3);
    editor.execute(Command::PageDown);
    let (line, _) = editor.current_buffer().char_to_line_col(editor.point());
    assert_eq!(line, 3);
    editor.execute(Command::PageUp);
    let (line, _) = editor.current_buffer().char_to_line_col(editor.point());
    assert_eq!(line, 0);
}

// === Recenter tests ===

#[test]
fn recenter_centers_cursor_line() {
    // 10 lines, viewport height 5, cursor on line 5
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.set_focused_viewport_height(5);
    editor.pane_tree.set_focused_viewport_width(40);
    // Move cursor to line 5 (the "f" line)
    for _ in 0..5 {
        editor.execute(Command::NextLine);
    }
    editor.execute(Command::RecenterTopBottom);
    // Center: scroll_top = 5 - 5/2 = 3
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 3);
}

#[test]
fn recenter_cycles_center_top_bottom() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.set_focused_viewport_height(5);
    editor.pane_tree.set_focused_viewport_width(40);
    // Move cursor to line 5
    for _ in 0..5 {
        editor.execute(Command::NextLine);
    }
    // First C-l: center (scroll_top = 3)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 3);
    // Second C-l: top (scroll_top = 5)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 5);
    // Third C-l: bottom (scroll_top = 5 - 4 = 1)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 1);
    // Fourth C-l: center again (scroll_top = 3)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 3);
}

#[test]
fn recenter_resets_on_other_command() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.set_focused_viewport_height(5);
    editor.pane_tree.set_focused_viewport_width(40);
    for _ in 0..5 {
        editor.execute(Command::NextLine);
    }
    // First C-l: center
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 3);
    // Any other command resets the cycle
    editor.execute(Command::ForwardChar);
    // Next C-l should be center again, not top
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 3);
}

#[test]
fn recenter_at_beginning_of_buffer() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne");
    editor.pane_tree.set_focused_viewport_height(5);
    editor.pane_tree.set_focused_viewport_width(40);
    // Cursor at line 0
    editor.execute(Command::RecenterTopBottom);
    // Center: 0.saturating_sub(2) = 0
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 0);
    // Top: scroll_top = 0
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 0);
    // Bottom: 0.saturating_sub(4) = 0
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top(), 0);
}

#[test]
fn save_buffer_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Save);

    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "Xoriginal");
}

#[test]
fn save_buffer_without_path_prompts() {
    let mut editor = Editor::new();
    editor.execute(Command::Save);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn find_file_opens_prompt() {
    let mut editor = Editor::new();
    editor.execute(Command::FindFile);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn switch_buffer_opens_prompt() {
    let mut editor = Editor::new();
    editor.execute(Command::SwitchBuffer);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn open_same_file_twice_switches() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    let first_id = editor.pane_tree.focused_pane().buffer_id();

    editor.open_file(&file).unwrap();
    assert_eq!(editor.pane_tree.focused_pane().buffer_id(), first_id);
    assert_eq!(editor.buffers.len(), 2);
}

#[test]
fn kill_last_buffer_creates_scratch() {
    let mut editor = Editor::new();
    assert_eq!(editor.buffers.len(), 1);
    editor.do_kill_buffer(0);
    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(editor.current_buffer().name(), "*scratch*");
}

#[test]
fn quit_with_unmodified_buffers() {
    let mut editor = Editor::new();
    editor.execute(Command::Quit);
    assert!(editor.should_quit);
}

#[test]
fn quit_with_modified_buffer_prompts() {
    let mut editor = Editor::new();
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::Quit);
    assert!(!editor.should_quit);
    assert!(editor.minibuffer.is_active());
}
