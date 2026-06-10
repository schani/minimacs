use super::*;
use crate::minibuffer::PromptKind;

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
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 2);
}

#[test]
fn backward_char_moves_one() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 3;
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
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 8);
}

#[test]
fn next_line_clamps_to_shorter_line() {
    let mut editor = Editor::new_with_text("hello\nhi");
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 8);
}

#[test]
fn next_line_preserves_preferred_column() {
    let mut editor = Editor::new_with_text("hello\nhi\nworld");
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.execute(Command::NextLine);
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 13);
}

#[test]
fn previous_line_basic() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.focused_pane_mut().point = 8;
    editor.execute(Command::PreviousLine);
    assert_eq!(editor.point(), 2);
}

#[test]
fn beginning_of_line() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.focused_pane_mut().point = 8;
    editor.execute(Command::BeginningOfLine);
    assert_eq!(editor.point(), 6);
}

#[test]
fn end_of_line() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.focused_pane_mut().point = 6;
    editor.execute(Command::EndOfLine);
    assert_eq!(editor.point(), 11);
}

#[test]
fn insert_char_basic() {
    let mut editor = Editor::new_with_text("hllo");
    editor.pane_tree.focused_pane_mut().point = 1;
    editor.execute(Command::InsertChar('e'));
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2);
}

#[test]
fn insert_newline() {
    let mut editor = Editor::new_with_text("helloworld");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "hello\nworld");
    assert_eq!(editor.point(), 6);
}

#[test]
fn insert_tab_inserts_four_spaces() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertTab);
    assert_eq!(editor.buffer_text(), "    ");
    assert_eq!(editor.point(), 4);
}

#[test]
fn delete_backward_basic() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 3;
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
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "helo");
    assert_eq!(editor.point(), 2);
}

#[test]
fn delete_forward_at_end_does_nothing() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn scroll_follows_cursor_down() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk");
    editor.pane_tree.focused_pane_mut().viewport_height = 3;
    editor.pane_tree.focused_pane_mut().scroll_top = 0;
    for _ in 0..8 {
        editor.execute(Command::NextLine);
    }
    let (line, _) = editor.current_buffer().char_to_line_col(editor.point());
    assert_eq!(line, 8);
    let pane = editor.pane_tree.focused_pane();
    assert!(pane.scroll_top <= line);
    assert!(line < pane.scroll_top + pane.viewport_height);
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
    editor.pane_tree.focused_pane_mut().point = 3;
    editor.execute(Command::DeleteBackward);
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "abc");
}

#[test]
fn kill_line_from_middle() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "he\nworld");
    assert_eq!(editor.clipboard, "llo");
}

#[test]
fn kill_line_at_eol() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "helloworld");
    assert_eq!(editor.clipboard, "\n");
}

#[test]
fn buffer_beginning_and_end() {
    let mut editor = Editor::new_with_text("hello\nworld");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::BufferBeginning);
    assert_eq!(editor.point(), 0);
    editor.execute(Command::BufferEnd);
    assert_eq!(editor.point(), 11);
}

#[test]
fn page_down_and_up() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.focused_pane_mut().viewport_height = 3;
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
    editor.pane_tree.focused_pane_mut().viewport_height = 5;
    editor.pane_tree.focused_pane_mut().viewport_width = 40;
    // Move cursor to line 5 (the "f" line)
    for _ in 0..5 {
        editor.execute(Command::NextLine);
    }
    editor.execute(Command::RecenterTopBottom);
    // Center: scroll_top = 5 - 5/2 = 3
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
}

#[test]
fn recenter_cycles_center_top_bottom() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.focused_pane_mut().viewport_height = 5;
    editor.pane_tree.focused_pane_mut().viewport_width = 40;
    // Move cursor to line 5
    for _ in 0..5 {
        editor.execute(Command::NextLine);
    }
    // First C-l: center (scroll_top = 3)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
    // Second C-l: top (scroll_top = 5)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 5);
    // Third C-l: bottom (scroll_top = 5 - 4 = 1)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 1);
    // Fourth C-l: center again (scroll_top = 3)
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
}

#[test]
fn recenter_resets_on_other_command() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne\nf\ng\nh\ni\nj");
    editor.pane_tree.focused_pane_mut().viewport_height = 5;
    editor.pane_tree.focused_pane_mut().viewport_width = 40;
    for _ in 0..5 {
        editor.execute(Command::NextLine);
    }
    // First C-l: center
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
    // Any other command resets the cycle
    editor.execute(Command::ForwardChar);
    // Next C-l should be center again, not top
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 3);
}

#[test]
fn recenter_at_beginning_of_buffer() {
    let mut editor = Editor::new_with_text("a\nb\nc\nd\ne");
    editor.pane_tree.focused_pane_mut().viewport_height = 5;
    editor.pane_tree.focused_pane_mut().viewport_width = 40;
    // Cursor at line 0
    editor.execute(Command::RecenterTopBottom);
    // Center: 0.saturating_sub(2) = 0
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 0);
    // Top: scroll_top = 0
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 0);
    // Bottom: 0.saturating_sub(4) = 0
    editor.execute(Command::RecenterTopBottom);
    assert_eq!(editor.pane_tree.focused_pane().scroll_top, 0);
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
    let first_id = editor.pane_tree.focused_pane().buffer_id;

    editor.open_file(&file).unwrap();
    assert_eq!(editor.pane_tree.focused_pane().buffer_id, first_id);
    assert_eq!(editor.buffers.len(), 2);
}

