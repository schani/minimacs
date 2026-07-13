- [x] `M-<` and `M->` don't work with the "option" key on macOS/cmux. They do work with "ESC", but they should also work with "option". (Done: kitty-protocol terminals report Alt+Shift+`,` as the *base* key with ALT|SHIFT; `Key::from_event` now resolves SHIFT into the shifted char (Unicode uppercase + US-layout punctuation table), and `main()` additionally pushes `REPORT_ALTERNATE_KEYS` so conforming terminals send the layout-correct shifted char themselves.)

# LF-only line endings (product decision, 2026-07-10)

**Decision:** minimacs only needs to support LF line endings correctly.
In-memory text (the rope) is invariantly LF-only: CRLF is a *file encoding*
handled at the load/save boundary (detected at load and converted to LF;
converted back on save when the buffer's `LineEnding` is CrLf — the emacs
model). Ropey's `unicode_lines` extras (lone CR, VT, FF, NEL, LS, PS) are
**not** line breaks; they are ordinary characters. Consequences accepted:
mixed-line-ending files are normalized to one ending on save; a lone `\r`
(mac-classic files) is not a line break and renders as an ordinary char.
Payoffs: ropey pinned to `default-features = false` (like Helix), ropey
line rows become *exactly* tree-sitter Point rows for arbitrary content
(the incremental-parsing row caveat disappears), and CRLF special-casing
in movement/rendering/paste is deleted.

Plan, in order (tests first for each step; one commit per step).
**Done (2026-07-10)** — landed as five commits; the decision is recorded
in ARCHITECTURE.md's Buffer section. Deep fuzz sweep after the flip
(`--lang all --runs 8 --steps 400`): 92/104 runs clean, 12 with the known
transient upstream error-recovery divergences (raw-probe-confirmed,
self-healing), same picture as before the migration.

- [x] Normalize at the file boundary. Tests first: loading a CRLF file
      yields a rope containing no `\r\n` with `line_ending == CrLf`; save
      writes CRLF back; pure-LF and pure-CRLF files round-trip
      byte-identical; a mixed-endings file loads LF-only and saves
      uniformly (intended normalization); the revert/external-reload path
      normalizes too (verify all file reads share `Buffer::from_file`).
      Implementation: `from_file` does `content.replace("\r\n", "\n")`
      after detection; `save_as` converts `\n` → `\r\n` for CrLf buffers
      at write time (lone `\r` content is left untouched in both
      directions). Ropey stays on default features in this step.
- [x] Simplify editing on the LF-only invariant. `insert_text`'s paste
      conversion to the buffer ending goes away (unify to `\n` only,
      editor.rs:1253-1264); RET always inserts `\n`; delete `inside_crlf`
      and CRLF-pair atomicity in `next/prev_grapheme_boundary`
      (editor.rs:429-454); drop CRLF from the direct-insertion test
      matrices (buffer.rs:528, render.rs:1040) — CRLF can no longer occur
      in a rope. `line_break_len_chars` keeps its CRLF arm until the next
      step only if any test still constructs one directly; otherwise
      simplify here.
- [x] Flip ropey to `default-features = false, features = ["simd"]`.
      Tests first: rewrite the ~15 unicode-line-break tests to assert the
      *new* behavior — lone CR / VT / FF / NEL / LS / PS are ordinary
      content: one ropey line, `C-e` moves past them, `C-k` kills through
      them, rendering and mouse mapping treat them as width-bearing chars.
      Then: `is_line_break_char` reduces to `\n`; `line_break_len_chars`
      becomes a trailing-`\n` check (0 or 1); simplify its consumers
      (`line_len_chars`, renderer line extraction, kill-line).
- [x] Verify and document exact tree-sitter row agreement: with LF-only
      ropey, `tree_sitter_point_at_char`'s rows equal tree-sitter Point
      rows for *arbitrary* content — update its comment, and drop the
      benign-divergence caveats from ARCHITECTURE.md and the fuzz-harness
      notes. Run `syntax-fuzz` (default sweep + a deep
      `--runs 16 --steps 500` pass) — the adverse alphabet (lone CR,
      U+2028/2029, FF, CRLF fragments) stays: those chars are exactly the
      ones that changed meaning, now stressing the content-not-break
      paths. Add a fuzz oracle assertion if cheap: rope `len_lines` - 1
      == count of `\n`.
