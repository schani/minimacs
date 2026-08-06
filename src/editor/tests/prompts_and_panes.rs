use super::*;
use crate::minibuffer::PromptKind;

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
fn edit_above_other_pane_viewport_shifts_its_scroll() {
    let text: String = (1..=200).map(|i| format!("line{i}\n")).collect();
    let mut editor = Editor::new_with_text(&text);
    editor.execute(Command::SplitVertical);

    // Scroll the other pane (same buffer) down to line 100.
    editor.pane_tree.cycle_focus();
    {
        let point = editor.current_buffer().line_col_to_char(100, 0);
        let pane = editor.pane_tree.focused_pane_mut();
        pane.scroll_top = 100;
        pane.point = point;
    }
    editor.pane_tree.cycle_focus();

    // Delete the first 50 lines from the focused pane.
    let end = editor.current_buffer().line_col_to_char(50, 0);
    editor.apply_edit(0, end, "", EditRecord::Delete);

    // The other pane must still show the same content: scroll_top shifted
    // up by the 50 removed lines, point still at the same line's start.
    editor.pane_tree.cycle_focus();
    let pane = editor.pane_tree.focused_pane();
    assert_eq!(pane.scroll_top, 50);
    let (line, col) = editor.current_buffer().char_to_line_col(pane.point);
    assert_eq!((line, col), (50, 0));
}

#[test]
fn os_clipboard_is_inert_under_test() {
    // arboard is compiled out in tests: the persistent handle must no-op on
    // set and return None on get so paste falls back to the internal
    // clipboard (which the paste tests below rely on).
    let mut clip = super::OsClipboard::new();
    clip.set_text("x");
    assert_eq!(clip.get_text(), None);
}

#[test]
fn paste_converts_crlf_to_lf_buffer_ending() {
    let mut editor = Editor::new();
    editor.clipboard = "a\r\nb".to_string();
    editor.execute(Command::Paste);
    assert_eq!(editor.buffer_text(), "a\nb");
}

#[test]
fn paste_converts_lone_cr_to_buffer_ending() {
    let mut editor = Editor::new();
    editor.clipboard = "a\rb".to_string();
    editor.execute(Command::Paste);
    assert_eq!(editor.buffer_text(), "a\nb");
}

#[test]
fn paste_stays_lf_in_crlf_buffer() {
    // The rope is LF-only regardless of the buffer's save-time line
    // ending; CRLF is produced at save, never stored.
    let mut editor = Editor::new();
    editor.current_buffer_mut().line_ending = crate::buffer::LineEnding::CrLf;
    editor.clipboard = "a\r\nb".to_string();
    editor.execute(Command::Paste);
    assert_eq!(editor.buffer_text(), "a\nb");
    assert_eq!(editor.point(), 3);
}

#[test]
fn insert_newline_inserts_lf_in_crlf_buffer() {
    let mut editor = Editor::new_with_text("ab");
    editor.current_buffer_mut().line_ending = crate::buffer::LineEnding::CrLf;
    editor.pane_tree.focused_pane_mut().point = 1;
    editor.execute(Command::InsertNewline);
    assert_eq!(editor.buffer_text(), "a\nb");
    assert_eq!(editor.point(), 2);
}

#[test]
fn minibuffer_paste_flattens_all_line_break_forms() {
    let mut editor = Editor::new();
    editor.execute(Command::SwitchBuffer);
    editor.clipboard = "a\rb\r\nc\nd".to_string();
    editor.execute(Command::Paste);
    assert_eq!(editor.minibuffer_text(), "a b c d");
}

#[test]
fn kill_confirm_reasks_on_invalid_answer() {
    let mut editor = Editor::new();
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::KillBuffer);
    assert!(editor.minibuffer.is_active());

    editor.set_minibuffer_text("wat");
    editor.submit_prompt();
    // Invalid answer: prompt stays active with cleared input, nothing killed.
    assert!(editor.minibuffer.is_active());
    assert_eq!(editor.minibuffer_text(), "");
    assert_eq!(editor.buffer_text(), "x");

    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(!editor.minibuffer.is_active());
    assert_eq!(editor.buffer_text(), "");
}

#[test]
fn overwrite_confirm_reasks_on_invalid_answer() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("target.txt");
    std::fs::write(&file, "old").unwrap();

    let mut editor = Editor::new_with_text("new");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&file.to_string_lossy());
    editor.submit_prompt();
    assert!(editor.minibuffer.is_active(), "expected overwrite confirm");

    editor.set_minibuffer_text("z");
    editor.submit_prompt();
    assert!(editor.minibuffer.is_active());
    assert_eq!(editor.minibuffer_text(), "");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");

    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert!(!editor.minibuffer.is_active());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
}

#[test]
fn save_anyway_confirm_reasks_on_invalid_answer() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "content").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('x'));

    // Simulate an external change (force a distinct mtime).
    let f = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
    drop(f);

    editor.execute(Command::Save);
    assert!(
        editor.minibuffer.is_active(),
        "expected save-anyway confirm"
    );

    editor.set_minibuffer_text("z");
    editor.submit_prompt();
    assert!(editor.minibuffer.is_active());
    assert_eq!(editor.minibuffer_text(), "");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "content");

    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(!editor.minibuffer.is_active());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "xcontent");
}