#[test]
fn kill_last_buffer_creates_scratch() {
    let mut editor = Editor::new();
    assert_eq!(editor.buffers.len(), 1);
    editor.do_kill_buffer(0);
    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(editor.current_buffer().name, "*scratch*");
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

impl Editor {
    /// Helper: set minibuffer text directly (for tests).
    fn set_minibuffer_text(&mut self, text: &str) {
        self.minibuffer_buffer.text = ropey::Rope::from_str(text);
        self.minibuffer_pane.point = text.chars().count();
    }
}

#[test]
fn goto_line_via_prompt() {
    let mut editor = Editor::new_with_text("line1\nline2\nline3\nline4");
    editor.execute(Command::GotoLine);
    editor.set_minibuffer_text("3");
    editor.submit_prompt();
    let (line, _) = editor.current_buffer().char_to_line_col(editor.point());
    assert_eq!(line, 2);
}

#[test]
fn find_file_submit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut editor = Editor::new();
    editor.execute(Command::FindFile);
    editor.set_minibuffer_text(&file.to_string_lossy());
    editor.submit_prompt();

    assert_eq!(editor.buffer_text(), "hello");
    assert!(!editor.minibuffer.is_active());
}

#[test]
fn find_file_submit_normalizes_dot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut editor = Editor::new();
    editor.execute(Command::FindFile);
    // Use /./  in path — should still open the file
    let input = format!("{}/./test.txt", dir.path().display());
    editor.set_minibuffer_text(&input);
    editor.submit_prompt();

    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn find_file_submit_normalizes_dotdot() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut editor = Editor::new();
    editor.execute(Command::FindFile);
    // Use /../ in path — should still open the file
    let input = format!("{}/sub/../test.txt", dir.path().display());
    editor.set_minibuffer_text(&input);
    editor.submit_prompt();

    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn switch_buffer_submit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "file content").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("*scratch*");
    editor.submit_prompt();

    assert_eq!(editor.current_buffer().name, "*scratch*");
}

#[test]
fn switch_buffer_empty_input_uses_last_buffer_in_window() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "file content").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    assert_eq!(editor.current_buffer().name, "test.txt");

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("");
    editor.submit_prompt();
    assert_eq!(editor.current_buffer().name, "*scratch*");

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("");
    editor.submit_prompt();
    assert_eq!(editor.current_buffer().name, "test.txt");
}

#[test]
fn switch_buffer_restores_point_in_window() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "file buffer").unwrap();

    let mut editor = Editor::new_with_text("scratch buffer");
    editor.pane_tree.focused_pane_mut().point = 6;

    editor.open_file(&file).unwrap();
    editor.pane_tree.focused_pane_mut().point = 4;

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("*scratch*");
    editor.submit_prompt();
    assert_eq!(editor.current_buffer().name, "*scratch*");
    assert_eq!(editor.point(), 6);

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("test.txt");
    editor.submit_prompt();
    assert_eq!(editor.current_buffer().name, "test.txt");
    assert_eq!(editor.point(), 4);
}

#[test]
fn switch_buffer_restores_point_per_window() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "file buffer").unwrap();

    let mut editor = Editor::new_with_text("scratch buffer");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.open_file(&file).unwrap();
    editor.pane_tree.focused_pane_mut().point = 5;

    editor.execute(Command::SplitVertical);
    editor.execute(Command::CycleFocus);
    editor.pane_tree.focused_pane_mut().point = 1;

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("*scratch*");
    editor.submit_prompt();
    editor.pane_tree.focused_pane_mut().point = 7;

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("test.txt");
    editor.submit_prompt();
    assert_eq!(editor.point(), 1);

    editor.execute(Command::CycleFocus);
    assert_eq!(editor.current_buffer().name, "test.txt");
    assert_eq!(editor.point(), 5);

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("*scratch*");
    editor.submit_prompt();
    assert_eq!(editor.point(), 2);

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("test.txt");
    editor.submit_prompt();
    assert_eq!(editor.point(), 5);
}

// === Region/Clipboard tests ===

#[test]
fn set_mark() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::SetMark);
    assert_eq!(editor.pane_tree.focused_pane().mark, Some(2));
}

#[test]
fn swap_point_and_mark() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 1;
    editor.execute(Command::SetMark);
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.execute(Command::SwapPointAndMark);
    assert_eq!(editor.point(), 1);
    assert_eq!(editor.pane_tree.focused_pane().mark, Some(4));
}

#[test]
fn region_returns_ordered_range() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.pane_tree.focused_pane_mut().mark = Some(1);
    let (start, end) = editor.region().unwrap();
    assert_eq!(start, 1);
    assert_eq!(end, 4);
}

#[test]
fn cut_removes_region() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::SetMark);
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.clipboard, " world");
    assert_eq!(editor.point(), 5);
    assert_eq!(editor.pane_tree.focused_pane().mark, None);
}