- [x] Docs sweep: rewrite ARCHITECTURE.md's line-break paragraph (Buffer
      section), the grapheme-cluster CRLF-atomicity mention, and the
      paste-normalization paragraph to describe the LF-only invariant;
      check README/FUTURE for stale mentions. Decide/verify how
      now-inline control chars (`\r`, FF, VT) render — passing them raw
      to the terminal is acceptable per the decision, but note it as a
      known limitation.

# Long single-line rendering (2026-07-12)

- [x] Add a release-mode benchmark for an exact 5 MiB, syntax-highlighted,
      single-line JSON buffer, measuring cold and repeated renders at both the
      beginning and far end.
- [x] Materialize only viewport rows and their syntax styles; centralize wrap,
      cursor, scrolling, and mouse geometry in `VisualLineLayout`; directly
      index printable ASCII lines and keep the Unicode/tab fallback
      memory-bounded. Far-end repeated rendering measured under 9ms on the
      development machine, so no checkpoint cache or invalidation state was
      added.

# Parse-failure handling (from PR review, 2026-07-09)

Problem: when `SyntaxState::ensure_syntax` fails — most plausibly by hitting
`PARSE_TIMEOUT` (2s) on a pathological or very large file — nothing is cached,
so every subsequent render retries the full parse: up to 2 seconds of
synchronous stall per frame, forever, on exactly the files where the parser
struggles. A repeatedly failing `Syntax::update` has the same shape (each
failure drops the tree; the next render re-runs a full 2s-capped parse).

Plan, in order (tests first for each step):

- [ ] Make the parse timeout injectable for tests (e.g. a
      `SyntaxState::with_timeout(lang, Duration)` constructor used by tests;
      `new()` keeps `PARSE_TIMEOUT`). With `Duration::ZERO`, assert the
      failure path: `highlight_rope` returns unstyled spans and does not
      panic. This is pure test plumbing — no behavior change.
- [ ] Poison the failed generation: on `ensure_syntax` failure, record the
      requested `version` in a `failed_version: Cell<Option<usize>>`. While
      `failed_version == Some(version)`, `highlight_rope` returns empty spans
      without attempting to parse. An edit bumps the generation and clears the
      poison, so retries happen at most once per edit — never per frame.
      Tests: with a zero timeout, the parse is attempted exactly once per
      generation (extend the existing `#[cfg(test)]` parse counters); cursor
      movement and idle frames after a failure do not stall.
- [ ] Give up after N consecutive failed generations (suggest N=3): set a
      permanent `disabled` flag, show "syntax highlighting disabled (parse
      timeout)" in the echo area once, and stop retrying entirely.
      `redetect_syntax` (used by save-as) resets the flag, giving users an
      explicit re-enable path. Tests: N failing edits disable it; the message
      appears once; redetect re-arms.
- [ ] Reconsider `PARSE_TIMEOUT`: 2s is far beyond an acceptable frame stall.
      With the poison flag the retry storm is gone, but a single 2s stall per
      edit on a pathological file is still bad. Suggest 500ms for the initial
      parse and keeping `Syntax::update` at 500ms too; measure on the
      syntax-bench 100k-line workload before choosing. Document the choice in
      ARCHITECTURE.md.
- [ ] (Future, complementary — already sketched in FUTURE.md) Move parsing to
      a background thread so even the first parse of a huge file never blocks
      input; the poison flag remains useful as the fallback for the
      synchronous path.

- [x] Wire `syntax-fuzz` (and a small `syntax-bench --lines 500 --edits 50`
      checksum run) into CI as a smoke check: e.g.
      `syntax-fuzz --runs 2 --steps 120` on the default languages, treating
      exit code 1 as failure. Known limitation to document: on heavily
      corrupted buffers tree-sitter's incremental error recovery can
      transiently produce different trees than a fresh parse (verified
      upstream with the raw-tree probe), so CI should run with modest step
      counts where the default languages are clean, and bumps to the pinned
      grammar/tree-house versions should re-run the deep sweep.
      (Done: `.github/workflows/ci.yml` runs on PRs and pushes to `main`; a
      `syntax-smoke` job runs `syntax-fuzz --runs 2 --steps 120` and
      `syntax-bench --lines 500 --edits 50` in release mode, with the
      modest-step-count rationale documented in the workflow and README.
      The main `test` job mirrors the pre-commit hook: build,
      `cargo llvm-cov --fail-under-lines 90`, `clippy -D warnings`.)
