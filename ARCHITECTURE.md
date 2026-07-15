# Architecture

This document describes the internal architecture of minimacs.

## Overview

minimacs has a synchronous main/UI thread and one lazily started background
syntax thread. There is no async runtime. The event loop polls for terminal
events with a 100ms timeout;
when an event arrives it is processed and the UI re-renders. Poll timeouts
and events that cannot have changed state (bare mouse motion, key releases,
focus changes) skip the render entirely — an idle minimacs does no drawing
work, even with the mouse waving over it. A completed background highlight is
the other redraw trigger, so syntax appears without requiring more input.
Rendered cells and syntax-style
lookups are bounded by the viewport rather than the length of a wrapped line.
Printable ASCII lines also use direct Rope indexing for cursor and viewport
jumps; an eight-entry, edit-generation-keyed per-buffer cache remembers that
line classification across the several geometry queries in one interaction.
Unicode, tabs, and control characters use a memory-bounded streaming fallback
whose time is linear in the prefix being skipped. ratatui diffs the terminal
output.

```
Terminal input (crossterm)
       |
       v
   Event loop (app.rs)
       |
       v
   Key routing ──> Keymap trie (keymap.rs)
       |                 |
       v                 v
   Editor state     Command enum
   mutation         (command.rs)
   (editor.rs)
       |
       v
   Render (render.rs) ──> ratatui ──> Terminal output
       |
       +──> coalescing syntax jobs ──> one syntax worker thread
```

## Module Map

```
src/
  main.rs           CLI parsing (parse_args), terminal setup/teardown (Drop
                    guard + panic hook), runs the event loop
  app.rs            App<B: Backend> -- event loop, dispatch_event, viewport update
  app/input.rs        InputState (chord + pending ESC) and key routing:
                      handle_key, isearch keys, minibuffer Tab, paste
  app/mouse.rs        Mouse click-to-point mapping and wheel scrolling
  app/tests.rs        Integration-test harness (test_app, event/screen helpers)
                      and the screen snapshot tests
  app/tests/*.rs      Integration tests by topic: editing, visual, input_state,
                      isearch, minibuffer, completions, mouse
  editor.rs         Editor -- struct, apply_edit, movement/editing, dispatch
  editor/isearch.rs   Incremental search state and commands
  editor/prompts.rs   Prompt starters, submit_prompt, confirm/quit flows
  editor/fileops.rs   open_file, the write_buffer save choke point, kill-buffer,
                      buffer-name uniquification
  editor/tests/       Editor unit tests, split by command and subsystem
  buffer.rs         Buffer -- Rope text storage, file I/O, metadata
  pane.rs           PaneTree/PaneNode/Pane -- window layout tree
  keymap.rs         Key/KeymapNode/KeymapState -- multi-key chord trie
  command.rs        Command enum -- flat enum of all editor actions
  render.rs         Rendering facade
  render/layout.rs  Screen, completions, and minibuffer layout
  render/visual_line.rs
                    Shared wrapped-line display geometry
  render/widgets.rs render() orchestration and ratatui widgets
  minibuffer.rs     Minibuffer/Prompt -- prompt state, tab completion functions
  history.rs        History -- undo/redo with edit grouping
  indent.rs         Shared indentation constants (INDENT_WIDTH = 4)
  syntax.rs         Syntax highlighting facade and StyledSpan
  syntax/languages.rs
                    Language detection and tree-house configuration
  syntax/state.rs   SyntaxState parser, cache, and background state
  syntax/theme.rs   Highlight capture names and VSCode Light+ styles
  syntax_worker.rs  Single-thread parser executor and coalescing mailbox
  syntax_bench.rs   syntax-bench performance harness (see below)
  syntax_fuzz.rs    syntax-fuzz incremental-vs-fresh fuzz harness (see below)
  bin/*.rs          Thin wrappers exposing the harnesses as cargo binaries
  event.rs          EventSource trait + Poll -- abstracts terminal vs test input
```

### Dependency Graph

```
main ──> app ──> editor ──> buffer
                    |          |
                    |          +──> history
                    |          +──> syntax
                    |
                    +──> pane
                    +──> minibuffer
                    +──> command
              app ──> render ──> editor (read-only)
               |         |
               +─────────+──> syntax_worker ──> syntax
              app ──> keymap
              app ──> event
```

All rendering reads from `Editor` without mutating it. The `render()` function
takes `&Editor` plus the syntax worker handle and produces a frame.

## Key Data Structures

### Editor

Central state container. Owns all buffers, the pane tree, the clipboard, the
minibuffer, and the incremental search state. All state mutation goes through
`Editor::execute(Command)`, which dispatches to private methods.

```rust
struct Editor {
    buffers: Vec<Buffer>,
    next_buffer_id: usize,
    pane_tree: PaneTree,
    clipboard: String,
    cwd: PathBuf,
    should_quit: bool,
    pending_keys: String,
    minibuffer: Minibuffer,
    minibuffer_buffer: Buffer,  // id=usize::MAX, not in buffers vec
    minibuffer_pane: Pane,      // viewport_height=1
    isearch: Option<ISearchState>,
    last_command: Option<Command>,
    last_recenter_position: Option<RecenterPosition>,
}
```

`active_buffer()` / `active_pane()` return the minibuffer buffer/pane when the
minibuffer is active, otherwise the focused pane's buffer. All editing and
movement methods use these instead of `current_buffer()` / `focused_pane()`.

### The Edit Primitive

All buffer mutation goes through `Editor::apply_edit(start, end, text, record)`,
which replaces the chars in `[start, end)` of the active buffer with `text` and
returns the deleted text. All positions and lengths are **char indices** — byte
lengths (`String::len()`) must never be mixed in. `apply_edit`:

1. Clamps the range to the buffer bounds.
2. Records undo history according to `EditRecord`
   (`Insert`/`Delete`/`Replace`/`NoHistory` — the latter used when replaying
   undo/redo groups).