#[test]
fn copy_preserves_text() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 0;
    editor.execute(Command::SetMark);
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::Copy);
    assert_eq!(editor.buffer_text(), "hello world");
    assert_eq!(editor.clipboard, "hello");
    assert_eq!(editor.pane_tree.focused_pane().mark, None);
}

#[test]
fn paste_inserts_clipboard() {
    let mut editor = Editor::new_with_text("hello");
    editor.clipboard = " world".to_string();
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::Paste);
    assert_eq!(editor.buffer_text(), "hello world");
    assert_eq!(editor.point(), 11);
}

#[test]
fn cancel_deactivates_mark() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::SetMark);
    assert!(editor.pane_tree.focused_pane().mark.is_some());
    editor.execute(Command::Cancel);
    assert_eq!(editor.pane_tree.focused_pane().mark, None);
}

#[test]
fn cut_then_paste() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 6;
    editor.execute(Command::SetMark);
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "hello ");
    editor.pane_tree.focused_pane_mut().point = 0;
    editor.execute(Command::Paste);
    assert_eq!(editor.buffer_text(), "worldhello ");
}

#[test]
fn cut_undo() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::SetMark);
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "hello");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "hello world");
}

// === Pane split tests ===

#[test]
fn split_vertical_creates_two_panes() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::SplitVertical);
    assert_eq!(editor.pane_tree.pane_count(), 2);
}

#[test]
fn split_horizontal_creates_two_panes() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::SplitHorizontal);
    assert_eq!(editor.pane_tree.pane_count(), 2);
}

#[test]
fn cycle_focus_between_panes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "other").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::SplitVertical);
    // Both panes show same buffer
    let first_bid = editor.pane_tree.focused_pane().buffer_id;
    editor.execute(Command::CycleFocus);
    let second_bid = editor.pane_tree.focused_pane().buffer_id;
    assert_eq!(first_bid, second_bid);
}

#[test]
fn delete_pane_works() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::SplitVertical);
    assert_eq!(editor.pane_tree.pane_count(), 2);
    editor.execute(Command::DeletePane);
    assert_eq!(editor.pane_tree.pane_count(), 1);
}

#[test]
fn delete_only_pane_shows_message() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::DeletePane);
    assert_eq!(editor.pane_tree.pane_count(), 1);
}

#[test]
fn delete_other_panes() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::SplitVertical);
    editor.execute(Command::SplitHorizontal);
    assert_eq!(editor.pane_tree.pane_count(), 3);
    editor.execute(Command::DeleteOtherPanes);
    assert_eq!(editor.pane_tree.pane_count(), 1);
}

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

#[test]
fn open_nonexistent_file_creates_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("new_file.txt");
    assert!(!file.exists());

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();

    assert_eq!(editor.current_buffer().name, "new_file.txt");
    assert_eq!(editor.buffer_text(), "");
    assert_eq!(
        editor.current_buffer().path.as_ref().unwrap().file_name(),
        Some(std::ffi::OsStr::new("new_file.txt"))
    );
}

#[test]
fn open_nonexistent_file_save_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("new_file.txt");
    assert!(!file.exists());

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('h'));
    editor.execute(Command::InsertChar('i'));
    editor.execute(Command::Save);

    assert!(file.exists());
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "hi");
}

#[test]
fn delete_word_backward_undo() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 11;
    editor.execute(Command::DeleteWordBackward);
    assert_eq!(editor.buffer_text(), "hello ");
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "hello world");
}

#[test]
fn undo_with_nothing_to_undo() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::Undo);
    assert_eq!(
        editor.minibuffer.message,
        Some("No further undo information".to_string())
    );
}

#[test]
fn redo_with_nothing_to_redo() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::Redo);
    assert_eq!(
        editor.minibuffer.message,
        Some("No further redo information".to_string())
    );
}

#[test]
fn swap_point_and_mark_no_mark() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::SwapPointAndMark);
    assert_eq!(editor.minibuffer.message, Some("No mark set".to_string()));
}

#[test]
fn cut_no_region() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::Cut);
    assert_eq!(
        editor.minibuffer.message,
        Some("No region selected".to_string())
    );
}

#[test]
fn copy_no_region() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::Copy);
    assert_eq!(
        editor.minibuffer.message,
        Some("No region selected".to_string())
    );
}

#[test]
fn cancel_active_minibuffer() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::FindFile);
    assert!(editor.minibuffer.is_active());
    editor.execute(Command::Cancel);
    assert!(!editor.minibuffer.is_active());
}

#[test]
fn write_file_prompt() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::WriteFile);
    assert!(editor.minibuffer.is_active());
    let prompt = editor.minibuffer.prompt().unwrap();
    assert_eq!(prompt.kind, PromptKind::WriteFile);
}

#[test]
fn kill_buffer_unmodified() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::KillBuffer);
    // Killing the only buffer creates a new scratch buffer
    assert_eq!(editor.current_buffer().name, "*scratch*");
    assert_eq!(editor.buffer_text(), "");
}

#[test]
fn kill_buffer_modified_prompts() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('x'));
    assert!(editor.current_buffer().modified);
    editor.execute(Command::KillBuffer);
    assert!(editor.minibuffer.is_active());
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::KillConfirm { .. }));
}

