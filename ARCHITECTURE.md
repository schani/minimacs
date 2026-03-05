# Architecture

This document describes the internal architecture of minimacs.

## Overview

minimacs is a synchronous, single-threaded terminal text editor. There is no
async runtime. The event loop polls for terminal events with a 100ms timeout,
processes them, and re-renders the UI. ratatui handles diffing internally, so
unconditional rendering is cheap.

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
```

## Module Map

```
src/
  main.rs           Terminal setup, CLI args, runs the event loop
  app.rs            App<B: Backend> -- event loop and key routing
  editor.rs         Editor -- command execution, all state mutation
  buffer.rs         Buffer -- Rope text storage, file I/O, metadata
  pane.rs           PaneTree/PaneNode/Pane -- window layout tree
  keymap.rs         Key/KeymapNode/KeymapState -- multi-key chord trie
  command.rs        Command enum -- flat enum of all editor actions
  render.rs         render() -- walks pane tree, produces ratatui widgets
  minibuffer.rs     Minibuffer/Prompt -- prompt state, tab completion functions
  history.rs        History -- undo/redo with edit grouping
  syntax.rs         SyntaxState -- tree-sitter highlighting
  event.rs          EventSource trait -- abstracts terminal vs test input
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
              app ──> keymap
              app ──> event
```

All rendering reads from `Editor` without mutating it. The `render()` function
takes `&Editor` and produces a frame.

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
    last_recenter: Option<RecenterPosition>,
}
```

`active_buffer()` / `active_pane()` return the minibuffer buffer/pane when the
minibuffer is active, otherwise the focused pane's buffer. All editing and
movement methods use these instead of `current_buffer()` / `focused_pane()`.

### Buffer

Text is stored in a `ropey::Rope`. Each buffer has an independent undo history
and optional syntax highlighting state. Buffers have no cursor -- cursor
position is per-pane.

```rust
struct Buffer {
    id: BufferId,       // usize, monotonically increasing, never reused
    text: Rope,
    path: Option<PathBuf>,
    name: String,
    modified: bool,
    read_only: bool,
    line_ending: LineEnding,
    history: History,
    syntax: Option<SyntaxState>,
}
```

### PaneTree

A recursive tree of splits and leaves. Each leaf is a `Pane` with its own
cursor, mark, scroll position, and viewport dimensions. This matches emacs
behavior where each window has independent state into a shared buffer.

```rust
enum PaneNode {
    Leaf(Pane),
    Split { direction: Direction, children: Vec<PaneNode> },
}

struct PaneTree {
    root: PaneNode,
    focus_path: Vec<usize>,  // indices from root to focused leaf
}
```

The focus path is a sequence of child indices that navigate from the root to the
currently focused pane. Operations like `focused_pane()` walk this path.
`calculate_rects(area)` recursively divides a `Rect` into per-pane rectangles.

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

## Event Loop

`App::run()` is the main loop:

1. Poll `EventSource::next_event()`.
2. Route the event:
   - `C-g` always cancels (hard-coded, bypasses keymap).
   - If incremental search is active, `handle_isearch_key()` processes the key.
   - If a minibuffer prompt is active, `handle_minibuffer_key()` processes the key.
   - Otherwise, `KeymapState::process_key()` walks the trie.
   - If the keymap returns `NotFound` and the key is a printable character with
     no modifiers, it falls through to `InsertChar`.
3. After each event: update viewport dimensions for all panes, then render.
4. Loop until `editor.should_quit`.

`Event::Paste(text)` inserts the pasted text at point as a single undo group.
`Event::Mouse` handles left-button clicks. When the minibuffer is not active,
a click determines which pane was clicked (using `calculate_rects()`), focuses
that pane, and places the cursor at the clicked position. The position
calculation accounts for line wrapping and scroll position.
Clicks below all content place the cursor at the end of the buffer.
`Event::Resize` is handled implicitly by the viewport update.

## Rendering

`render::render(frame, &editor)` is called every iteration. It:

1. Splits the terminal area into a pane region and a 1-row minibuffer.
2. Walks `pane_tree.calculate_rects()` to get per-pane rectangles.
3. For each pane:
   - Splits the pane rect into a text area and a 1-row mode line.
   - For each visible line: if syntax state exists, computes per-character
     styles from tree-sitter highlight spans; otherwise uses default style.
   - Long lines are wrapped with a `\` continuation marker in the last column.
   - Overlays region highlighting (white background between mark and point).
   - Overlays search match highlighting (yellow for current match, dark for
     others).
   - Text content is rendered as spans within a single `Paragraph` widget.
   - Renders the mode line: modified flag, buffer name, `(line,col)`,
     position percentage, and any pending key chord prefix.
   - Focused pane gets a white-background bold mode line; unfocused panes
     get dark gray background with white text.
4. Renders the minibuffer (prompt or message).
5. Sets the terminal cursor position. If the minibuffer is active, the cursor
   goes to the minibuffer input. Otherwise it is placed at the focused pane's
   point, accounting for line wrapping when computing the visual position.

## Syntax Highlighting

Language is detected from file extension at load time. Each buffer with a
recognized language gets a `SyntaxState` containing a tree-sitter
`HighlightConfiguration`.

Highlighting happens at render time on visible lines only. The `highlight()`
method takes a byte slice, runs tree-sitter-highlight, and returns a list of
`StyledSpan { start, end, style }` (byte ranges). The renderer converts these
to per-character `Style` entries in a `HashMap<(line, col), Style>` that it
consults when building `Span`s.

The color theme is a built-in light palette matching VSCode's Light+ theme, using true color (RGB) values.

## Minibuffer

The minibuffer has two states:

- **Idle**: shows timed messages ("Wrote file.txt", "Quit", errors).
- **Prompt**: active text input with a label. Prompt kinds:
  `FindFile`, `WriteFile`, `SwitchBuffer`, `GotoLine`,
  `SaveConfirm { buffer_name }`, `ISearch`.

### Minibuffer as a Real Buffer

The minibuffer uses a real `Buffer` (`minibuffer_buffer`, id=`usize::MAX`) and
`Pane` (`minibuffer_pane`, viewport_height=1) owned by `Editor`. When a prompt
is active, `active_buffer()` / `active_pane()` return the minibuffer's
buffer/pane instead of the focused pane's. This means all editing and movement
commands (word movement, kill-line, delete-forward, undo/redo, mark/cut/copy/
paste) work automatically in the minibuffer without duplicating logic.

The minibuffer buffer is never in `self.buffers` and never appears in buffer
lists. Its `History` is reset each time a prompt starts. The shared clipboard
means kill/yank in the minibuffer uses the same clipboard as the main editor.

Key routing: Enter submits the prompt, Tab triggers completion (both intercepted
before the keymap). All other keys go through the normal keymap. `InsertNewline`,
`InsertTab`, `IndentLine`, and `DedentLine` are intercepted in `execute()` when
the minibuffer is active.

Prompt nesting is prevented by two mechanisms: (1) `start_minibuffer_prompt()`
returns early if a prompt is already active; (2) `isearch_start()`,
`kill_buffer()`, and `quit()` have local guards.

Tab completion is implemented as free functions `complete_path_with_candidates()`
and `complete_buffer_with_candidates()` in `minibuffer.rs`. Each returns
`(completed_prefix, display_candidates)`. Path candidates use basenames with
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

Rendering uses `completions_height()` (shared by `render()` and
`update_viewport()`) to conditionally insert a completions area between panes
and minibuffer. The shared `completions_layout()` helper computes column width,
column count, and row count from the candidates and terminal width. Height is
the number of layout rows, capped at `(screen_height - 2) / 3`, always leaving
room for panes and minibuffer. Candidates display in a multi-column layout
(like `ls` output) with a dark gray background.

When candidates overflow a single page, pressing Tab again advances the page
(`completion_page` on `Minibuffer`). The renderer wraps the page counter via
modulo. A `[Page X/Y]` indicator appears in the bottom-right of the completions
area when multiple pages exist. `completion_page` resets to 0 on typing, paste,
`C-g`, Enter, or when the completion prefix changes.

Pasted text has newlines replaced with spaces when pasting into the minibuffer.

## Incremental Search

`C-s` / `C-r` starts an incremental search. The search state tracks:

- The query string (built up character by character).
- The search direction (forward or backward).
- The original point and scroll position (restored on `C-g`).
- The current match position.

As the user types, `isearch_update()` searches the buffer text from the
original position. `C-s`/`C-r` during search cycle to the next/previous match.
Enter accepts the position. `C-g` restores the original position.

All matches are collected by `isearch_matches()` for rendering. The current
match is highlighted in yellow; other matches in a dim color.

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

The editor is generic over `ratatui::Backend`. Production uses
`CrosstermBackend<Stdout>`, tests use ratatui's `TestBackend`. Input is
abstracted via the `EventSource` trait:

```rust
trait EventSource {
    fn next_event(&mut self) -> Option<Event>;
}
```

`TerminalEventSource` reads from the real terminal. `TestEventSource` replays a
`VecDeque<Event>`.

### Test Layers

1. **Unit tests** (per-module `#[cfg(test)] mod tests`): test pure logic with
   no terminal. Buffer operations, undo/redo, keymap lookups, pane tree
   manipulation, minibuffer prompts, syntax detection.

2. **Integration tests** (in `app::tests`): drive the full event loop through
   `App<TestBackend>` with `TestEventSource`. Verify typing, navigation, key
   chords, file I/O, paste handling.

3. **Snapshot tests**: use `insta::assert_snapshot!` to capture rendered screen
   output from `TestBackend` and compare against stored snapshots.