3. Calls `Buffer::replace`, which performs deletion and insertion as one atomic
   rope edit, advances `edit_generation` exactly once, and computes the
   tree-sitter `InputEdit` in UTF-8 byte offsets/byte columns for the syntax
   layer. The buffer records that edit with its generation for the background
   worker. The synchronous parser path used by the fuzz and benchmark harnesses
   receives the same edit.
4. Calls `Pane::adjust_for_edit` on every pane viewing the buffer (or on the
   minibuffer pane for the minibuffer buffer), keeping point, mark, scroll
   position, and saved per-buffer view states valid. Positions at or before
   the edit stay put (emacs marker semantics), positions inside a removed
   span are kept within the new text, and positions after it shift by the
   length delta. `scroll_top` is adjusted the same way in line units (an
   `EditDelta` carries the edit's char span plus its line-level effect), so
   a pane keeps showing the same content when another pane edits above its
   viewport. The pane's sub-line `scroll_row_offset` is left untouched (the
   pane can't see line lengths); consumers clamp it before use (see the
   PaneTree section).

Commands then set the active pane's point explicitly (in char units) when they
want something other than marker semantics, e.g. point after inserted text.
Undo/redo replay each recorded edit through `apply_edit` with `NoHistory`, so
other panes' points are adjusted there too.

`C-f`/`C-b` and Backspace/`C-d` move and delete by **grapheme cluster**
(`unicode-segmentation`): combining sequences and emoji ZWJ sequences are one
step (`next_grapheme_boundary` / `prev_grapheme_boundary`), and line endings
are a single `\n`. These queries use `GraphemeCursor` directly over Rope
chunks, requesting adjacent chunks and Unicode pre-context only when needed;
they never flatten or scan the complete buffer line. Positions computed by
column arithmetic — line movement's column clamping (`C-n`/`C-p`, page
up/down) and mouse-click mapping — are snapped to the nearest cluster boundary
at or before the raw column (`Buffer::snap_to_grapheme_boundary`), so point
never rests mid-cluster, where a Backspace would orphan a combining mark and
typing would split the cluster. Only the landing point is snapped; the
remembered goal column (`preferred_column`) keeps the unsnapped value, so moving
through a cluster-bearing line and back restores the original column.
Word movement and backward-word deletion likewise walk whole grapheme
clusters. A cluster is word text when any scalar in it is alphanumeric or `_`,
so a decomposed letter plus combining marks stays part of the surrounding word
and every command leaves point on a cluster boundary.

### Buffer

Text is stored in a `ropey::Rope`. Each buffer has an independent undo history
and optional syntax highlighting state. Buffers have no cursor -- cursor
position is per-pane.

**LF-only line endings (decision, 2026-07-10).** Only LF is supported as a
line break; the rope is invariantly LF-only. CRLF is a file *encoding*:
`Buffer::from_file` detects it and converts to LF, `save_as` converts back
when `LineEnding` is CrLf, and `RET`/paste always insert `\n` — so a CRLF
file round-trips byte-identical while every in-memory position, line count,
and movement sees only `\n`. Ropey is pinned to
`default-features = false` (dropping its `unicode_lines` feature, as Helix
does): lone CR, VT, FF, NEL, LS, and PS are ordinary content — `C-e` moves
past them, `C-k` kills through them, and they render as width-bearing chars
(a raw control char reaching the terminal cell is a known, accepted
limitation). This also makes ropey line rows exactly equal tree-sitter
Point rows for arbitrary content (see the syntax section). Accepted
tradeoffs: mixed-ending files are normalized to one uniform ending on
save, and mac-classic lone-CR files are not split into lines.

The single line-break authority is `buffer::line_break_len_chars(line)` —
1 for a terminating `\n`, 0 on the final line. `Buffer::line_len_chars`,
the renderer's line extraction (display, wrapping, mouse mapping, syntax
styling), and kill-line's at-EOL break removal all use it.

Saving is atomic: `Buffer::save()` streams the rope's chunks directly to a
temp file in the target's directory (transcoding LF to CRLF chunk-by-chunk
when needed), copies the target's permissions, fsyncs, and renames over the
target. On Unix, the containing directory is then fsynced so the new directory
entry is crash-durable as well as atomic. No contiguous whole-buffer `String`
is allocated, and a crash or full disk mid-write cannot destroy the existing
file.
The bytes land on the *physical* file behind the buffer's *logical* path:
`save_as` resolves symlink chains at write time (`resolve_write_target`,
including dangling links — writing through `foo -> missing` creates
`missing`, like emacs), so the rename rewrites the link's target instead of
replacing the link with a regular file, and the buffer keeps the logical
path as its identity (`C-x C-w` through a symlink does not silently rename
the buffer; the fingerprint covers the bytes written to the physical file).
One exception to the
rename: when the resolved target has other hard links (nlink > 1), a rename
would replace the inode and make the other names diverge, so the save falls
back to an in-place truncate-write + fsync. That keeps the inode shared but
trades away crash-atomicity for exactly that case (a crash mid-write can
leave the file truncated) — the same tradeoff emacs makes with
backup-by-copying. Each buffer remembers the modification time and a SHA-256
fingerprint of the exact on-disk bytes from the last successful load/save
boundary (including CRLF encoding); save streams update the digest while
writing, without flattening the rope. Every flow that writes a
buffer to its own path (`C-x C-s`, `C-x C-w` to the buffer's path, and
quit-time saves) goes through `Editor::external_modification_guard`, which
reads the current file (following symlinks) and asks "changed on disk; save
anyway? (y/n)" if its mtime or bytes differ, it was created/deleted, or it
cannot be read safely. This detects timestamp-preserving in-place rewrites too.
Answering "y" is the one bypass: the `SaveAnywayConfirm` handler writes via
`write_buffer` directly.
The guard does not apply to `C-x C-w` to a *different* existing path — that is
covered by `OverwriteConfirm` instead (the fingerprint baseline covers the buffer's
own file), so no flow double-prompts.