#[test]
fn kill_confirm_yes_kills_buffer_without_quitting() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('x'));
    let old_id = editor.pane_tree.focused_pane().buffer_id;
    editor.execute(Command::KillBuffer);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    assert!(editor.buffers.iter().all(|b| b.id != old_id));
}

#[test]
fn kill_confirm_no_keeps_buffer() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('x'));
    let old_id = editor.pane_tree.focused_pane().buffer_id;
    editor.execute(Command::KillBuffer);
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    assert!(editor.buffers.iter().any(|b| b.id == old_id));
    assert_eq!(editor.buffer_text(), "x");
}

#[test]
fn kill_buffer_with_others_remaining() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.open_file(&file2).unwrap();
    assert_eq!(editor.buffers.len(), 3); // scratch + a.txt + b.txt
    let current_id = editor.pane_tree.focused_pane().buffer_id;
    editor.execute(Command::KillBuffer);
    // Buffer was killed, switched to first remaining
    assert!(editor.buffers.iter().all(|b| b.id != current_id));
}

#[test]
fn goto_line_invalid_input() {
    let mut editor = Editor::new_with_text("line1\nline2\nline3");
    editor.execute(Command::GotoLine);
    editor.set_minibuffer_text("abc");
    editor.submit_prompt();
    assert_eq!(
        editor.minibuffer.message,
        Some("Invalid line number".to_string())
    );
}

#[test]
fn switch_to_nonexistent_buffer() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("nonexistent");
    editor.submit_prompt();
    assert_eq!(
        editor.minibuffer.message,
        Some("No buffer named 'nonexistent'".to_string())
    );
}

#[test]
fn submit_write_file_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("output.txt");

    let mut editor = Editor::new_with_text("content");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&file.to_string_lossy());
    editor.submit_prompt();

    assert!(file.exists());
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "content");
}

#[test]
fn save_confirm_yes() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    // Simulate quit which triggers save confirm
    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(editor.should_quit);
    let content = std::fs::read_to_string(&file).unwrap();
    assert_eq!(content, "Xoriginal");
}

#[test]
fn save_confirm_no() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert!(editor.should_quit);
}

#[test]
fn save_confirm_quit() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("q");
    editor.submit_prompt();
    assert!(!editor.should_quit);
}

#[test]
fn save_confirm_invalid_reprompts_same_buffer() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("x");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    // The prompt re-asks for the same buffer instead of aborting the quit.
    assert!(editor.minibuffer.is_active());
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::QuitSaveConfirm { .. }));
}

#[test]
fn quit_prompts_for_each_modified_buffer() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.execute(Command::InsertChar('1'));
    editor.open_file(&file2).unwrap();
    editor.execute(Command::InsertChar('2'));

    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    // First buffer saved; second modified buffer must now be prompted.
    assert!(!editor.should_quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(editor.should_quit);
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "1aaa");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "2bbb");
}

#[test]
fn quit_save_confirm_no_skips_buffer_and_continues() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.execute(Command::InsertChar('1'));
    editor.open_file(&file2).unwrap();
    editor.execute(Command::InsertChar('2'));

    editor.execute(Command::Quit);
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(editor.should_quit);
    // First buffer was declined and not saved; second was saved.
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "2bbb");
}

#[test]
fn quit_save_confirm_q_aborts_whole_flow() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.execute(Command::InsertChar('1'));
    editor.open_file(&file2).unwrap();
    editor.execute(Command::InsertChar('2'));

    editor.execute(Command::Quit);
    editor.set_minibuffer_text("q");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    assert!(!editor.minibuffer.is_active());
    // Nothing was saved and a fresh quit starts the flow over.
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "bbb");
    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn quit_save_confirm_a_aborts_editor_without_saving() {
    let dir = tempfile::tempdir().unwrap();
    let file1 = dir.path().join("a.txt");
    let file2 = dir.path().join("b.txt");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.execute(Command::InsertChar('1'));
    editor.open_file(&file2).unwrap();
    editor.execute(Command::InsertChar('2'));

    editor.execute(Command::Quit);
    editor.set_minibuffer_text("a");
    editor.submit_prompt();
    // Quits immediately, discarding everything, and signals abort so main
    // exits non-zero (for use as a git editor).
    assert!(editor.should_quit);
    assert!(editor.quit_abort);
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "bbb");
}

#[test]
fn normal_quit_is_not_an_abort() {
    let mut editor = Editor::new();
    editor.execute(Command::Quit);
    assert!(editor.should_quit);
    assert!(!editor.quit_abort);
}

#[test]
fn quit_save_confirm_saves_correct_buffer_with_duplicate_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("a")).unwrap();
    std::fs::create_dir(dir.path().join("b")).unwrap();
    let file1 = dir.path().join("a").join("mod.rs");
    let file2 = dir.path().join("b").join("mod.rs");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.open_file(&file2).unwrap();
    // Only the second buffer (b/mod.rs) is modified.
    editor.execute(Command::InsertChar('2'));

    editor.execute(Command::Quit);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(editor.should_quit);
    // The modified buffer must be saved to its own file, not the
    // first buffer that happens to share the name "mod.rs".
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "2bbb");
}

