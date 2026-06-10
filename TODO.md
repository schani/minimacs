- [x] When switching buffers with `C-x b`, when the user just presses enter without picking a buffer, switch to the buffer that was last visited in that window.
- [x] When switching to a buffer, point needs to go to where it was the last time the buffer was visited in that window.
- [x] Find-file should start at the directory of the current buffer, not from where minimacs was started.
- [x] It's possible to move the cursor beyond the last line on the screen without it scrolling. I belive that happens when some lines on the screen overflow.
- [x] Ctrl-_ does not undo in some terminals
- [x] Killing with Ctrl-k and then yanking with Ctrl-y doesn't yank the killed text, but the OS's clipboard. Either the killing should kill "into" the OS clipboard, or yanking should ignore the OS clipboard if there's stuff in the kill ring. Let's discuss. Also: How does Emacs do this?
- [x] Horizontal splits should be separated by a bar
- [x] Support mouse scrolling - the pane that the mouse is over should scroll, but the focused pane shouldn't change
- [x] Remove the line number gutter
- [x] Implement: C-l (recenter-top-bottom): This command clears the screen and redisplays the current line at the top, center, or bottom of the window. It also serves to generally refresh the display in the process. Repeating C-l cycles the current line's position.
- [x] Make the color pallette equivalent to "VSCode Light+". We only need to support "light" terminals right now.
- [x] Find file should not accept `.` and `..` as parts of the path, and should also not use them as completions
- [x] Support mouse clicks, which should place the cursor at the point clicked at. It probably shouldn't do that when the minibuffer is active.
- [x] Ctrl-/ to undo doesn't work
- [x] Alt-Backspace doesn't delete word backwards
- [x] Add a pre-commit hook that runs the build and unit tests, and doesn't allow committing if they fail
- [x] Enforce unit test coverage threshold
- [x] Invoking the editor with a filename that doesn't exist should still open a buffer for that file, and saving it should save to that file
- [x] When a file is unchanged, doing something, and then undoing that, should put it back into the unchanged state

# From code review (2026-06-10)

Larger items (incremental tree-sitter parsing, missing emacs features) are in FUTURE.md.

## Critical — crashes, corruption, data loss

- [x] Install a panic hook that restores the terminal (raw mode, alternate screen, mouse capture) before printing the panic message. Currently teardown only happens on `main()`'s normal return path (`main.rs:47-77`), so any panic leaves the shell garbled and the message swallowed by the alternate screen.
- [x] Big refactor: introduce a single centralized edit primitive on `Editor` that every editing command goes through. It must (a) operate in char indices only — never add `String::len()` (bytes) to a position, (b) record history, (c) adjust point and mark in *all* panes viewing the edited buffer (plus saved per-pane `buffer_states`), and (d) clamp to buffer bounds. Buffer-mutation + history-recording + point-update is currently hand-repeated in every edit command, which is the structural cause of the next four items.
- [x] Undo/redo mix char positions with byte lengths (`editor.rs:1190`, `1215`, `1224`): undoing an edit containing non-ASCII text panics at buffer end and silently corrupts text mid-buffer (verified: "abécd" undoes to "abd"). Redo also sets point using byte length.
- [x] `C-y` paste sets `point = pos + text.len()` in bytes (`editor.rs:1349`); pasting non-ASCII text near buffer end places point past the end and the next keystroke panics. (The bracketed-paste path in `app.rs:289` correctly uses `chars().count()`.)
- [x] Undo-grouping adjacency check uses byte length (`history.rs:66`) — after typing one multibyte char, every subsequent keystroke lands in its own undo group.
- [x] Editing a buffer never adjusts point/mark in other panes showing it (no mechanism exists; panes are only clamped on buffer switch, `pane.rs:117-128`). Verified panic: split pane, point at end in pane B, cut all text from pane A, type in pane B.
- [x] Answering the kill-buffer confirmation quits the entire editor: `kill_buffer` (`editor.rs:515-534`) reuses `PromptKind::SaveConfirm`, whose only handler (`editor.rs:467-489`) is the quit flow — "y" saves and quits, "n" quits discarding. The buffer is never killed. The prompt handler needs to know which flow it belongs to.
- [x] Quitting with multiple modified buffers prompts only for the first and silently discards the changes of all the others.
- [x] The `SaveConfirm` handler looks the buffer up by name (`editor.rs:471`); buffer names are file basenames and never uniquified, so with two files of the same basename it can save the wrong buffer. Uniquify buffer names (emacs style, e.g. `mod.rs<lib>`) — duplicate names also make the second buffer unreachable via `C-x b`.
- [x] Completion rendering byte-slices candidate strings at arbitrary indices (`render.rs:624`, `639`); tab-completing in a directory with non-ASCII filenames in a narrow terminal panics. Also `completions_layout` divides by zero when terminal width is 0 (`render.rs:17-18`).
- [x] Add non-ASCII test data across the suite (insert, delete, undo, paste, search, completion, rendering). There is currently zero Unicode text in any test, which is why the byte-vs-char bugs survived.