#[test]
fn pane_commands_are_ignored_while_prompt_is_active() {
    let mut editor = Editor::new();
    editor.execute(Command::SwitchBuffer);
    assert!(editor.minibuffer.is_active());

    editor.execute(Command::SplitVertical);
    editor.execute(Command::SplitHorizontal);
    assert_eq!(editor.pane_tree.pane_count(), 1);
}

#[test]
fn focus_and_layout_frozen_while_prompt_is_active() {
    let mut editor = Editor::new();
    editor.execute(Command::SplitVertical);
    let focus_before = editor.pane_tree.focus_path().to_vec();

    editor.execute(Command::SwitchBuffer);
    assert!(editor.minibuffer.is_active());

    editor.execute(Command::CycleFocus);
    assert_eq!(editor.pane_tree.focus_path(), focus_before.as_slice());

    editor.execute(Command::DeletePane);
    editor.execute(Command::DeleteOtherPanes);
    assert_eq!(editor.pane_tree.pane_count(), 2);
}

#[test]
fn goto_line_scrolls_target_into_view() {
    let text: String = (1..=200).map(|i| format!("line{i}\n")).collect();
    let mut editor = Editor::new_with_text(&text);
    editor.pane_tree.focused_pane_mut().viewport_height = 10;
    editor.pane_tree.focused_pane_mut().viewport_width = 80;
    editor.execute(Command::GotoLine);
    editor.set_minibuffer_text("150");
    editor.submit_prompt();
    let (line, _) = editor.current_buffer().char_to_line_col(editor.point());
    assert_eq!(line, 149);
    let pane = editor.pane_tree.focused_pane();
    assert!(
        pane.scroll_top <= 149 && 149 < pane.scroll_top + pane.viewport_height,
        "target line must be inside the viewport; scroll_top={}",
        pane.scroll_top
    );
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
fn find_file_at_filesystem_root_has_one_trailing_separator() {
    let root = std::path::PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    let mut editor = Editor::new();
    editor.cwd = root.clone();

    editor.execute(Command::FindFile);

    assert_eq!(editor.minibuffer_text(), root.to_string_lossy());
}

#[test]
fn write_file_at_filesystem_root_has_one_trailing_separator() {
    let root = std::path::PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    let mut editor = Editor::new_with_text("content");
    editor.cwd = root.clone();

    editor.execute(Command::WriteFile);

    assert_eq!(editor.minibuffer_text(), root.to_string_lossy());
}

#[test]
fn find_file_empty_input_reasks() {
    let mut editor = Editor::new();
    let buffer_count = editor.buffers.len();
    editor.execute(Command::FindFile);
    editor.set_minibuffer_text("");
    editor.submit_prompt();

    // No phantom buffer; the prompt re-asks with the requirement flagged in
    // the label and the default directory prefill restored.
    assert_eq!(editor.buffers.len(), buffer_count);
    assert!(editor.minibuffer.is_active());
    let prompt = editor.minibuffer.prompt().unwrap();
    assert_eq!(prompt.kind, PromptKind::FindFile);
    assert_eq!(prompt.label, "Find file (path required): ");
    assert_eq!(
        editor.minibuffer_text(),
        format!("{}/", editor.cwd.display())
    );

    // The re-asked prompt still works: submitting a real path opens it.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "hello").unwrap();
    editor.set_minibuffer_text(&file.to_string_lossy());
    editor.submit_prompt();
    assert!(!editor.minibuffer.is_active());
    assert_eq!(editor.buffer_text(), "hello");
}

#[test]
fn find_file_whitespace_input_reasks() {
    let mut editor = Editor::new();
    let buffer_count = editor.buffers.len();
    editor.execute(Command::FindFile);
    editor.set_minibuffer_text("   ");
    editor.submit_prompt();

    assert_eq!(editor.buffers.len(), buffer_count);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn find_file_input_normalizing_to_empty_reasks() {
    // "." normalizes to the empty string, so it must re-ask like empty input
    // instead of opening a phantom buffer.
    let mut editor = Editor::new();
    let buffer_count = editor.buffers.len();
    editor.execute(Command::FindFile);
    editor.set_minibuffer_text(".");
    editor.submit_prompt();

    assert_eq!(editor.buffers.len(), buffer_count);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn write_file_empty_input_reasks() {
    let mut editor = Editor::new_with_text("content");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text("");
    editor.submit_prompt();

    // No write, no identity change; the prompt re-asks with the requirement
    // flagged in the label and the default directory prefill restored.
    assert!(editor.current_buffer().path.is_none());
    assert!(editor.minibuffer.is_active());
    let prompt = editor.minibuffer.prompt().unwrap();
    assert_eq!(prompt.kind, PromptKind::WriteFile);
    assert_eq!(prompt.label, "Write file (path required): ");
    assert_eq!(
        editor.minibuffer_text(),
        format!("{}/", editor.cwd.display())
    );
}

#[test]
fn write_file_whitespace_input_reasks() {
    let mut editor = Editor::new_with_text("content");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text("  ");
    editor.submit_prompt();

    assert!(editor.current_buffer().path.is_none());
    assert!(editor.minibuffer.is_active());
}

#[test]
fn open_file_empty_path_errors() {
    // Defense in depth: `open_file` itself rejects an empty path (e.g. from
    // a CLI argument) instead of creating a phantom, unsaveable buffer.
    let mut editor = Editor::new();
    let buffer_count = editor.buffers.len();
    assert!(editor.open_file(std::path::Path::new("")).is_err());
    assert_eq!(editor.buffers.len(), buffer_count);
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
