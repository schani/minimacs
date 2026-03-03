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
  minibuffer.rs     Minibuffer/Prompt -- prompt UI with tab completion
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
    isearch: Option<ISearchState>,
}
```

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
`Event::Resize` is handled implicitly by the viewport update.

## Rendering

`render::render(frame, &editor)` is called every iteration. It:

1. Splits the terminal area into a pane region and a 1-row minibuffer.
2. Walks `pane_tree.calculate_rects()` to get per-pane rectangles.
3. For each pane:
   - Splits the pane rect into a text area and a 1-row mode line.
   - Computes gutter width from the buffer's line count.
   - For each visible line: if syntax state exists, computes per-character
     styles from tree-sitter highlight spans; otherwise uses default style.
   - Overlays region highlighting (white background between mark and point).
   - Overlays search match highlighting (yellow for current match, dark for
     others).
   - Renders lines with `Paragraph`, gutter with `Paragraph`.
   - Renders the mode line: modified flag, buffer name, `(line,col)`,
     position percentage, and any pending key chord prefix.
   - Focused pane gets bold white mode line; unfocused panes get dim gray.
4. Renders the minibuffer (prompt or message).
5. Sets the terminal cursor position to the focused pane's point.

## Syntax Highlighting

Language is detected from file extension at load time. Each buffer with a
recognized language gets a `SyntaxState` containing a tree-sitter
`HighlightConfiguration`.

Highlighting happens at render time on visible lines only. The `highlight()`
method takes a byte range, runs tree-sitter-highlight, and returns a list of
`StyledSpan { text, style }`. These are converted to per-character `Style`
entries in a `HashMap<(line, col), Style>` that the renderer consults when
building `Span`s.

The color theme is a built-in dark palette using 256-color indices.

## Minibuffer

The minibuffer has two states:

- **Idle**: shows timed messages ("Wrote file.txt", "Quit", errors).
- **Prompt**: active text input with a label. Prompt kinds:
  `FindFile`, `WriteFile`, `SwitchBuffer`, `KillBuffer`, `GotoLine`,
  `SaveConfirm`, `ISearch`.

Tab completion is implemented for file paths (reads the filesystem) and buffer
names (matches against open buffers). The minibuffer supports basic editing:
`C-f`/`C-b` movement, `C-a`/`C-e` for start/end, backspace.

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

There are 144 tests across all modules.