## Major

- [x] Atomic saves: write to a temp file in the same directory and rename over the target (`buffer.rs:142` currently truncates in place with `fs::write`, so a crash or full disk mid-write destroys the file). Consider fsync.
- [x] Detect external file modification: remember mtime at load/save and warn before saving over (or editing) a file changed on disk. There is also no revert-buffer command.
- [x] `C-x C-w` mutates the buffer's path and name *before* knowing the save succeeded (`editor.rs:440-449`), silently overwrites existing files without confirmation, and never re-detects syntax for the new extension.
- [ ] Wide-character and grapheme-cluster support: `unicode-width` and `unicode-segmentation` are declared in Cargo.toml but never imported anywhere in src/. `visual_width_for_chars` (`render.rs:187-195`) counts every non-tab char as width 1, so CJK/emoji lines get wrong cursor position, wrap points, mouse mapping, and highlight columns; cursor movement and backspace step through combining sequences one scalar at a time.
- [x] Scroll-to-keep-cursor-visible counts raw chars (`editor.rs:320`) while the renderer wraps by tab-expanded visual width (`render.rs:118-119`), so on tab-heavy files the cursor can move below the viewport without scrolling (reintroduces the previously fixed "cursor beyond last line" bug for tabs).
- [x] Cursor row off-by-one when point is in a full final wrap segment (`render.rs:126-131`): cursor is drawn on top of the next buffer line's first column.
- [x] Cursor bounds check mixes pane-relative and absolute coordinates (`render.rs:138`): cursor disappears at EOL of a width-filling line in the leftmost pane, and is drawn one cell outside the pane (on the separator) in right-hand panes.
- [x] Bind a key to `Command::Redo` — it is fully implemented and wired in `execute()` but `default_keymap()` binds nothing to it, so users cannot redo at all.
- [x] An unbound key after a prefix self-inserts: `C-x j` inserts a literal `j` and marks the buffer modified (`app.rs:158-168`). Should report "C-x j is undefined" instead.
- [x] Expand `~` in find-file/write-file paths; currently `~/foo` is treated as a literal relative path and tab completion silently does nothing.
- [ ] Incremental search materializes the whole buffer into a `String` on every keystroke (`editor.rs:1429`, `1479`) and `isearch_matches()` does it again every rendered frame (`editor.rs:1528-1558`, called from `render.rs:95`). Large files crawl during isearch.
- [ ] Idle loop renders ~10×/second forever (100ms poll in `event.rs:14` + unconditional render in `app.rs:37-63`), and per-frame work includes O(buffer) paths (cursor-row loop unbounded by viewport height in `render.rs:117-119`; style map rebuilt from all cached spans even on syntax-cache hits, `render.rs:472-507`). Only render after an event or when state changed, and bound per-frame work by the viewport.

## Minor

- [ ] Deletions split CRLF pairs: `kill_line` at EOL, `delete_forward`, and `delete_backward` remove a single char, turning `\r\n` into `\n` and mixing line endings; `forward_char` can place point between `\r` and `\n`.
- [ ] `common_prefix` (`minibuffer.rs:205-225`) is only correct for sorted input; `complete_buffer_with_candidates` passes matches in creation order, so TAB can rewrite the minibuffer to a "prefix" that excludes a valid match.
- [ ] Undo does not restore mark and only sets point on the active pane (`editor.rs:1198-1200`).
- [ ] The pre-commit coverage check silently degrades to plain `cargo test` when `cargo-llvm-cov` is not installed, and `build.rs` never updates a stale hook (`build.rs:7,17-21`). The hook also tests the working tree, not the staged index.
- [ ] Update ARCHITECTURE.md: it references a nonexistent `handle_minibuffer_key()` (line 209), describes outdated colors for region/match highlighting and mode lines (lines 243-251), and the claim "unconditional rendering is cheap" (line 9) is contradicted by the per-frame O(buffer) paths above.