#[test]
fn quit_save_yes_without_path_aborts_with_message() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Quit);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    let message = editor.minibuffer.message.clone().unwrap();
    assert!(message.contains("no file"), "got message: {message}");
}

// === External modification detection ===

#[test]
fn save_over_externally_modified_file_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));

    // Another program rewrites the file.
    std::fs::write(&file, "external change").unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
    drop(f);

    editor.execute(Command::Save);
    assert!(editor.minibuffer.is_active(), "save must prompt first");
    // Declining keeps the on-disk content.
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "external change");
    assert!(editor.current_buffer().modified);

    // Confirming overwrites.
    editor.execute(Command::Save);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xoriginal");
    assert!(!editor.current_buffer().modified);
}

#[test]
fn save_without_external_change_does_not_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Save);
    assert!(!editor.minibuffer.is_active());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xoriginal");
}

// === Write-file (C-x C-w) flow ===

#[test]
fn write_file_to_existing_path_prompts_before_overwriting() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "precious").unwrap();

    let mut editor = Editor::new_with_text("new content");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();
    assert!(editor.minibuffer.is_active(), "must confirm overwrite");

    // Declining leaves the file and the buffer identity untouched.
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious");
    assert_eq!(editor.current_buffer().name, "*scratch*");
    assert!(editor.current_buffer().path.is_none());

    // Confirming overwrites and renames the buffer.
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content");
    assert_eq!(editor.current_buffer().name, "target.txt");
}

#[test]
fn write_file_failure_keeps_buffer_identity() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("no_such_dir").join("file.txt");

    let mut editor = Editor::new_with_text("content");
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();

    // The save failed; the buffer must not have been renamed/re-pathed.
    assert_eq!(editor.current_buffer().name, "*scratch*");
    assert!(editor.current_buffer().path.is_none());
    assert!(editor.current_buffer().modified);
}

#[test]
fn write_file_redetects_syntax_for_new_extension() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("code.rs");

    let mut editor = Editor::new_with_text("fn main() {}");
    assert!(editor.current_buffer().syntax.is_none());
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();
    assert!(
        editor.current_buffer().syntax.is_some(),
        "writing to .rs must enable rust highlighting"
    );
}

#[test]
fn write_file_to_own_path_does_not_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    let own_path = editor.current_buffer().path.clone().unwrap();
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&own_path.to_string_lossy());
    editor.submit_prompt();
    assert!(!editor.minibuffer.is_active());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xoriginal");
}

// === Buffer name uniquification ===

#[test]
fn duplicate_basenames_get_uniquified_names() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("a")).unwrap();
    std::fs::create_dir(dir.path().join("b")).unwrap();
    let file1 = dir.path().join("a").join("mod.rs");
    let file2 = dir.path().join("b").join("mod.rs");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.open_file(&file2).unwrap();

    let names: Vec<&str> = editor.buffers.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names.iter().filter(|n| **n == "mod.rs").count(), 1);
    assert!(
        names.contains(&"mod.rs<b>"),
        "second buffer should be disambiguated by parent dir: {names:?}"
    );
}

#[test]
fn uniquified_buffer_reachable_via_switch_buffer() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("a")).unwrap();
    std::fs::create_dir(dir.path().join("b")).unwrap();
    let file1 = dir.path().join("a").join("mod.rs");
    let file2 = dir.path().join("b").join("mod.rs");
    std::fs::write(&file1, "aaa").unwrap();
    std::fs::write(&file2, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file1).unwrap();
    editor.open_file(&file2).unwrap();

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("mod.rs");
    editor.submit_prompt();
    assert_eq!(editor.buffer_text(), "aaa");

    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("mod.rs<b>");
    editor.submit_prompt();
    assert_eq!(editor.buffer_text(), "bbb");
}

#[test]
fn three_way_name_collision_yields_distinct_names() {
    let dir = tempfile::tempdir().unwrap();
    for sub in ["x", "y", "z"] {
        std::fs::create_dir(dir.path().join(sub)).unwrap();
        std::fs::write(dir.path().join(sub).join("mod.rs"), sub).unwrap();
    }

    let mut editor = Editor::new();
    for sub in ["x", "y", "z"] {
        editor.open_file(&dir.path().join(sub).join("mod.rs")).unwrap();
    }

    let mut names: Vec<&str> = editor
        .buffers
        .iter()
        .map(|b| b.name.as_str())
        .filter(|n| n.starts_with("mod.rs"))
        .collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), 3, "all three buffers need distinct names");
}

#[test]
fn mark_stays_valid_through_undo() {
    let mut editor = Editor::new_with_text("hello");
    editor.execute(Command::BufferEnd);
    editor.execute(Command::SetMark); // mark at 5
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::InsertChar('y')); // "helloxy", mark still 5
    editor.execute(Command::Undo); // back to "hello"
    let pane = editor.pane_tree.focused_pane();
    let len = editor.current_buffer().char_count();
    assert!(pane.mark.unwrap() <= len, "mark out of bounds after undo");
    // Region operations on the surviving mark must not panic.
    editor.execute(Command::Copy);
}

