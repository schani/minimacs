use super::*;
use crate::minibuffer::PromptKind;

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
fn nonexistent_file_spellings_share_one_buffer_identity() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("subdir");
    std::fs::create_dir(&subdir).unwrap();
    let with_parent = subdir.join("..").join("future.txt");
    let direct = dir.path().join("future.txt");

    let mut editor = Editor::new();
    editor.open_file(&with_parent).unwrap();
    let first_id = editor.current_buffer().id;
    editor.open_file(&direct).unwrap();

    assert_eq!(editor.current_buffer().id, first_id);
    assert_eq!(editor.buffers.len(), 2, "scratch plus one file buffer");
    let expected = std::fs::canonicalize(dir.path())
        .unwrap()
        .join("future.txt");
    assert_eq!(
        editor.current_buffer().path.as_deref(),
        Some(expected.as_path())
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
fn goto_line_zero_is_invalid() {
    let mut editor = Editor::new_with_text("line1\nline2\nline3");
    editor.pane_tree.focused_pane_mut().point = 6;
    editor.execute(Command::GotoLine);
    editor.set_minibuffer_text("0");
    editor.submit_prompt();

    assert_eq!(editor.point(), 6);
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

/// Simulate another program rewriting `path`: replace the content and bump
/// the mtime well past the one the buffer recorded at load/save time.
fn externally_modify(path: &std::path::Path, content: &str) {
    std::fs::write(path, content).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
}

#[test]
fn quit_save_over_externally_modified_file_prompts_and_resumes_quit() {
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

    externally_modify(&file1, "external");

    editor.execute(Command::Quit);
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::QuitSaveConfirm { .. }));
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    // The quit-time save must hit the external-modification guard instead
    // of clobbering the file.
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::SaveAnywayConfirm { .. }));
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "external");
    assert!(!editor.should_quit);

    // Confirming saves the buffer and resumes the quit with the next
    // pending buffer.
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "1aaa");
    assert!(!editor.should_quit);
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::QuitSaveConfirm { .. }));
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(editor.should_quit);
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "2bbb");
}

#[test]
fn quit_save_anyway_declined_cancels_quit() {
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

    externally_modify(&file1, "external");

    editor.execute(Command::Quit);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::SaveAnywayConfirm { .. }));

    // Declining cancels the whole quit, like a failed save does: no
    // further prompts, nothing written, nothing quit.
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert!(!editor.should_quit);
    assert!(editor.minibuffer.prompt().is_none());
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "external");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "bbb");
    let buf_a = editor.buffers.iter().find(|b| b.name == "a.txt").unwrap();
    assert!(buf_a.modified);

    // A fresh quit starts the flow over from the first modified buffer.
    editor.execute(Command::Quit);
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::QuitSaveConfirm { .. }));
    assert!(!editor.should_quit);
}

#[test]
fn write_file_to_own_externally_modified_path_prompts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    // Use the buffer's own (canonicalized) path so this is the
    // save-to-own-path case, not the overwrite-another-file case.
    let own_path = editor.current_buffer().path.clone().unwrap();

    externally_modify(&file, "external");

    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&own_path.to_string_lossy());
    editor.submit_prompt();
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::SaveAnywayConfirm { .. }));

    // Declining keeps the on-disk content.
    editor.set_minibuffer_text("n");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "external");
    assert!(editor.current_buffer().modified);

    // Confirming overwrites.
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&own_path.to_string_lossy());
    editor.submit_prompt();
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xoriginal");
    assert!(!editor.current_buffer().modified);
}

#[test]
fn write_file_to_other_path_asks_overwrite_only_despite_external_change() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    let other = dir.path().join("b.txt");
    std::fs::write(&file, "aaa").unwrap();
    std::fs::write(&other, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));

    // The buffer's own file changes on disk, but the write goes to a
    // different path — only the overwrite confirmation applies, no
    // double-prompting with the external-modification guard.
    externally_modify(&file, "external");

    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&other.to_string_lossy());
    editor.submit_prompt();
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(matches!(prompt.kind, PromptKind::OverwriteConfirm { .. }));
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(editor.minibuffer.prompt().is_none());
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "Xaaa");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "external");
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

#[cfg(unix)]
#[test]
fn write_file_to_symlink_keeps_link_and_buffer_identity() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "old").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let mut editor = Editor::new_with_text("new content");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&link.to_string_lossy());
    editor.submit_prompt();
    // The link path exists, so the overwrite confirmation applies.
    editor.set_minibuffer_text("y");
    editor.submit_prompt();

    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "C-x C-w through a symlink must write the target, not replace the link"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "new content");
    // The buffer's identity is the logical path the user typed, not the
    // resolved target.
    assert_eq!(
        editor.current_buffer().path.as_deref(),
        Some(link.as_path())
    );
    assert_eq!(editor.current_buffer().name, "link.txt");
}

#[cfg(unix)]
#[test]
fn save_through_symlink_detects_external_target_modification() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "old").unwrap();
    let link = dir.path().join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    // Adopt the symlink as the buffer's path via C-x C-w.
    let mut editor = Editor::new_with_text("mine");
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&link.to_string_lossy());
    editor.submit_prompt();
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert!(!editor.current_buffer().modified);

    editor.execute(Command::InsertChar('X'));
    externally_modify(&target, "external");

    editor.execute(Command::Save);
    let prompt = editor.minibuffer.prompt().unwrap();
    assert!(
        matches!(prompt.kind, PromptKind::SaveAnywayConfirm { .. }),
        "external modification of the symlink target must be detected"
    );
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "Xmine");
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "saving through the link must keep it a link"
    );
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
        editor
            .open_file(&dir.path().join(sub).join("mod.rs"))
            .unwrap();
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
