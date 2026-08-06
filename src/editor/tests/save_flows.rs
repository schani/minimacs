use super::*;

// === Save-flow characterization (all flows must behave identically
// === before and after routing through the single write choke point) ===

/// Make every save into `dir` fail by removing write permission.
/// Returns the original permissions so the test can restore them
/// (tempdir cleanup needs the directory writable again).
fn make_dir_readonly(dir: &std::path::Path) -> std::fs::Permissions {
    use std::os::unix::fs::PermissionsExt;
    let orig = std::fs::metadata(dir).unwrap().permissions();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    orig
}

/// Bump a file's mtime so the editor sees it as externally modified.
fn bump_mtime(file: &std::path::Path) {
    let f = std::fs::OpenOptions::new().write(true).open(file).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
}

#[test]
fn save_success_reports_wrote_and_clears_modified() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    assert!(editor.current_buffer().is_modified());

    editor.execute(Command::Save);
    assert_eq!(
        editor.minibuffer.message,
        Some("Wrote test.txt".to_string())
    );
    assert!(!editor.current_buffer().is_modified());
}

#[test]
fn save_error_reports_message_and_keeps_modified() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));

    let orig = make_dir_readonly(dir.path());
    editor.execute(Command::Save);
    std::fs::set_permissions(dir.path(), orig).unwrap();

    let message = editor.minibuffer.message.clone().unwrap();
    assert!(
        message.starts_with("Error saving:"),
        "got message: {message}"
    );
    assert!(editor.current_buffer().is_modified());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
}

#[test]
fn save_anyway_confirm_yes_reports_wrote_and_clears_modified() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    bump_mtime(&file);

    editor.execute(Command::Save);
    assert!(
        editor.minibuffer.is_active(),
        "expected save-anyway confirm"
    );
    editor.set_minibuffer_text("y");
    editor.submit_prompt();

    assert_eq!(
        editor.minibuffer.message,
        Some("Wrote test.txt".to_string())
    );
    assert!(!editor.current_buffer().is_modified());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xoriginal");
}

#[test]
fn save_anyway_confirm_yes_reports_error_when_save_fails() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));
    bump_mtime(&file);

    editor.execute(Command::Save);
    assert!(
        editor.minibuffer.is_active(),
        "expected save-anyway confirm"
    );
    let orig = make_dir_readonly(dir.path());
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    std::fs::set_permissions(dir.path(), orig).unwrap();

    let message = editor.minibuffer.message.clone().unwrap();
    assert!(
        message.starts_with("Error saving:"),
        "got message: {message}"
    );
    assert!(editor.current_buffer().is_modified());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "original");
}

#[test]
fn write_file_success_reports_wrote_and_updates_identity() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("output.txt");

    let mut editor = Editor::new();
    editor.execute(Command::InsertChar('x'));
    assert!(editor.current_buffer().is_modified());

    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();

    assert_eq!(
        editor.minibuffer.message,
        Some("Wrote output.txt".to_string())
    );
    assert!(!editor.current_buffer().is_modified());
    assert_eq!(editor.current_buffer().name(), "output.txt");
    assert_eq!(
        editor.current_buffer().path().as_deref(),
        Some(target.as_path())
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
}

#[test]
fn write_file_failure_reports_error_message() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("no_such_dir").join("file.txt");

    let mut editor = Editor::new();
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();

    let message = editor.minibuffer.message.clone().unwrap();
    assert!(
        message.starts_with("Error saving:"),
        "got message: {message}"
    );
    assert!(editor.current_buffer().is_modified());
}

#[test]
fn overwrite_confirm_yes_reports_wrote_and_clears_modified() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "old").unwrap();

    let mut editor = Editor::new();
    editor.execute(Command::InsertChar('x'));
    editor.execute(Command::WriteFile);
    editor.set_minibuffer_text(&target.to_string_lossy());
    editor.submit_prompt();
    assert!(editor.minibuffer.is_active(), "expected overwrite confirm");
    editor.set_minibuffer_text("y");
    editor.submit_prompt();

    assert_eq!(
        editor.minibuffer.message,
        Some("Wrote target.txt".to_string())
    );
    assert!(!editor.current_buffer().is_modified());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "x");
}

#[test]
fn quit_save_confirm_yes_error_aborts_quit_with_message() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("ro");
    std::fs::create_dir(&sub).unwrap();
    let file1 = sub.join("a.txt");
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
    let orig = make_dir_readonly(&sub);
    editor.set_minibuffer_text("y");
    editor.submit_prompt();
    std::fs::set_permissions(&sub, orig).unwrap();

    // The failed save aborts the whole quit flow: no quit, no further
    // prompts, and the error names the buffer.
    assert!(!editor.should_quit);
    assert!(!editor.minibuffer.is_active());
    let message = editor.minibuffer.message.clone().unwrap();
    assert!(
        message.starts_with("Could not save a.txt:"),
        "got message: {message}"
    );
    assert_eq!(std::fs::read_to_string(&file1).unwrap(), "aaa");
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "bbb");

    // The pending-quit list was cleared: a fresh quit starts over with
    // the first modified buffer.
    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
}

#[test]
fn quit_save_confirm_yes_saves_clears_modified_and_continues_quit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("test.txt");
    std::fs::write(&file, "original").unwrap();

    let mut editor = Editor::new();
    editor.open_file(&file).unwrap();
    editor.execute(Command::InsertChar('X'));

    editor.execute(Command::Quit);
    assert!(editor.minibuffer.is_active());
    editor.set_minibuffer_text("y");
    editor.submit_prompt();

    assert!(editor.should_quit);
    assert!(!editor.current_buffer().is_modified());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "Xoriginal");
}

#[test]
fn open_files_opens_all_and_focuses_the_first() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let c = dir.path().join("c.txt");
    std::fs::write(&a, "aaa").unwrap();
    std::fs::write(&b, "bbb").unwrap();
    std::fs::write(&c, "ccc").unwrap();

    let mut editor = Editor::new();
    editor.open_files(&[a, b, c]);

    // All files became buffers, in argument order (after the scratch buffer),
    // reachable via C-x b; the FIRST file is the one displayed.
    let names: Vec<&str> = editor.buffers.iter().map(|b| b.name()).collect();
    assert_eq!(names, ["*scratch*", "a.txt", "b.txt", "c.txt"]);
    assert_eq!(editor.current_buffer().name(), "a.txt");
    assert_eq!(editor.minibuffer.message.as_deref(), Some("Opened 3 files"));
}

#[test]
fn open_files_single_file_keeps_the_plain_opened_message() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.txt");
    std::fs::write(&a, "aaa").unwrap();

    let mut editor = Editor::new();
    editor.open_files(&[a]);

    assert_eq!(editor.current_buffer().name(), "a.txt");
    assert_eq!(editor.minibuffer.message.as_deref(), Some("Opened a.txt"));
}

#[test]
fn open_files_failed_path_opens_the_rest_and_keeps_the_error_visible() {
    let dir = tempfile::tempdir().unwrap();
    let b = dir.path().join("b.txt");
    std::fs::write(&b, "bbb").unwrap();

    let mut editor = Editor::new();
    editor.open_files(&[PathBuf::new(), b]);

    // The empty path fails (see open_file_empty_path_errors) but the later
    // file still opens and is focused (it is the first successful open);
    // the error message is not papered over by an "Opened N files" summary.
    assert_eq!(editor.current_buffer().name(), "b.txt");
    let message = editor.minibuffer.message.clone().unwrap();
    assert!(message.contains("empty file path"), "got: {message}");
}
