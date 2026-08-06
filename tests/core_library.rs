use minimacs::{Command, Editor};

#[test]
fn public_core_edits_without_a_terminal_frontend() {
    let mut editor = Editor::new();

    editor.execute(Command::InsertChar('H'));
    editor.execute(Command::InsertChar('i'));

    assert_eq!(editor.current_buffer().text.to_string(), "Hi");
    assert_eq!(editor.pane_tree.focused_pane().point, 2);
}
