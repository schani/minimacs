use super::*;

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
    assert!(pane.mark().unwrap() <= len, "mark out of bounds after undo");
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
    editor.pane_tree.set_focused_point(3);
    editor.execute(Command::BackwardChar);
    assert_eq!(editor.point(), 1);
}

#[test]
fn delete_backward_removes_whole_combining_cluster() {
    let mut editor = Editor::new_with_text("ae\u{301}b");
    editor.pane_tree.set_focused_point(3);
    editor.execute(Command::DeleteBackward);
    assert_eq!(editor.buffer_text(), "ab");
    assert_eq!(editor.point(), 1);
}

#[test]
fn delete_forward_removes_whole_combining_cluster() {
    let mut editor = Editor::new_with_text("ae\u{301}b");
    editor.pane_tree.set_focused_point(1);
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

// === Vertical movement snaps to grapheme boundaries ===

// "ab\nxe\u{301}z": a(0) b(1) \n(2) x(3) e(4) combining-acute(5) z(6).
// Column 2 on line 1 falls between e and the combining acute — mid-cluster.

#[test]
fn next_line_snaps_point_to_cluster_start() {
    let mut editor = Editor::new_with_text("ab\nxe\u{301}z");
    editor.pane_tree.set_focused_point(2); // line 0, col 2
    editor.execute(Command::NextLine);
    assert_eq!(
        editor.point(),
        4,
        "point must snap to the cluster start, not rest mid-cluster at 5"
    );
    // Delete-forward at the snapped point removes the whole cluster.
    editor.execute(Command::DeleteForward);
    assert_eq!(editor.buffer_text(), "ab\nxz");
}

#[test]
fn next_line_snap_keeps_backspace_cluster_safe() {
    let mut editor = Editor::new_with_text("ab\nxe\u{301}z");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::NextLine);
    // Backspace at the snapped point deletes the whole preceding grapheme;
    // unsnapped (point 5) it would delete just "e" and orphan the mark.
    editor.execute(Command::DeleteBackward);
    assert_eq!(editor.buffer_text(), "ab\ne\u{301}z");
    assert_eq!(editor.point(), 3);
}

#[test]
fn previous_line_snaps_point_to_cluster_start() {
    // "xe\u{301}z\nab": x(0) e(1) acute(2) z(3) \n(4) a(5) b(6)
    let mut editor = Editor::new_with_text("xe\u{301}z\nab");
    editor.pane_tree.set_focused_point(7); // line 1, col 2
    editor.execute(Command::PreviousLine);
    assert_eq!(editor.point(), 1, "point must snap to the cluster start");
}

#[test]
fn page_down_snaps_point_to_cluster_start() {
    let mut editor = Editor::new_with_text("ab\nxe\u{301}z");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::PageDown);
    assert_eq!(editor.point(), 4);
}

#[test]
fn page_up_snaps_point_to_cluster_start() {
    let mut editor = Editor::new_with_text("xe\u{301}z\nab");
    editor.pane_tree.set_focused_point(7);
    editor.execute(Command::PageUp);
    assert_eq!(editor.point(), 1);
}

#[test]
fn next_line_snaps_out_of_emoji_zwj_sequence() {
    // Line 1: x(0) man(1) ZWJ(2) woman(3) ZWJ(4) girl(5) z(6); the family
    // emoji is one cluster spanning chars 1..6 of the line.
    let text = "ab\nx\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}z";
    let mut editor = Editor::new_with_text(text);
    editor.pane_tree.set_focused_point(2); // line 0, col 2
    editor.execute(Command::NextLine);
    assert_eq!(
        editor.point(),
        4,
        "col 2 of line 1 is inside the ZWJ sequence; snap to its start"
    );
}

