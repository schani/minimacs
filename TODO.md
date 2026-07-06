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
- [x] Wide-character and grapheme-cluster support: `unicode-width` and `unicode-segmentation` are declared in Cargo.toml but never imported anywhere in src/. `visual_width_for_chars` (`render.rs:187-195`) counts every non-tab char as width 1, so CJK/emoji lines get wrong cursor position, wrap points, mouse mapping, and highlight columns; cursor movement and backspace step through combining sequences one scalar at a time.
- [x] Scroll-to-keep-cursor-visible counts raw chars (`editor.rs:320`) while the renderer wraps by tab-expanded visual width (`render.rs:118-119`), so on tab-heavy files the cursor can move below the viewport without scrolling (reintroduces the previously fixed "cursor beyond last line" bug for tabs).
- [x] Cursor row off-by-one when point is in a full final wrap segment (`render.rs:126-131`): cursor is drawn on top of the next buffer line's first column.
- [x] Cursor bounds check mixes pane-relative and absolute coordinates (`render.rs:138`): cursor disappears at EOL of a width-filling line in the leftmost pane, and is drawn one cell outside the pane (on the separator) in right-hand panes.
- [x] Bind a key to `Command::Redo` — it is fully implemented and wired in `execute()` but `default_keymap()` binds nothing to it, so users cannot redo at all.
- [x] An unbound key after a prefix self-inserts: `C-x j` inserts a literal `j` and marks the buffer modified (`app.rs:158-168`). Should report "C-x j is undefined" instead.
- [x] Expand `~` in find-file/write-file paths; currently `~/foo` is treated as a literal relative path and tab completion silently does nothing.
- [x] Incremental search materializes the whole buffer into a `String` on every keystroke (`editor.rs:1429`, `1479`) and `isearch_matches()` does it again every rendered frame (`editor.rs:1528-1558`, called from `render.rs:95`). Large files crawl during isearch.
- [x] Idle loop renders ~10×/second forever (100ms poll in `event.rs:14` + unconditional render in `app.rs:37-63`), and per-frame work includes O(buffer) paths (cursor-row loop unbounded by viewport height in `render.rs:117-119`; style map rebuilt from all cached spans even on syntax-cache hits, `render.rs:472-507`). Only render after an event or when state changed, and bound per-frame work by the viewport.

## Minor

- [x] Deletions split CRLF pairs: `kill_line` at EOL, `delete_forward`, and `delete_backward` remove a single char, turning `\r\n` into `\n` and mixing line endings; `forward_char` can place point between `\r` and `\n`.
- [x] `common_prefix` (`minibuffer.rs:205-225`) is only correct for sorted input; `complete_buffer_with_candidates` passes matches in creation order, so TAB can rewrite the minibuffer to a "prefix" that excludes a valid match.
- [x] Undo does not restore mark and only sets point on the active pane (`editor.rs:1198-1200`). (Resolved by the apply_edit refactor: undo replay adjusts point/mark in every pane with marker semantics, keeping them valid; exact mark restoration is not emacs behavior and is not attempted.)
- [x] The pre-commit coverage check silently degrades to plain `cargo test` when `cargo-llvm-cov` is not installed, and `build.rs` never updates a stale hook (`build.rs:7,17-21`). The hook also tests the working tree, not the staged index. (Fixed: the hook now warns loudly when the coverage tool is missing, and build.rs rewrites the hook whenever its content differs. Testing the staged index instead of the working tree is intentionally not done — it would require a full rebuild per commit.)
- [x] Update ARCHITECTURE.md: it references a nonexistent `handle_minibuffer_key()` (line 209), describes outdated colors for region/match highlighting and mode lines (lines 243-251), and the claim "unconditional rendering is cheap" (line 9) is contradicted by the per-frame O(buffer) paths above.

# From code review (2026-07-06)

Order matters: the two structural refactors go first (they are the root cause
of the data-loss bugs), then the data-loss fixes land on the unified paths,
then the correctness fixes. One commit per item; tests first; keep
ARCHITECTURE.md current.

## Structural refactors

- [ ] Unify all file-writing flows behind a single safe-save choke point on `Editor`. Today `Save` (`fileops.rs:170`), `write_buffer_to_path` (`prompts.rs:244`), `SaveAnywayConfirm`, `OverwriteConfirm`, and `QuitSaveConfirm` (`prompts.rs:189`) each call `Buffer::save`/`save_as` directly, which is why the `externally_modified()` guard exists on exactly one of five flows. Introduce one method every flow calls (behavior-preserving in this step — the guard lands in the next item). Add characterization tests for each flow before refactoring.
- [ ] Unify input state in `app.rs`. Prefix-chord state (`keymap_state`), `esc_pending`, isearch key interception, paste, and mouse handling each mutate partially overlapping state with implicit invariants; only `handle_key` ever clears the pending-chord state. Gather these into one explicit input-state struct with a single reset point, and route every event kind (key, paste, mouse, resize) through one dispatcher that decides what clears what. Behavior-preserving (the stale-prefix and isearch-paste fixes below land on top).

## Data loss

- [ ] External-modification guard on every save flow: quitting with `C-x C-c` and answering "y" (`prompts.rs:192`), and `C-x C-w` to the buffer's own path (`prompts.rs:113,249`), call `Buffer::save`/`save_as` without checking `externally_modified()`, silently clobbering changes another program wrote to disk. Only `C-x C-s` checks. Route all flows through the unified safe-save path's guard, prompting "changed on disk; save anyway?" like `C-x C-s` does.
- [ ] `normalize_path_string` silently drops leading `..` on relative paths (`minibuffer.rs:83-87`): `ParentDir` pops only when a prior component exists, so `../notes.txt` becomes `notes.txt` and find-file/write-file open or save the wrong file. Preserve leading `..` components for rootless paths (and resolve them against the effective base directory where the result is used).