// === Grapheme clusters ===

#[test]
fn forward_char_moves_over_combining_cluster() {
    // "ae\u{301}b": a(0) e(1) combining-acute(2) b(3)
    let mut editor = Editor::new_with_text("ae\u{301}b");
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 1);
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 3, "must skip the whole e+combining cluster");
}

#[test]
fn backward_char_moves_over_combining_cluster() {
    let mut editor = Editor::new_with_text("ae\u{301}b");
    editor.pane_tree.focused_pane_mut().point = 3;
    editor.execute(Command::BackwardChar);
    assert_eq!(editor.point(), 1);
}

#[test]
fn delete_backward_removes_whole_combining_cluster() {
    let mut editor = Editor::new_with_text("ae\u{301}b");
    editor.pane_tree.focused_pane_mut().point = 3;
    editor.execute(Command::DeleteBackward);
    assert_eq!(editor.buffer_text(), "ab");
    assert_eq!(editor.point(), 1);
}

#[test]
fn delete_forward_removes_whole_combining_cluster() {
    let mut editor = Editor::new_with_text("ae\u{301}b");
    editor.pane_tree.focused_pane_mut().point = 1;
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "ab");
    assert_eq!(editor.point(), 1);
}

#[test]
fn forward_char_moves_over_emoji_zwj_sequence() {
    // Family emoji: man + ZWJ + woman + ZWJ + girl = 5 chars, 1 cluster.
    let text = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}x";
    let mut editor = Editor::new_with_text(text);
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 5, "must skip the whole ZWJ sequence");
    editor.execute(Command::BackwardChar);
    assert_eq!(editor.point(), 0);
}

// === CRLF atomicity ===

fn crlf_editor() -> Editor {
    // "ab\r\ncd": chars are a(0) b(1) \r(2) \n(3) c(4) d(5)
    Editor::new_with_text("ab\r\ncd")
}

#[test]
fn forward_char_skips_over_crlf_pair() {
    let mut editor = crlf_editor();
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::ForwardChar);
    assert_eq!(editor.point(), 4, "point must not land between \\r and \\n");
}

#[test]
fn backward_char_skips_over_crlf_pair() {
    let mut editor = crlf_editor();
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.execute(Command::BackwardChar);
    assert_eq!(editor.point(), 2);
}

#[test]
fn delete_backward_removes_whole_crlf() {
    let mut editor = crlf_editor();
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.execute(Command::DeleteBackward);
    assert_eq!(editor.buffer_text(), "abcd");
    assert_eq!(editor.point(), 2);
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "ab\r\ncd");
}

#[test]
fn delete_forward_removes_whole_crlf() {
    let mut editor = crlf_editor();
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "abcd");
    assert_eq!(editor.point(), 2);
}

#[test]
fn kill_line_at_eol_removes_whole_crlf() {
    let mut editor = crlf_editor();
    editor.pane_tree.focused_pane_mut().point = 2; // EOL of "ab"
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "abcd");
    assert_eq!(editor.clipboard, "\r\n");
}

// === Non-ASCII (char-vs-byte) correctness ===

#[test]
fn undo_non_ascii_insert_at_end() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('é'));
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "");
}

#[test]
fn undo_non_ascii_insert_mid_buffer() {
    let mut editor = Editor::new_with_text("abcd");
    editor.execute(Command::ForwardChar);
    editor.execute(Command::ForwardChar);
    editor.execute(Command::InsertChar('é'));
    assert_eq!(editor.buffer_text(), "abécd");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "abcd");
}

#[test]
fn redo_non_ascii_roundtrip() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('é'));
    editor.execute(Command::InsertChar('ü'));
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "");
    editor.execute(Command::Redo);
    assert_eq!(editor.buffer_text(), "éü");
    assert_eq!(editor.point(), 2);
}

#[test]
fn undo_non_ascii_kill_line() {
    let mut editor = Editor::new_with_text("héllo wörld");
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "héllo wörld");
}

#[test]
fn paste_non_ascii_sets_point_in_chars() {
    let mut editor = Editor::new_with_text("");
    editor.clipboard = "héllo".to_string();
    editor.execute(Command::Paste);
    assert_eq!(editor.point(), 5);
    editor.execute(Command::InsertChar('!'));
    assert_eq!(editor.buffer_text(), "héllo!");
}

#[test]
fn isearch_finds_non_ascii_query_at_char_position() {
    let mut editor = Editor::new_with_text("héllo wörld");
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "wörld".to_string();
    }
    editor.isearch_update();
    // "wörld" ends at char index 11 (point goes to match end), and the
    // match starts at char index 6 — not at the byte offsets 13/8.
    let state = editor.isearch.as_ref().unwrap();
    assert_eq!(state.current_match, Some(6));
    assert_eq!(editor.point(), 6);
}

#[test]
fn isearch_backward_non_ascii() {
    let mut editor = Editor::new_with_text("ééé aaa ééé");
    editor.execute(Command::BufferEnd);
    editor.execute(Command::ISearchBackward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "ééé".to_string();
    }
    editor.isearch_update();
    let state = editor.isearch.as_ref().unwrap();
    assert_eq!(state.current_match, Some(8));
}