Every flow that writes a buffer to disk — `C-x C-s`, `C-x C-w`, and the
save-anyway / overwrite / quit-save confirmation handlers — goes through one
choke point, `Editor::write_buffer(buffer_id, WriteTarget)` in
`editor/fileops.rs`, so cross-cutting save concerns live in exactly one place.
`WriteTarget` separates the buffer's logical path from the physical write
target: `BufferPath` saves to the buffer's own path; `Path(p)` (the `C-x C-w`
flow) writes to an explicit path, and only after the write succeeds does the
buffer adopt it as its identity (path, uniquified name, re-detected syntax).
The `write_buffer_reporting` wrapper adds the standard "Wrote {name}" /
"Error saving: {e}" minibuffer messages; the quit flow calls `write_buffer`
directly because it continues quitting on success and aborts the quit with its
own message on failure.

Buffer names are the file basename, uniquified emacs-style on collision by
appending trailing path components (`mod.rs<lib>`), falling back to a numeric
suffix (`mod.rs<2>`). `Editor::unique_buffer_name` enforces this when opening
files and when renaming via `C-x C-w`, so name-based lookup (`C-x b`) is
unambiguous.

```rust
struct Buffer {
    id: BufferId,       // usize, monotonically increasing, never reused
    text: Rope,
    path: Option<PathBuf>,
    name: String,
    modified: bool,
    line_ending: LineEnding,
    history: History,
    syntax: Option<SyntaxState>,
}
```

### PaneTree

A recursive tree of splits and leaves. Each leaf is a `Pane` with its own
cursor, mark, scroll position, viewport dimensions, and remembered per-buffer
view state. This matches emacs behavior where each window has independent state
into a shared buffer.

```rust
struct Pane {
    buffer_id: BufferId,
    point: usize,
    mark: Option<usize>,
    preferred_column: Option<usize>,
    scroll_top: usize,        // first (partially) visible buffer line
    scroll_row_offset: usize, // visual rows of that line scrolled off above
    viewport_height: usize,
    viewport_width: usize,
    last_buffer_id: Option<BufferId>,
    buffer_states: HashMap<BufferId, BufferViewState>,
}

enum PaneNode {
    Leaf(Pane),
    Split { direction: Direction, children: Vec<PaneNode> },
}

struct PaneTree {
    root: PaneNode,
    focus_path: Vec<usize>,  // indices from root to focused leaf
}
```

The scroll position is sub-line granular: `scroll_top` is the first buffer
line with any visible content, and `scroll_row_offset` is how many visual rows
of that line (when it wraps) are scrolled off above the viewport. The offset
is only ever nonzero for a wrapping top line, which is what lets the cursor's
visual row be brought into view even inside a single line taller than the
viewport (e.g. `M->` in a one-line minified file). `compute_scroll_position`
in pane.rs computes the `(scroll_top, scroll_row_offset)` pair that makes the
cursor's visual row visible; `scroll_down_visual_rows` /
`scroll_up_visual_rows` move the pair by whole visual rows (mouse wheel).
Everything that doesn't care about wrapping (edit adjustment, view-state
save/restore) treats `scroll_top` as a plain line index. An edit or resize
can change how the top line wraps, leaving the stored offset stale;
`Pane::adjust_for_edit` cannot see line lengths, so every consumer (renderer,
cursor placement, mouse mapping, scroll computation) clamps the offset to the
top line's current visual height before use.

When a pane switches away from a buffer, it saves that buffer's point, mark,
preferred column, and scroll position (including the sub-line row offset)
into `buffer_states`. Switching back to a buffer in the same pane restores
that saved view state. `last_buffer_id` tracks the alternate buffer for that
pane, so `C-x b RET` toggles to the most recently visited buffer in that
window.

The focus path is a sequence of child indices that navigate from the root to the
currently focused pane. Operations like `focused_pane()` walk this path.
`calculate_rects(area)` recursively divides a `Rect` into per-pane rectangles
and separator rects. Horizontal splits reserve 1-column gaps between children
for vertical separator bars (│).

### Keymap

A trie (prefix tree) where each node maps `Key -> KeymapNode`. Leaf nodes hold
a `Command`. This naturally supports multi-key chords like `C-x C-s`.

```rust
struct Key { code: KeyCode, ctrl: bool, alt: bool }

struct KeymapNode {
    children: HashMap<Key, KeymapNode>,
    command: Option<Command>,
}
```

`KeymapState` wraps the trie root and accumulates pending keys. Each call to
`process_key()` walks the trie from the root using all pending keys and returns
`Matched(Command)`, `Pending`, or `NotFound`.

`Key` has no shift field: `Key::from_event` resolves a SHIFT modifier on a
char key into the shifted character (`shifted_char`: Unicode uppercase for
letters, a US-layout table for punctuation). Bindings name the shifted
character (`M-<`, `C-_`), but kitty-protocol terminals without the "report
alternate keys" flag deliver shifted keys as the *base* key plus SHIFT
(Alt+Shift+`,` for M-<) — both forms must reach the same binding. `main()`
also pushes `REPORT_ALTERNATE_KEYS`, so terminals that support it report the
layout-correct shifted char and the US-layout table is only the fallback.

### Command

A flat enum with no data except `InsertChar(char)`. All editor actions are
variants. There are no trait objects or dynamic dispatch -- just a single
`match` in `Editor::execute()`.

### History

Linear undo/redo stacks of `EditGroup`s. Each `Edit` records a position,
deleted text, and inserted text. Edits are grouped by kind:

- Consecutive character inserts at adjacent positions form a group.
- A space or newline commits the current group.
- Deletes and other operations each get their own group.
- Redo stack is cleared on any new edit.

History tracks a monotonic `version` counter (incremented on each commit) and
a `clean_version` recording the version at last save/load. `is_clean()` returns
true when no uncommitted edits exist and the current version matches the clean
version. When the redo stack is cleared and the clean version was on the
discarded branch, `clean_version` is set to `None` (unreachable).

## Terminal Lifecycle