## Correctness

- [ ] Pasting during isearch desyncs query from display: `handle_paste` (`app.rs:283-296`) never checks `editor.isearch`, so pasted text lands in the minibuffer text but `ISearchState::query` is not updated and no search runs. Route paste through isearch: append the (sanitized) text to the query and call `isearch_update`.
- [ ] Stale key-prefix state survives paste and mouse events: only `handle_key` clears `keymap_state`/`esc_pending` (`app.rs:99`). `C-x` then paste then `C-s` executes `C-x C-s`; clicking another pane mid-chord retargets the chord. Paste and mouse events must cancel any pending chord and pending ESC.
- [ ] A single buffer line that wraps taller than the viewport traps the cursor off-screen: `compute_scroll_top` (`pane.rs:47-52`) is line-granular, so in a one-line minified file `M->` leaves point on a visual row far below the pane with no recovery, and the wheel is dead. Support sub-line scrolling (scroll offset in visual rows within the top line) so the cursor's visual row can always be brought into view. Note: `scroll_top` is line-based in `pane.rs:24`, `editor.rs:353`, `render.rs:122`, and `app.rs:404` (mouse mapping) — all four must agree on the new representation. This is refactor-sized; it must land before the EOL-cursor-wrap fix below, which depends on it.
- [ ] Cursor invisible at EOL of a line exactly filling the pane width: cursor col computes to `text_width`, the `screen_col < text_area.width` guard (`render.rs:164`) fails, and `set_cursor_position` is never called. Wrap the cursor to column 0 of the next visual row instead of hiding it — requires the sub-line scrolling item above so that row can always be scrolled into view.
- [ ] Messages emitted while a prompt is active are invisible, then reappear stale after the prompt finishes: `show_message` during a prompt (e.g. "Failing I-search", `isearch.rs:136`) is not rendered, and `Minibuffer::finish()` doesn't clear `message`. Failing isearch must be visible during the search (emacs shows it in the prompt label); prompt finish/cancel must clear stale messages.
- [ ] `kill_line` with nothing to kill still overwrites the OS clipboard with stale content (`editor.rs:1099-1101`); only touch the clipboard when text was actually killed. Also `last_command` survives prompt exits, so `C-k` in a prompt followed by `C-k` in a buffer wrongly appends the kills — reset `last_command` on *every* prompt exit path: `submit_prompt()` (`prompts.rs:82`), `isearch_accept()` (`isearch.rs:173`), and prompt cancel (`editor.rs:1254`).
- [ ] Atomic save replaces the inode: saving over a symlink destroys the link (writes a regular file in its place) and hard links diverge (`buffer.rs:160-167`). Separate the buffer's logical path from the physical write target: resolve symlinks at write time (in the editor-level safe-save choke point) without mutating `Buffer::path` to the canonicalized form (`C-x C-w` through a symlink must not silently rename the buffer). When the resolved target has other hard links (nlink > 1), fall back to in-place truncate-write and document the tradeoff.
- [ ] Line-ending handling disagrees with ropey's line-break set in three places: `line_len_chars` (`buffer.rs:229-246`), the renderer's `line_chars_without_ending` (`render.rs:210`), and syntax-line extraction (`render.rs:581`) all strip only `\n`/`\r\n`, but ropey also breaks on `\r`, `\x0b`, `\x0c`, `\u{85}`, `\u{2028}`, `\u{2029}` — so `C-e`/`C-k`, display, and mouse mapping are all off by one on such lines. Fix all three consumers with one shared "strip line break" helper matching ropey's set.
- [ ] Filter key events to `KeyEventKind::Press`: nothing checks `key.kind`, so on Windows (or kitty-protocol terminals reporting release events) every keystroke executes twice.
- [ ] Vertical movement (`next_line`/`previous_line`/`page_up`/`page_down`, `editor.rs:574-658`) can park point mid-grapheme-cluster via raw-char column clamping; backspace then orphans a combining mark. Snap the landing position to a grapheme boundary.
- [ ] A dead event source busy-spins forever: `TerminalEventSource::next_event` (`event.rs:14-20`) converts poll/read errors into `None`, which the run loop treats as a timeout. Distinguish errors from timeouts and exit the loop (terminal teardown still runs in `main`).
- [ ] Mouse motion causes a render storm: `EnableMouseCapture` enables any-motion tracking, and events discarded by `handle_mouse` (`app.rs:298-306`) still trigger a full render. Only render after events that could have changed state (handlers report whether they did anything).
- [ ] Completion layout measures display width with `chars().count()` (`render.rs:31-35, 681-733`), so CJK/emoji candidate names misalign columns and overflow rows. Use `unicode-width` like the mode line already does.
- [ ] Submitting an empty path creates garbage in both path prompts: empty find-file input creates a phantom, unsaveable buffer with an empty name (`prompts.rs:97-103` → `fileops.rs:9-35`), and empty write-file input falls through to `write_buffer_to_path(PathBuf::new())` (`prompts.rs:108,242`). Add one shared "non-empty normalized path" validation both prompts go through; on empty input show a message and re-ask.
- [ ] `main.rs` CLI/teardown gaps: `minimacs a.txt b.txt` silently ignores every file after the first (open them all); `--help`/`--version` open as literal buffer names (print usage/version and exit); an `Err` between `enable_raw_mode()` and entering the run loop returns early without `restore_terminal()`, leaving the shell raw (the panic hook only covers panics).