- [ ] Track the tree-sitter-md external-scanner overflow: the fuzz harness's
      raw probe segfaulted inside `tree_sitter_markdown_external_scanner_serialize`
      (memmove past the serialization buffer) during an incremental parse
      from a stale tree with deeply nested block state. minimacs parses the
      same grammar incrementally through tree-house, so the crash class is
      reachable in the editor in principle (not reproduced through tree-house
      in 16x500-step fuzz runs). Watch upstream tree-sitter-md for a scanner
      fix and bump the pinned version; re-run `syntax-fuzz --lang markdown
      --raw` afterwards to confirm.

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

- [x] Unify all file-writing flows behind a single safe-save choke point on `Editor`. Today `Save` (`fileops.rs:170`), `write_buffer_to_path` (`prompts.rs:244`), `SaveAnywayConfirm`, `OverwriteConfirm`, and `QuitSaveConfirm` (`prompts.rs:189`) each call `Buffer::save`/`save_as` directly, which is why the `externally_modified()` guard exists on exactly one of five flows. Introduce one method every flow calls (behavior-preserving in this step — the guard lands in the next item). Add characterization tests for each flow before refactoring. (Done: all five flows now route through `Editor::write_buffer(buffer_id, WriteTarget)` in fileops.rs, with `WriteTarget::BufferPath` vs `::Path` keeping the logical path separate from the write target; characterization tests added first.)
- [x] Unify input state in `app.rs`. Prefix-chord state (`keymap_state`), `esc_pending`, isearch key interception, paste, and mouse handling each mutate partially overlapping state with implicit invariants; only `handle_key` ever clears the pending-chord state. Gather these into one explicit input-state struct with a single reset point, and route every event kind (key, paste, mouse, resize) through one dispatcher that decides what clears what. Behavior-preserving (the stale-prefix and isearch-paste fixes below land on top). (Done: `App` owns an `InputState` (chord walk + pending ESC, mirroring `editor.pending_keys` for the mode line) with `reset()` as the single clear point; all events route through `dispatch_event()`, whose paste/mouse arms deliberately don't reset pending input yet — characterization tests pin that buggy behavior for the two fix items below.)

## Data loss

- [x] External-modification guard on every save flow: quitting with `C-x C-c` and answering "y" (`prompts.rs:192`), and `C-x C-w` to the buffer's own path (`prompts.rs:113,249`), call `Buffer::save`/`save_as` without checking `externally_modified()`, silently clobbering changes another program wrote to disk. Only `C-x C-s` checks. Route all flows through the unified safe-save path's guard, prompting "changed on disk; save anyway?" like `C-x C-s` does. (Done: all own-path save flows now call `Editor::external_modification_guard`; `SaveAnywayConfirm` gained a `resume_quit` flag so the quit flow chains through the guard — "y" saves and resumes the quit, "n" cancels it like a failed quit-time save.)
- [x] `normalize_path_string` silently drops leading `..` on relative paths (`minibuffer.rs:83-87`): `ParentDir` pops only when a prior component exists, so `../notes.txt` becomes `notes.txt` and find-file/write-file open or save the wrong file. Preserve leading `..` components for rootless paths (and resolve them against the effective base directory where the result is used). (Done: leading `..` is preserved on rootless paths (rooted paths still clamp at `/`); find-file/write-file submission resolves relative input against `Editor::cwd` via `path_from_input`, and tab completion looks relative input up against the same base while keeping the minibuffer text relative.)

## Correctness

- [x] Pasting during isearch desyncs query from display: `handle_paste` (`app.rs:283-296`) never checks `editor.isearch`, so pasted text lands in the minibuffer text but `ISearchState::query` is not updated and no search runs. Route paste through isearch: append the (sanitized) text to the query and call `isearch_update`. (Done: the dispatcher's paste arm routes to `Editor::isearch_yank`, which normalizes line breaks to spaces, appends to the query, syncs the minibuffer display, and re-runs `isearch_update`.)
- [x] Stale key-prefix state survives paste and mouse events: only `handle_key` clears `keymap_state`/`esc_pending` (`app.rs:99`). `C-x` then paste then `C-s` executes `C-x C-s`; clicking another pane mid-chord retargets the chord. Paste and mouse events must cancel any pending chord and pending ESC. (Done: the dispatcher's paste and mouse arms call `InputState::reset` before handling (cancel-then-handle), so a mid-chord paste/click cancels the chord and pending ESC yet still takes effect; resize still leaves chords alone, and isearch paste routing is untouched.)
- [x] A single buffer line that wraps taller than the viewport traps the cursor off-screen: `compute_scroll_top` (`pane.rs:47-52`) is line-granular, so in a one-line minified file `M->` leaves point on a visual row far below the pane with no recovery, and the wheel is dead. Support sub-line scrolling (scroll offset in visual rows within the top line) so the cursor's visual row can always be brought into view. Note: `scroll_top` is line-based in `pane.rs:24`, `editor.rs:353`, `render.rs:122`, and `app.rs:404` (mouse mapping) — all four must agree on the new representation. This is refactor-sized; it must land before the EOL-cursor-wrap fix below, which depends on it. (Done: `Pane` gained `scroll_row_offset` (visual rows of the wrapping top line scrolled off above); `compute_scroll_position` replaces `compute_scroll_top`, the renderer skips/subtracts the offset, mouse click adds it, the wheel scrolls by 3 visual rows via `scroll_{down,up}_visual_rows`, and stale offsets are clamped by every consumer.)
- [x] Cursor invisible at EOL of a line exactly filling the pane width: cursor col computes to `text_width`, the `screen_col < text_area.width` guard (`render.rs:164`) fails, and `set_cursor_position` is never called. Wrap the cursor to column 0 of the next visual row instead of hiding it — requires the sub-line scrolling item above so that row can always be scrolled into view. (Done: `visual_row_col_in_line` wraps a column of `text_width` — EOL of an exactly-full line or final wrap segment — to `(row + 1, 0)`; cursor placement and `compute_scroll_position` share it, so the extra row also scrolls into view at the viewport bottom, and mouse clicks on that row map to what it displays.)
- [x] Messages emitted while a prompt is active are invisible, then reappear stale after the prompt finishes: `show_message` during a prompt (e.g. "Failing I-search", `isearch.rs:136`) is not rendered, and `Minibuffer::finish()` doesn't clear `message`. Failing isearch must be visible during the search (emacs shows it in the prompt label); prompt finish/cancel must clear stale messages. (Done: the isearch prompt label is live — `isearch_sync_label` shows "Failing I-search [backward]: " from `ISearchState::failing` instead of queueing messages — and `Minibuffer::finish()` now clears `message`, so nothing queued during a prompt reappears; handlers still show result messages after `finish()`.)
- [x] `kill_line` with nothing to kill still overwrites the OS clipboard with stale content (`editor.rs:1099-1101`); only touch the clipboard when text was actually killed. Also `last_command` survives prompt exits, so `C-k` in a prompt followed by `C-k` in a buffer wrongly appends the kills — reset `last_command` on *every* prompt exit path: `submit_prompt()` (`prompts.rs:82`), `isearch_accept()` (`isearch.rs:173`), and prompt cancel (`editor.rs:1254`). (Done: a no-op `C-k` shows "End of buffer", touches neither clipboard, and doesn't arm the kill chain; `submit_prompt()` and `isearch_accept()` reset `last_command`, while C-g already resets it by running `Command::Cancel` through `execute()` — pinned by a regression test.)
- [x] Atomic save replaces the inode: saving over a symlink destroys the link (writes a regular file in its place) and hard links diverge (`buffer.rs:160-167`). Separate the buffer's logical path from the physical write target: resolve symlinks at write time (in the editor-level safe-save choke point) without mutating `Buffer::path` to the canonicalized form (`C-x C-w` through a symlink must not silently rename the buffer). When the resolved target has other hard links (nlink > 1), fall back to in-place truncate-write and document the tradeoff. (Done: `Buffer::save_as` — the single physical writer all `write_buffer` flows reach — resolves symlink chains (dangling ones too) via `resolve_write_target`, keeps the passed logical path as buffer identity, captures mtime from the physical file, and truncate-writes hard-linked targets in place, trading crash-atomicity for keeping the inode shared.)
- [x] Line-ending handling disagrees with ropey's line-break set in three places: `line_len_chars` (`buffer.rs:229-246`), the renderer's `line_chars_without_ending` (`render.rs:210`), and syntax-line extraction (`render.rs:581`) all strip only `\n`/`\r\n`, but ropey also breaks on `\r`, `\x0b`, `\x0c`, `\u{85}`, `\u{2028}`, `\u{2029}` — so `C-e`/`C-k`, display, and mouse mapping are all off by one on such lines. Fix all three consumers with one shared "strip line break" helper matching ropey's set. (Done: `buffer::line_break_len_chars(line)` is the single authority for ropey's full break set; `line_len_chars`, both renderer sites, and kill-line's at-EOL break removal use it, so `C-e`, column clamping, display, mouse mapping, and `C-k` agree with ropey's line boundaries.)
- [x] Filter key events to `KeyEventKind::Press`: nothing checks `key.kind`, so on Windows (or kitty-protocol terminals reporting release events) every keystroke executes twice. (Done: the `Event::Key` arm of `dispatch_event` drops `KeyEventKind::Release` before it reaches `handle_key` — Press and Repeat still execute, so held keys keep repeating — pinned by typing/isearch/mid-chord release tests.)
- [x] Vertical movement (`next_line`/`previous_line`/`page_up`/`page_down`, `editor.rs:574-658`) can park point mid-grapheme-cluster via raw-char column clamping; backspace then orphans a combining mark. Snap the landing position to a grapheme boundary. (Done: `Buffer::snap_to_grapheme_boundary` snaps to the boundary at or before the raw column; all four vertical movements and mouse-click mapping (which could land mid-ZWJ-sequence) use it, while `preferred_column` keeps the unsnapped goal column. Note: `forward_word`/`backward_word` can still stop before a combining mark — a word-syntax definition question, left open.)
- [x] A dead event source busy-spins forever: `TerminalEventSource::next_event` (`event.rs:14-20`) converts poll/read errors into `None`, which the run loop treats as a timeout. Distinguish errors from timeouts and exit the loop (terminal teardown still runs in `main`). (Done: `next_event` now returns `Poll { Event, Timeout, Closed }`; poll/read errors map to `Closed`, on which `run()` bails with "event source closed" — after `restore_terminal()` in main — while timeouts keep idling; `TestEventSource` reports `Closed` on a drained queue, preserving test semantics.)
- [x] Mouse motion causes a render storm: `EnableMouseCapture` enables any-motion tracking, and events discarded by `handle_mouse` (`app.rs:298-306`) still trigger a full render. Only render after events that could have changed state (handlers report whether they did anything). (Done: `dispatch_event` returns whether the event may have changed state and `run()` skips the redraw when it didn't — key Release, bare motion/drag/up/non-left buttons, and focus events are discarded; this also fixed a latent bug where the mouse arm reset pending input before checking the kind, so bare motion cancelled a chord in progress.)
- [x] Completion layout measures display width with `chars().count()` (`render.rs:31-35, 681-733`), so CJK/emoji candidate names misalign columns and overflow rows. Use `unicode-width` like the mode line already does. (Done: `completions_height` and `render_completions` measure candidates with `UnicodeWidthStr::width`; over-budget names and the `[Page x/y]` splice truncate via a new `truncate_to_width` helper that drops a straddling wide glyph and pads the gap so the indicator stays flush right.)
- [x] Submitting an empty path creates garbage in both path prompts: empty find-file input creates a phantom, unsaveable buffer with an empty name (`prompts.rs:97-103` → `fileops.rs:9-35`), and empty write-file input falls through to `write_buffer_to_path(PathBuf::new())` (`prompts.rs:108,242`). Add one shared "non-empty normalized path" validation both prompts go through; on empty input show a message and re-ask. (Done: both prompt arms validate via `validated_path_from_input` (blank or normalized-empty input, e.g. `.`, re-asks: the live label flags "(path required)" and the directory prefill is restored); `open_file` and `write_buffer` also bail on empty paths as defense in depth.)
- [x] `main.rs` CLI/teardown gaps: `minimacs a.txt b.txt` silently ignores every file after the first (open them all); `--help`/`--version` open as literal buffer names (print usage/version and exit); an `Err` between `enable_raw_mode()` and entering the run loop returns early without `restore_terminal()`, leaving the shell raw (the panic hook only covers panics). (Done: `parse_args` (pure, unit-tested) handles `-h`/`--help`, `-V`/`--version` (exit 0 before terminal setup), `--`, and rejects unknown `-` options and empty args (exit 2); `Editor::open_files` opens every argument and displays the first; a `RestoreGuard` Drop restores the terminal on every error path, dropped explicitly before the abort-quit `exit(1)`.)

## Refactor: split src/app.rs (after all fixes above — Codex-reviewed plan)

app.rs is ~2900 lines, 81% of which is `mod tests`. Split it following the
`editor.rs` → `src/editor/` idiom: `src/app.rs` stays as the module root
(never renamed); children under `src/app/`. Land these only AFTER the
remaining app.rs-touching fixes above (KeyEventKind, dead event source,
render storm) — per Codex plan review, the fixes take priority and the
production-code moves would churn against them. Behavior-preserving
throughout (module plumbing, explicit test imports, and `pub(super)`
promotions are expected; no logic changes).

- [x] Split commit 1: move `#[cfg(test)] mod tests` to `src/app/tests.rs` verbatim (plus its own explicit imports so it no longer borrows app.rs's `use` list via `use super::*`), and `git mv` the three insta snapshots from `src/snapshots/` to `src/app/snapshots/` in the same commit (insta's snapshot *directory* follows the source file; the *names* stay because the module path `app::tests` is unchanged — the three snapshot tests must stay at the tests-module root forever for this reason). Do not run insta --accept; stale `source:` headers are ignored metadata. (Done: `src/app.rs` now ends at `#[cfg(test)] mod tests;`; the body moved verbatim (one indent dropped) into `src/app/tests.rs` with its own explicit crossterm/ratatui/editor/event imports on top of `use super::*`; the three `.snap` files were `git mv`'d and pass from `src/app/snapshots/` without re-accepting; 600 tests before and after.)
- [x] Split commit 2: shard the tests by topic into `src/app/tests/{editing,visual,input_state,isearch,minibuffer,completions,mouse}.rs`, each starting `use super::*;`. Shared helpers (`test_app*`, `key`/`ctrl`/`alt`/`char_key`/`key_events`, `capture_screen`, `mouse_click`, `mouse_scroll_*`) stay at the tests root — `mouse_scroll_down` is used by three topics. `digit_line`/`char_under_cursor` go to visual.rs; `open_find_file_with_clear` to completions.rs. Test count must be identical before/after. (Done: seven topic files created; single-topic helpers moved with their topic — `release`/`mouse_moved` and the `ScriptedEventSource` run-loop/render-gating tests to input_state.rs, `digit_line`/`char_under_cursor` to visual.rs, `open_find_file_with_clear` to completions.rs — while the 3 snapshot tests and all multi-topic helpers stayed at the tests root; 600 tests before and after.)
- [x] Split commit 3: extract `handle_mouse` + `handle_mouse_scroll` into `src/app/mouse.rs` (same impl header incl. the `where B::Error: Send + Sync + 'static` bound; `handle_mouse` becomes `pub(super)`); prune app.rs's mouse imports. (Done: both handlers moved verbatim into an identical impl block in `src/app/mouse.rs`; `handle_mouse` is `pub(super)` for `dispatch_event`, `handle_mouse_scroll` stays private; only `MouseEvent` could be pruned from app.rs — `MouseButton`/`MouseEventKind` are still used by the dispatcher's discard match.)
- [x] Split commit 4: extract `InputState` + `handle_key`/`handle_isearch_key`/`handle_minibuffer_tab`/`handle_paste` into `src/app/input.rs` (`InputState`, `InputState::{new,reset}`, `App::{handle_key,handle_paste}` become `pub(super)`); prune app.rs imports. `dispatch_event`, `run`, `run_until_idle`, `update_viewport`, `render`, and the `App` struct stay in app.rs. Update ARCHITECTURE.md's module map in each commit that creates a file. (Done: `src/app/input.rs` holds `InputState` and the four key/paste handlers under an identical impl header; exactly the planned items became `pub(super)` and the rest stayed private; app.rs (190 lines) keeps the `App` struct, run loop, dispatcher, viewport update, and render, importing only Event/KeyEventKind/MouseButton/MouseEventKind from crossterm plus Editor, EventSource/Poll, and render; ARCHITECTURE.md's module map was updated in each of the four commits; 600 tests throughout.)