CLI arguments are handled before any terminal setup by the pure
`parse_args()` (unit-tested in main.rs): `-h`/`--help` and `-V`/`--version`
print to a normal screen and exit 0; unrecognized leading-dash arguments and
empty arguments print an error pointing at `--help` and exit 2; `--` ends
option parsing so files literally named `--help` stay reachable, and a lone
`-` is a file name. Every file argument is opened, in order, via
`Editor::open_files` (fileops.rs): like `emacs a b`, all files become
buffers, the *first* successfully opened one is displayed, and the rest are
reachable via `C-x b`; a path that fails to open is reported and skipped
(the error message is never papered over by the "Opened N files" summary).

`main()` installs a panic hook before entering raw mode, and holds a
`RestoreGuard` whose `Drop` calls `restore_terminal()`, so every error path
between `enable_raw_mode()` and the end of `main` (the `?` early returns
from terminal setup and from the run loop) restores the terminal before the
error prints. The normal path drops the guard explicitly before the
abort-quit `std::process::exit(1)` (which skips destructors). Both the guard
and the panic hook call `restore_terminal()`, which best-effort disables raw
mode, pops keyboard enhancement flags, leaves the alternate screen, disables
bracketed paste and mouse capture, and shows the cursor. Every step runs
even if earlier ones fail, the whole function is idempotent (guard + panic
hook may both fire), and the panic hook chains to the previously installed
hook so the panic message prints on the normal screen.

## Event Loop

`App::run()` is the main loop:

1. Poll `EventSource::next_event()`, which returns a three-way `Poll`:
   `Event` (input arrived), `Timeout` (idle — skip the re-render and poll
   again), or `Closed` (the source is dead; see below).
2. Pass the event to `dispatch_event()`, the single dispatcher that routes
   every event kind (key, paste, mouse, resize) to its handler. The
   dispatcher is the one place that decides, per event kind, what happens to
   the pending input state (see below), and returns whether the event may
   have changed visible state.