#[test]
fn consecutive_non_ascii_inserts_undo_as_one_group() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('é'));
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::InsertChar('ü'));
    editor.execute(Command::Undo);
    // All three chars form one undo group, like ASCII inserts do.
    assert_eq!(editor.buffer_text(), "");
}

// === Multi-pane point/mark adjustment ===

/// Split, move the *other* pane's point to the buffer end, and refocus
/// the original pane. Returns with the original pane focused.
fn split_with_other_pane_at_end(editor: &mut Editor) {
    editor.execute(Command::SplitVertical);
    editor.execute(Command::CycleFocus);
    editor.execute(Command::BufferEnd);
    editor.execute(Command::CycleFocus);
}

#[test]
fn cut_in_one_pane_adjusts_point_in_other_pane() {
    let mut editor = Editor::new_with_text("hello world");
    split_with_other_pane_at_end(&mut editor); // other pane: point=11
    editor.execute(Command::BufferBeginning);
    editor.execute(Command::SetMark);
    editor.execute(Command::BufferEnd);
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "");
    editor.execute(Command::CycleFocus);
    assert_eq!(editor.point(), 0);
    editor.execute(Command::InsertChar('x')); // must not panic
    assert_eq!(editor.buffer_text(), "x");
}

#[test]
fn insert_in_one_pane_shifts_point_in_other_pane() {
    let mut editor = Editor::new_with_text("abc");
    split_with_other_pane_at_end(&mut editor); // other pane: point=3
    editor.execute(Command::BufferBeginning);
    editor.execute(Command::InsertChar('x')); // "xabc"
    editor.execute(Command::CycleFocus);
    assert_eq!(editor.point(), 4);
}

#[test]
fn delete_in_one_pane_adjusts_mark_in_other_pane() {
    let mut editor = Editor::new_with_text("hello world");
    // The other pane keeps a mark at the buffer end.
    editor.execute(Command::SplitVertical);
    editor.execute(Command::CycleFocus);
    editor.execute(Command::BufferEnd);
    editor.execute(Command::SetMark);
    editor.execute(Command::BufferBeginning);
    editor.execute(Command::CycleFocus);
    // In the focused pane, delete "world" (chars 6..11).
    editor.execute(Command::BufferBeginning);
    for _ in 0..6 {
        editor.execute(Command::ForwardChar);
    }
    editor.execute(Command::SetMark);
    editor.execute(Command::BufferEnd);
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "hello ");
    // The other pane's mark (was 11) must have been clamped into bounds;
    // cutting its region must not panic and cuts the remaining text.
    editor.execute(Command::CycleFocus);
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "");
}

#[test]
fn undo_in_one_pane_adjusts_point_in_other_pane() {
    let mut editor = Editor::new_with_text("");
    editor.execute(Command::InsertChar('a'));
    editor.execute(Command::InsertChar('b'));
    split_with_other_pane_at_end(&mut editor); // other pane: point=2
    editor.execute(Command::Undo); // buffer back to ""
    assert_eq!(editor.buffer_text(), "");
    editor.execute(Command::CycleFocus);
    assert_eq!(editor.point(), 0);
    editor.execute(Command::InsertChar('x')); // must not panic
}

#[test]
fn edit_adjusts_saved_view_state_of_other_pane() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("other.txt");
    std::fs::write(&file, "xyz").unwrap();

    let mut editor = Editor::new_with_text("hello world");
    // The other pane views the scratch buffer with point at the end,
    // then switches away (saving that view state).
    editor.execute(Command::SplitVertical);
    editor.execute(Command::CycleFocus);
    editor.execute(Command::BufferEnd);
    editor.open_file(&file).unwrap();
    editor.execute(Command::CycleFocus);
    // Delete everything in the scratch buffer from the first pane.
    editor.execute(Command::SetMark);
    editor.execute(Command::BufferEnd);
    editor.execute(Command::Cut);
    assert_eq!(editor.buffer_text(), "");
    // Switching the other pane back must restore an in-bounds point.
    editor.execute(Command::CycleFocus);
    editor.execute(Command::SwitchBuffer);
    editor.set_minibuffer_text("*scratch*");
    editor.submit_prompt();
    assert_eq!(editor.point(), 0);
    editor.execute(Command::InsertChar('x')); // must not panic
}

#[test]
fn isearch_no_match() {
    let mut editor = Editor::new_with_text("hello world");
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "xyz".to_string();
    }
    editor.isearch_update();
    assert_eq!(
        editor.minibuffer.message,
        Some("Failing I-search".to_string())
    );
}

#[test]
fn isearch_next_no_more_matches() {
    let mut editor = Editor::new_with_text("hello world");
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "hello".to_string();
    }
    editor.isearch_update();
    // Try to cycle — no more matches
    editor.isearch_next();
    assert!(editor
        .minibuffer
        .message
        .as_ref()
        .unwrap()
        .contains("Failing"));
}

#[test]
fn isearch_backward_finds_match() {
    let mut editor = Editor::new_with_text("hello world hello");
    // Move to end
    editor.pane_tree.focused_pane_mut().point = 17;
    editor.execute(Command::ISearchBackward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "hello".to_string();
    }
    editor.isearch_update();
    // rfind from position 17 finds the last "hello" before cursor = position 12
    assert_eq!(editor.point(), 12);
}