#[test]
fn snap_does_not_corrupt_preferred_column() {
    // "ab\nxe\u{301}z\nabcd": moving up from "abcd" col 2 lands mid-cluster
    // on the middle line (snapped to col 1); moving on and back down must
    // restore the ORIGINAL column 2, not the snapped one.
    let mut editor = Editor::new_with_text("ab\nxe\u{301}z\nabcd");
    editor.pane_tree.set_focused_point(10); // line 2, col 2
    editor.execute(Command::PreviousLine);
    assert_eq!(editor.point(), 4, "snapped to cluster start on line 1");
    editor.execute(Command::PreviousLine);
    assert_eq!(editor.point(), 2); // line 0, col 2
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 4);
    editor.execute(Command::NextLine);
    assert_eq!(
        editor.point(),
        10,
        "preferred column must be the original 2, not the snapped 1"
    );
}

// CRLF pairs cannot occur in a rope (the file boundary converts them to
// LF), so no CRLF-atomicity handling exists in movement or deletion.

// === Ex-line-break chars are ordinary content (LF-only decision) ===
//
// With ropey's `unicode_lines` feature off, only `\n` is a line break;
// lone CR, VT, FF, NEL, LS, PS are content like any other char.

#[test]
fn end_of_line_goes_past_form_feed() {
    // FF is not a line break: "a\u{0c}b" is one line of three chars.
    let mut editor = Editor::new_with_text("a\u{0c}b\n");
    editor.execute(Command::EndOfLine);
    assert_eq!(editor.point(), 3, "C-e must go past the FF");
}

#[test]
fn kill_line_kills_through_form_feed() {
    let mut editor = Editor::new_with_text("a\u{0c}b\n");
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "\n");
    assert_eq!(editor.clipboard, "a\u{0c}b");
}

#[test]
fn end_of_line_goes_past_lone_cr() {
    // Old-Mac style content: lone \r chars do not break lines.
    let mut editor = Editor::new_with_text("x\ry\r");
    editor.execute(Command::EndOfLine);
    assert_eq!(editor.point(), 4);
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 4, "there is no second line");
}

#[test]
fn kill_line_kills_through_lone_cr() {
    let mut editor = Editor::new_with_text("x\ry\r");
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "");
    assert_eq!(editor.clipboard, "x\ry\r");
}

#[test]
fn end_of_line_goes_past_unicode_line_separator() {
    let mut editor = Editor::new_with_text("ab\u{2028}cd");
    editor.execute(Command::EndOfLine);
    assert_eq!(editor.point(), 5);
}

#[test]
fn kill_line_kills_through_unicode_line_separator() {
    let mut editor = Editor::new_with_text("ab\u{2028}cd");
    editor.pane_tree.set_focused_point(2);
    editor.execute(Command::KillLine);
    assert_eq!(editor.buffer_text(), "ab");
    assert_eq!(editor.clipboard, "\u{2028}cd");
}

#[test]
fn next_line_clamps_column_counting_form_feed_as_content() {
    // Line 1 is "ab\u{0c}x" — four chars, the FF included.
    let mut editor = Editor::new_with_text("hello\nab\u{0c}x");
    editor.pane_tree.set_focused_point(4);
    editor.execute(Command::NextLine);
    assert_eq!(editor.point(), 10, "col 4 exists; the FF counts");
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
    drive_isearch_query(&mut editor, "wörld");
    // "wörld" ends at char index 11 (point goes to match end), and the
    // match starts at char index 6 — not at the byte offsets 13/8.
    let state = editor.isearch.as_ref().unwrap();
    assert_eq!(state.current_match(), Some(6));
    assert_eq!(editor.point(), 6);
}

#[test]
fn isearch_backward_non_ascii() {
    let mut editor = Editor::new_with_text("ééé aaa ééé");
    editor.execute(Command::BufferEnd);
    editor.execute(Command::ISearchBackward);
    drive_isearch_query(&mut editor, "ééé");
    let state = editor.isearch.as_ref().unwrap();
    assert_eq!(state.current_match(), Some(8));
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