3. Only if it may have: update viewport dimensions for all panes, then
   render. Per event kind: key Press/Repeat, paste, left-button mouse down,
   mouse scroll, and resize render (key presses conservatively so — whether
   a command actually changed anything is the editor's business); key
   Release, bare mouse motion, drags, button releases, non-left buttons,
   and focus gain/loss are discarded without a render. This matters because
   `EnableMouseCapture` turns on any-motion tracking (mode 1003): bare
   mouse movement floods `Moved` events, each of which would otherwise be a
   full redraw (crossterm 0.29 offers no button-motion-only capture mode).
4. Loop until `editor.should_quit`.

`Poll::Closed` means no further input can ever arrive (e.g. a tty hangup),
so `run()` exits with an "event source closed" error instead of spinning on
the dead source. Unsaved buffers cannot be prompted about — there is no
input to answer with — so the editor just quits; `main` still runs
`restore_terminal()` on this error path before propagating the error.

### Input State

All partially-consumed input lives in one struct, `InputState`, owned by
`App`:

- `keymap`: the `KeymapState` chord-in-progress walk of the keymap trie
  (`C-x ...`).
- `esc_pending`: a bare ESC was seen; the next key gets the ALT modifier
  (ESC-as-Meta).

`Editor::pending_keys` — the mode-line display of the pending input — is a
mirror of this state (it lives on `Editor` because rendering only sees
`&Editor`). Every mutation goes through `InputState` methods so the mirror
stays in sync, and `InputState::reset()` is the single point that clears
everything at once (a chord in progress, a pending ESC, and the mode-line
indicator).

### Key routing (`handle_key`)

- The dispatcher drops `KeyEventKind::Release` events before they reach
  `handle_key` (Windows and kitty-protocol terminals report them, which
  would execute every keystroke twice); Press and Repeat are handled.
- `C-g` always cancels (hard-coded, bypasses keymap): resets all pending
  input state, then runs `Cancel`.
- A bare ESC calls `set_esc_pending()`; the next key is re-tagged with ALT.
- If incremental search is active, `handle_isearch_key()` processes the key.
- If a minibuffer prompt is active, Enter (submit) and Tab (complete) are
  intercepted inline in `handle_key()`; all other keys fall through to the
  keymap.
- Otherwise, `InputState::process_key()` walks the trie, keeping the
  mode-line prefix display in sync with the result.
- If the keymap returns `NotFound` and the key is a printable character with
  no modifiers, it falls through to `InsertChar` — unless the key ended a
  pending chord (e.g. `C-x j`), in which case a "C-x j is undefined"
  message is shown instead of self-inserting.

### Other event kinds

Paste and *acted-on* mouse events — left-button down and scroll — cancel any
pending input first (cancel-then-handle): the dispatcher calls
`InputState::reset()` before handling them, so a paste or click mid-chord
cancels the chord (and any pending ESC) and then performs the paste/click
normally. This only touches the chord/ESC state — isearch is not pending
input and is unaffected. Discarded mouse events (bare motion, drags, button
releases, non-left buttons) touch nothing: under any-motion tracking, merely
moving the mouse over the terminal must not cancel a chord in progress.

`Event::Paste(text)` inserts the pasted text at point as a single undo group.
When incremental search is active, the dispatcher instead routes the paste to
`Editor::isearch_yank` (see the Incremental Search section).
`Event::Mouse` handles left-button clicks. When the minibuffer is not active,
a click determines which pane was clicked (using `calculate_rects()`), focuses
that pane, and places the cursor at the clicked position. The position
calculation accounts for line wrapping, scroll position — including the
pane's `scroll_row_offset`, added to the clicked screen row so wrap segments
of a partially scrolled-off top line map correctly — and display-only tab
expansion while still mapping back to buffer character indices.
Clicks below all content place the cursor at the end of the buffer.
Mouse scroll events (`ScrollUp`/`ScrollDown`) scroll the pane under the mouse
cursor by 3 **visual rows** (`scroll_down_visual_rows` /
`scroll_up_visual_rows` in pane.rs) without changing which pane is focused,
so the wheel moves smoothly through wrapped lines and can scroll within a
single line taller than the viewport.
`Event::Resize` is handled implicitly by the viewport update; it deliberately
does not cancel a chord in progress.

## Rendering

`render::render(frame, &editor)` is called when visible state changes. It:

1. Calls the shared `screen_layout()` calculation used by both rendering and
   `App::update_viewport()`. It divides the terminal into the pane region, an
   optional completion list, and a dynamically sized minibuffer.
2. Walks `pane_tree.calculate_rects()` to get per-pane rectangles.
3. For each pane:
   - Splits the pane rect into a text area and a 1-row mode line.
   - `VisualLineLayout` is the shared authority for wrapping, cursor geometry,
     mouse reverse-mapping, and visible-row extraction. For printable ASCII it
     computes deep wrapped positions directly from Rope character offsets. For
     Unicode, tabs, and control characters it streams from the line start but
     retains only one visual row at a time. `Buffer::line_is_printable_ascii`
     caches the eight most recently queried line classifications, keyed by the
     buffer edit generation; stale results cannot match after an edit and age
     out without explicit invalidation.
   - Only visible rows are expanded into `VisualCell`s, one per terminal column:
     literal tabs expand to spaces ending at the next `INDENT_WIDTH` tab stop;
     double-width chars (CJK, emoji) contribute their cell plus an empty
     continuation cell; combining marks are appended to the preceding cell's
     text (width 0). The buffer still stores each char individually, and
     editing/movement indexes are not changed by this rendering expansion.
     Width comes from `unicode-width` (`char_width()`), used consistently by
     wrapping, cursor placement, mouse mapping, and scrolling. A double-width
     character is kept atomic and moved to the next row if it would straddle a
     wrap boundary.
   - Long lines are wrapped with a `\` continuation marker in the last column,
     using the expanded visual width. The first `scroll_row_offset` wrap
     segments of the top (`scroll_top`) line are skipped — they are scrolled
     off above the viewport.
   - Overlays region highlighting (light blue background between mark and
     point).
   - Overlays search match highlighting (olive background for the current
     match, light orange for others).
   - Text content is rendered as spans within a single `Paragraph` widget.
   - Renders the mode line: modified flag, buffer name, `(line,col)`,
     position percentage, language name (if syntax is active), and any
     pending key chord prefix.
   - Focused pane gets a blue-background bold mode line with white text;
     unfocused panes get a light gray background with dark text.
4. Renders the minibuffer's hard-wrapped prompt or message rows.
5. Sets the terminal cursor position. If the minibuffer is active, the cursor
   goes to the minibuffer input's wrapped row and display column. Otherwise it
   is placed at the focused pane's point, accounting for line wrapping and
   display-only tab expansion when computing the visual position, and
   subtracting the pane's
   `scroll_row_offset` (a cursor row among the scrolled-off wrap segments of
   the top line is above the viewport and left unset). When point is at EOL
   of a line (or of a final wrap segment) that exactly fills the pane width,
   the column would compute to one past the last cell; the cursor instead
   wraps to column 0 of the next visual row (emacs behavior) — on screen the
   next buffer line's first row, or a blank row past the end of the buffer.
   `render::visual_row_col_in_line` computes this wrapped position and is
   shared by cursor placement and scroll computation, so the extra row is
   also counted when scrolling the cursor into view (clicking that row maps
   to what it displays: the next line's start, or end-of-buffer — that same
   EOL — below the last line). After every command,
   `Editor::ensure_cursor_visible` recomputes `(scroll_top,
   scroll_row_offset)` via `compute_scroll_position` so the cursor's visual
   row is always on screen — even inside one line that wraps taller than the
   viewport. Recentering (`C-l`) is line-granular: it resets the offset to 0
   and lets `ensure_cursor_visible` re-apply one if needed.

## Syntax Highlighting

Language is detected from file extension or filename at load time (e.g. `.env`
files are matched by filename). Each `Language` variant has a `name()` method
returning a human-readable string displayed in the mode line. Each buffer with a
recognized language gets a `SyntaxState` backed by the tree-house highlighter.
`TreeHouseLoader` compiles the queries for all statically linked grammar crates
once and maps injection names to those configurations; no dynamic grammar
libraries are required.

The app owns one lazily started `SyntaxWorker`. That thread exclusively owns a
persistent tree-house `Syntax` per highlighted buffer; parser state is never
shared behind a UI-thread mutex. Its root and injection layers parse the entire
Rope, preserving context-dependent constructs such as Markdown fenced code
blocks. The initial parse is lazy. Every subsequent `Buffer::replace()` records
one atomic, generation-tagged `InputEdit`, so the worker can feed ordered edit
batches to `Syntax::update()` and reuse unchanged subtrees. The edit's
`Point` rows are exact for arbitrary content: with ropey pinned to
`default-features = false`, its line index counts exactly the `\n` chars
tree-sitter counts. Parsing reads
the Rope directly through tree-house's `RopeSlice` input instead of copying a
buffer prefix into a temporary byte vector. Submitting a Rope snapshot is an
O(1) clone of its shared chunks.

The worker has a bounded coalescing mailbox: at most one pending snapshot and
one completed result exist per syntax-state key. Newer work replaces older
pending work for that key. If the worker already reached an intermediate
generation from a coalesced batch, it applies only the remaining suffix rather
than rebuilding. Jobs and completions carry the syntax-state key and edit
generation; the UI rejects stale results. This gives rapid typing backpressure
without cancellation machinery or additional parser threads.

A failed or timed-out incremental update drops the tree and falls back to a
fresh parse. A failed generation is attempted only once. Three consecutive
failed generations permanently disable that syntax state and show one
minibuffer message; save-as language redetection creates a new state and
re-enables highlighting. The parser timeout remains two seconds: it now bounds
worker occupation rather than a UI frame, and retaining it avoids rejecting
valid very large parses.

At render time, a cache miss submits the current Rope snapshot, edit batch, and
visible byte range. While that job runs, the renderer keeps any overlapping
exact or provisional cached spans instead of flashing the whole viewport back
to unstyled text. The worker runs tree-house's range highlighter for an 8 KiB-
padded window and publishes that complete window as absolute
`StyledSpan { start, end, style }` ranges. On a later poll the app accepts a
current completion and redraws. The renderer clips those spans to
the byte interval covered by the materialized visible cells and walks the
sorted spans while building terminal spans. It does not construct a style
entry for every character in a long line.

**Caching**: the worker's highlight cache stores `edit_generation`, the padded
absolute byte range, and its spans. The UI retains up to eight completed
viewport windows per generation so disjoint panes viewing one buffer do not
evict each other. On replacement, cached byte ranges and unaffected spans are
rebased through the `InputEdit`; a capture intersecting the edit is split around
the changed bytes. These windows are marked provisional, displayed immediately,
and refreshed by the versioned worker result. The inserted/replaced bytes remain
unstyled until that exact result arrives. Padding keeps typical highlight
queries small while making ordinary scrolling an exact cache hit.

**Performance harness**: `cargo run --release --bin syntax-bench --` generates a
deterministic Rust buffer and applies the same fixed-width visible edit under
four strategies: Rope editing without parsing, a new full parse after every
edit, the persistent incremental tree plus viewport highlighting, and the
single background worker.
Grammar/query initialization and the incremental tree's initial parse happen
before timing, modeling edits to an already-open buffer. The harness keeps
highlight results alive to prevent optimization but computes their checksums
outside the timed region, checks final text across modes, and requires full and
incremental highlight checksums to match before reporting the speedup. Its
output also separates Rope mutation, UI dispatch, parse/update, and highlight
query time. The background checksum must match the synchronous incremental
result, and its dispatch column measures Rope snapshotting, provisional-cache
rebasing, and mailbox submission independently of worker completion latency.

**Long-line render benchmark**: the ignored
`benchmark_five_megabyte_single_line` integration test renders an exact 5 MiB
single-line JSON buffer in a 120x40 test terminal with syntax highlighting,
then measures repeated renders at the beginning and far end, cursor-left
command handling, and cursor-left plus redraw. Run it in release mode:

```sh
cargo test --release benchmark_five_megabyte_single_line -- --ignored --nocapture
```

It is a manual regression benchmark rather than a CI threshold because
wall-clock timing is machine-dependent. Measurements after the
viewport-streaming work made a separate visual-position checkpoint cache
unnecessary; the generation-keyed line-class cache is enough for the direct
printable-ASCII path used by minified JSON.

**Fuzz harness**: `cargo run --release --bin syntax-fuzz --` applies random
edits through `Buffer::replace` and after every edit compares the persistent
incremental tree's highlights against a fresh parse of the same text — over
the whole file and over a random padded viewport window (the same path the
renderer uses). Edits mix a plain alphabet with an adversarial one (CRLF and
lone-CR splits, Unicode line separators, combining characters, fence/quote/
comment tokens, multi-kilobyte pastes, whole-buffer replacement), and half the
edits target structural hotspots (fences, quotes, comment openers) that
uniform random positions rarely hit. Runs are deterministic from `--seed`.
With `--raw`, a raw tree-sitter tree fed the same `InputEdit`s (no tree-house)
attributes each divergence to tree-sitter core versus tree-house's layer
handling; it is opt-in because the raw tree can reach states tree-house never
does, where the tree-sitter-md block scanner segfaults in its C serialize
function, and it is only an annotation because tree-sitter's incremental error
recovery legitimately produces transiently different trees on ERROR-heavy
buffers (`--keep-going` shows such divergences healing within a few edits).

**Language injections**: `TreeHouseLoader` resolves injection names to any
grammar minimacs ships. Markdown uses this both for fenced languages and for the
`markdown_inline` parser (emphasis, strong, code spans, links). A custom
injection query with `injection.include-children` is used instead of the
upstream default so injected parsers receive complete fenced/inline contents.
The JavaScript query's Glimmer-only `#offset!` injection is omitted because
tree-house does not implement that editor-specific predicate and minimacs ships
no Glimmer grammar.

The color theme is a built-in light palette matching VSCode's Light+ theme,
using true color (RGB) values. Markdown-specific highlight names (`text.title`,
`text.emphasis`, `text.strong`, `text.literal`, `text.uri`, `text.reference`)
are mapped to appropriate styles (bold, italic, underline, colors).

## Minibuffer

The minibuffer has two states:

- **Idle**: shows timed messages ("Wrote file.txt", "Quit", errors).
- **Prompt**: active text input with a label. Prompt kinds:
  `FindFile`, `WriteFile`, `SwitchBuffer`, `GotoLine`, `ISearch`,
  `KillConfirm { buffer_id }`, `QuitSaveConfirm { buffer_id }`.

Confirmation prompts identify buffers by id, never by name (names are not
unique). All of them save through the `Editor::write_buffer` choke point (see
the Buffer section). `C-x C-w` to an existing file asks `OverwriteConfirm`
first; the buffer's path, name, and syntax language are only updated after the
write succeeds, so a failed save never changes buffer identity. Any save to
the buffer's own path over a file changed on disk asks `SaveAnywayConfirm`
(the external-modification guard, see the Buffer section). `C-x k` on a
modified buffer asks `KillConfirm`; "y" kills the
buffer, "n" cancels. `C-x C-c` collects the ids of all modified buffers into
`Editor::quit_pending` and asks `QuitSaveConfirm` for each in turn: "y" saves
that buffer and moves on, "n" skips it, "q" cancels the quit, "a" aborts —
quit immediately, discard all unsaved changes, and exit with status 1 (like
vim's `:cq`, so git abandons the operation when minimacs is `core.editor`).
The editor quits only after every pending buffer has been answered. A "y" on
a buffer with no file path aborts the quit with a message instead of silently
discarding it. A "y" on an externally-modified buffer chains into
`SaveAnywayConfirm { resume_quit: true }` (prompt nesting is fine here — the
quit prompt finishes before the guard prompt starts): "y" saves and resumes
the quit sequence with the next pending buffer, "n" cancels the whole quit,
consistent with how a failed quit-time save cancels it.

All confirmation prompts treat an unrecognized answer the same way: the input
is cleared and the prompt re-asks (the prompt state stays alive); only a
recognized answer finishes the prompt.

`GotoLine` accepts one-based positive line numbers. Non-numeric input and zero
both leave point unchanged and report `Invalid line number`; values past the
end continue to clamp to the last line through `Buffer::line_col_to_char`.

The path prompts (find-file, write-file) share a "non-empty normalized path"
validation on submit: input that is blank or normalizes to the empty path
(`.`, `a/..`) re-asks instead of acting — the requirement is flagged in the
live prompt label ("Find file (path required): ", the same mechanism as the
failing-isearch label, since queued messages are invisible while a prompt is
active) and the default directory prefill is restored. `open_file` and the
`write_buffer` choke point also reject empty paths outright (defense in depth
for non-prompt callers like CLI arguments).
Before opening, `Editor::open_file` anchors relative paths to the editor cwd,
lexically removes `.`/`..`, and canonicalizes the longest existing ancestor.
That gives nonexistent files a stable absolute identity too: multiple CLI or
prompt spellings of the same future file switch to one buffer instead of
creating duplicate buffers that later save over each other.

Message lifecycle: `message` is only rendered while the minibuffer is idle —
a prompt hides it. To keep a message queued during a prompt (e.g. "Mark set"
from `C-SPC`) from reappearing stale afterwards, every prompt exit clears it:
`start_prompt()` and `finish()` set `message = None`, and `cancel()` replaces
it with "Quit". Handlers that want a result message ("Wrote file.txt",
"Opened file.txt") therefore show it *after* calling `finish()`.

### Minibuffer as a Real Buffer

The minibuffer uses a real `Buffer` (`minibuffer_buffer`, id=`usize::MAX`) and
`Pane` (`minibuffer_pane`) owned by `Editor`. Its viewport starts at one row
and `App::update_viewport()` refreshes both dimensions from `screen_layout()`.
When a prompt is active, `active_buffer()` / `active_pane()` return the
minibuffer's buffer/pane instead of the focused pane's. This means all editing
and movement commands (word movement, kill-line, delete-forward, undo/redo,
mark/cut/copy/paste) work automatically in the minibuffer without duplicating
logic.

### Dynamic Minibuffer Layout

`minibuffer_layout()` is the single authority for the minibuffer's height,
rendered rows, and prompt cursor. It concatenates the live prompt label and
input (or uses the idle message), hard-wraps without trimming whitespace, and
measures terminal display columns rather than bytes or scalar values. Grapheme
clusters remain intact; CJK and emoji can occupy two columns and combining
marks occupy none. A cursor immediately after content that exactly fills a row
appears at column zero of the following row.

The minibuffer grows upward and shrinks again when content is deleted or the
terminal widens. Its height is capped at one third of the terminal. If a prompt
exceeds the cap, the visible row window follows the cursor; an idle message
shows its first rows. `screen_layout()` places any completion list immediately
above the resulting minibuffer and gives the remaining rows to the pane tree.

The minibuffer buffer is never in `self.buffers` and never appears in buffer
lists. Its `History` is reset each time a prompt starts. The shared clipboard
means kill/yank in the minibuffer uses the same clipboard as the main editor.
Kill chains don't cross prompt boundaries, though: `submit_prompt()` and
`isearch_accept()` reset `last_command` (Enter bypasses `execute()`, which
would otherwise update it), so a `C-k` in a prompt followed by a `C-k` in the
buffer replaces the kill instead of appending. C-g needs no explicit reset —
it runs `Command::Cancel` through `execute()`. A `C-k` that kills nothing
(point at end of buffer) touches neither the internal nor the OS clipboard
and doesn't start or extend a kill chain.

Key routing: Enter submits the prompt, Tab triggers completion (both intercepted
before the keymap). All other keys go through the normal keymap. `InsertNewline`,
`IndentLine`, and `DedentLine` are intercepted in `execute()` when the
minibuffer is active.

Prompt handlers may move point in the focused pane without going through
`execute()` (e.g. goto-line), so `submit_prompt()` ends by calling
`ensure_cursor_visible()` — the same scroll-into-view pass `execute()` runs
after every command.

Prompt nesting is prevented by two mechanisms: (1) `start_minibuffer_prompt()`
returns early if a prompt is already active; (2) `isearch_start()`,
`kill_buffer()`, and `quit()` have local guards.

Pane layout and focus are frozen while a prompt is active: the split, delete-
pane, and cycle-focus commands are ignored in `execute()` (and mouse clicks
are ignored in key routing), so prompts that resolve their target at submit
time — e.g. `C-x C-w` writes the focused pane's buffer — cannot be retargeted
mid-prompt.

Path input goes through `normalize_path_string()` (both prompt submission and
tab completion): a leading `~`/`~/...` expands to `$HOME`, `.` components are
dropped, and `..` components are resolved lexically — on a relative path,
leading `..` components are preserved (`a/../../b` → `../b`) rather than
silently dropped, while on an absolute path `..` clamps at `/`. At submission
time the find-file and write-file prompts resolve a relative result against
`Editor::cwd` (captured at startup) via `Editor::path_from_input()`, so which
file is opened or written never depends on the process working directory.

Tab completion is implemented as free functions `complete_path_with_candidates()`
and `complete_buffer_with_candidates()` in `minibuffer.rs`. Each returns
`(completed_prefix, display_candidates)`. Relative path input is looked up on
disk against `Editor::cwd` (the same base submission resolves against) but the
completed string stays relative. Path candidates use basenames with
trailing `/` for directories; buffer candidates are sorted alphabetically.
Completion replaces the buffer contents as a single undo group using
`record_replace()`, making it undoable.

### Completion List

When Tab is pressed and multiple matches exist, `minibuffer.completions` is set
to `Some(Vec<String>)` containing the display candidates. The completions field
is cleared:
- Before keymap processing on any non-Tab/Enter key when minibuffer is active
- On paste events
- In `cancel()`, `finish()`, and `start_prompt()` lifecycle methods

`screen_layout()` uses `completions_height()` to conditionally insert a
completions area between panes and minibuffer. The shared
`completions_layout()` helper computes column width, column count, and row
count from the candidates and terminal width. All widths are display columns
measured with unicode-width (CJK/emoji names count two columns per glyph, not
one per char); names too wide for the remaining row are truncated by
`truncate_to_width()`, which drops a straddling wide char entirely rather than
splitting a glyph. Height is the number of layout rows, capped at
`(screen_height - 2) / 3`, always leaving room for panes and minibuffer.
Candidates display in a multi-column layout (like `ls` output) with a dark
gray background.

When candidates overflow a single page, pressing Tab again advances the page
(`completion_page` on `Minibuffer`). The renderer wraps the page counter via
modulo. A `[Page X/Y]` indicator appears in the bottom-right of the completions
area when multiple pages exist. `completion_page` resets to 0 on typing, paste,
`C-g`, Enter, or when the completion prefix changes.

Pasted text is normalized (`Editor::normalized_paste`, used by both `C-y` and
bracketed paste): in the minibuffer every line-break form (`\r\n`, `\r`, `\n`)
becomes a space; in a buffer, every break form is unified to `\n` (the rope
is LF-only regardless of the buffer's save-time `LineEnding`), so pasting
CRLF text cannot smuggle in raw `\r` chars.

## Incremental Search

`C-s` / `C-r` starts an incremental search. The search state tracks:

- The query string (built up character by character).
- One contiguous snapshot of the buffer text, created when search starts.
- The search direction (forward or backward).
- The original point and scroll position (restored on `C-g`).
- The current match position.

Isearch owns input until it finishes, so the searched buffer cannot change;
it flattens the Rope into `ISearchState::text_snapshot` once at startup rather
than once per query edit. As the user types, `isearch_update()` scans that
snapshot and caches the char positions of all matches in
`ISearchState::matches`, then jumps to the first match from the original
position. `C-s`/`C-r` during search cycle to the next/previous match by walking
the cached list — no rescan. Enter accepts the position. `C-g` restores the
original position.

The prompt label is live, like emacs: it is recomputed from the search state
(`isearch_sync_label`) after every query edit, cycle, and direction flip.
Normally it reads "I-search: " / "I-search backward: "; when the last search
action found no match (`ISearchState::failing`) it becomes "Failing
I-search: " / "Failing I-search backward: ". Failure is shown only in the
label — never as a queued minibuffer message, which would be invisible behind
the prompt and reappear stale after it ends (see the Minibuffer section's
message lifecycle).

`isearch_matches()` (used by the renderer every frame) also just reads the
cache. The only O(buffer) work is the single scan per query change.

Pasting while isearch is active extends the query instead of inserting into a
buffer (emacs `isearch-yank` semantics): `Editor::isearch_yank` normalizes the
pasted text with `normalized_paste` (the query is a single line, so every line
break becomes a space, like any minibuffer paste), appends it to the query,
syncs the minibuffer display, and re-runs `isearch_update`. Backspace after a
paste removes one extended grapheme cluster at a time, same as after typed
input, so decomposed characters and emoji sequences are not split.

The current match is highlighted with an olive background; other matches in
light orange.

## Indentation

Indentation uses spaces only, with a centralized `INDENT_WIDTH` constant (4).

- **RET** (`InsertNewline`): inserts a newline followed by the current line's
  leading whitespace (spaces only; tabs are converted to spaces). Cursor lands
  after the copied indentation.
- **TAB** (`IndentLine`): prepends `INDENT_WIDTH` spaces at the start of the
  current line. If a region is active, indents all lines intersecting the
  region (excluding the last line if the region end is at column 0).
- **Shift+TAB** (`DedentLine`): removes up to `INDENT_WIDTH` leading spaces
  from the current line. Region behavior mirrors `IndentLine`.

Region indent/dedent replaces the entire affected span in a single
`record_replace()` call, making it one undo step. Point and mark are adjusted
by tracking the cumulative character delta per line.

## Testing

The package declares Rust 1.88 as its minimum supported Rust version. CI runs
`cargo check --locked --all-targets --all-features` on exactly 1.88 in addition
to the stable-toolchain test, coverage, Clippy, fuzz-smoke, and benchmark-smoke
jobs, preventing the manifest and installation documentation from drifting
below the compiler required by source or locked dependencies.

The editor is generic over `ratatui::Backend`. Production uses
`CrosstermBackend<Stdout>`, tests use ratatui's `TestBackend`. Input is
abstracted via the `EventSource` trait:

```rust
enum Poll { Event(Event), Timeout, Closed }

trait EventSource {
    fn next_event(&mut self) -> Poll;
}
```

`TerminalEventSource` reads from the real terminal: a poll timeout maps to
`Timeout`, and poll/read *errors* map to `Closed` (the terminal is gone —
mapping them to `Timeout` would busy-spin the run loop forever).
`TestEventSource` replays a `VecDeque<Event>` and reports `Closed` once the
queue is drained, which is how `run_until_idle` terminates in tests.

### Test Layers

1. **Unit tests** (per-module `#[cfg(test)] mod tests`): test pure logic with
   no terminal. Buffer operations, undo/redo, keymap lookups, pane tree
   manipulation, minibuffer prompts, syntax detection.

2. **Integration tests** (in `app::tests`): drive the full event loop through
   `App<TestBackend>` with `TestEventSource`. Verify typing, navigation, key
   chords, file I/O, paste handling. Split by topic under `src/app/tests/`
   (editing, visual, input_state, isearch, minibuffer, completions, mouse);
   shared helpers live at the tests-module root in `src/app/tests.rs`.

3. **Snapshot tests**: use `insta::assert_snapshot!` to capture rendered screen
   output from `TestBackend` and compare against stored snapshots. The stored
   snapshots live in `src/app/snapshots/` (insta derives the snapshot
   directory from the source file's directory, and the snapshot names from
   the module path `app::tests` — so these tests must stay directly in that
   module).

4. **Repository policy tests** (under `tests/`): protect source-tree invariants
   that Cargo itself cannot express. In particular, a normal Cargo build has no
   build script and therefore cannot install or overwrite Git hooks. The
   versioned `.githooks/pre-commit` check suite is strictly opt-in via the
   checkout-local `core.hooksPath` configuration.