#[test]
fn isearch_next_backward() {
    let mut editor = Editor::new_with_text("ab ab ab");
    editor.pane_tree.focused_pane_mut().point = 8;
    editor.execute(Command::ISearchBackward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "ab".to_string();
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
                .message
                .as_ref()
                .unwrap()
                .contains("Failing")
    );
}

#[test]
fn isearch_empty_query_restores() {
    let mut editor = Editor::new_with_text("hello world");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::ISearchForward);
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = "world".to_string();
    }
    editor.isearch_update();
    assert_eq!(editor.point(), 6);
    // Clear query → should restore
    if let Some(ref mut isearch) = editor.isearch {
        isearch.query = String::new();
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
    assert!(!editor.current_buffer().modified);
    editor.execute(Command::InsertChar('X'));
    assert!(editor.current_buffer().modified);
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert!(
        !editor.current_buffer().modified,
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
    assert!(!editor.current_buffer().modified);

    // Make an edit and save
    editor.execute(Command::InsertChar('X'));
    editor.execute(Command::Save);
    assert!(!editor.current_buffer().modified);

    // Make another edit
    editor.execute(Command::InsertChar('Y'));
    assert!(editor.current_buffer().modified);

    // Undo back to saved state
    editor.commit_undo_group();
    editor.execute(Command::Undo);
    assert!(
        !editor.current_buffer().modified,
        "Should be unmodified after undoing to last save point"
    );
}

// === Indentation tests ===

#[test]
fn insert_newline_copies_indentation() {
    let mut editor = Editor::new_with_text("    hello");
    // Place cursor at end of "hello"
    editor.pane_tree.focused_pane_mut().point = 9;
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "    hello\n    ");
    assert_eq!(editor.point(), 14); // after the 4 spaces on new line
}

#[test]
fn insert_newline_no_indent_at_column_zero() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 5;
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "hello\n");
    assert_eq!(editor.point(), 6);
}

#[test]
fn insert_newline_mid_line_preserves_indent() {
    let mut editor = Editor::new_with_text("    helloworld");
    editor.pane_tree.focused_pane_mut().point = 9; // between "hello" and "world"
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "    hello\n    world");
    assert_eq!(editor.point(), 14);
}

#[test]
fn indent_line_single() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    hello");
    assert_eq!(editor.point(), 6); // 2 + 4
}

#[test]
fn indent_line_with_region() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    // Select all three lines
    let pane = editor.pane_tree.focused_pane_mut();
    pane.point = 0;
    pane.mark = Some(11); // end of "ccc"
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    aaa\n    bbb\n    ccc");
}

#[test]
fn dedent_line_single() {
    let mut editor = Editor::new_with_text("    hello");
    editor.pane_tree.focused_pane_mut().point = 6; // on 'l'
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2); // 6 - 4
}

#[test]
fn dedent_line_partial_spaces() {
    let mut editor = Editor::new_with_text("  hello");
    editor.pane_tree.focused_pane_mut().point = 4;
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2); // 4 - 2
}

#[test]
fn dedent_line_no_leading_spaces() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "hello");
    assert_eq!(editor.point(), 2); // unchanged
}

#[test]
fn dedent_line_with_region() {
    let mut editor = Editor::new_with_text("    aaa\n    bbb\n    ccc");
    let pane = editor.pane_tree.focused_pane_mut();
    pane.point = 0;
    pane.mark = Some(23); // end of text
    editor.execute(Command::DedentLine);
    assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
}

#[test]
fn region_end_at_col0_excludes_last_line() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    // Region covers first two lines, but end is at start of "ccc"
    let pane = editor.pane_tree.focused_pane_mut();
    pane.point = 0;
    pane.mark = Some(8); // start of "ccc" line (col 0)
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    aaa\n    bbb\nccc");
}

#[test]
fn undo_reverses_indent() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().point = 2;
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    hello");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn undo_reverses_region_indent() {
    let mut editor = Editor::new_with_text("aaa\nbbb\nccc");
    let pane = editor.pane_tree.focused_pane_mut();
    pane.point = 0;
    pane.mark = Some(11);
    editor.execute(Command::IndentLine);
    assert_eq!(editor.buffer_text(), "    aaa\n    bbb\n    ccc");
    editor.execute(Command::Undo);
    assert_eq!(editor.buffer_text(), "aaa\nbbb\nccc");
}

#[test]
fn preferred_column_cleared_after_indent() {
    let mut editor = Editor::new_with_text("hello");
    editor.pane_tree.focused_pane_mut().preferred_column = Some(5);
    editor.execute(Command::IndentLine);
    assert_eq!(editor.pane_tree.focused_pane().preferred_column, None);
}

#[test]
fn preferred_column_cleared_after_dedent() {
    let mut editor = Editor::new_with_text("    hello");
    editor.pane_tree.focused_pane_mut().preferred_column = Some(5);
    editor.pane_tree.focused_pane_mut().point = 6;
    editor.execute(Command::DedentLine);
    assert_eq!(editor.pane_tree.focused_pane().preferred_column, None);
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
