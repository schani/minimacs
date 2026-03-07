# minimacs

A terminal text editor with emacs keybindings, written in Rust.

minimacs aims to be a fast, lightweight editor that feels familiar to emacs
users. It is not extensible -- it ships one good editor, not a platform.

## Features

- **Emacs keybindings** -- standard movement, editing, and chord sequences
  (`C-x C-s`, `C-x C-f`, `M-g g`, etc.)
- **Multiple buffers** -- open, switch, and kill buffers
- **Pane splits** -- vertical and horizontal splits with per-pane cursors
- **Syntax highlighting** -- tree-sitter based, supporting 12 languages
- **Incremental search** -- `C-s`/`C-r` with live match highlighting
- **Undo/redo** -- grouped edits with automatic boundary detection
- **OS clipboard** -- copy/paste integrates with the system clipboard
- **Line ending detection** -- handles both LF and CRLF files
- **Bracketed paste** -- pastes multi-line text as a single undo group

## Supported Languages

Rust, JavaScript, TypeScript, TSX, JSON, TOML, Go, HTML, Bash, YAML, Markdown,
Env (`.env` files, using bash grammar).

Language is auto-detected from file extension or filename. The detected language
is shown in the mode line.

## Installation

```sh
cargo install --git https://github.com/schani/minimacs
```

This requires the Rust toolchain and a C compiler (for tree-sitter grammars).

To install without OS clipboard support:

```sh
cargo install --git https://github.com/schani/minimacs --no-default-features
```

## Building

Requires Rust 1.70+ and a C compiler (for tree-sitter grammars).

```sh
cargo build --release
```

The binary is at `target/release/minimacs`.

To build without OS clipboard support (removes the `arboard` dependency):

```sh
cargo build --release --no-default-features
```

## Usage

```sh
# Open a file
minimacs src/main.rs

# Start with an empty scratch buffer
minimacs
```

## Keybindings

### Movement

| Key | Action |
|-----|--------|
| `C-f` / Right | Forward character |
| `C-b` / Left | Backward character |
| `M-f` | Forward word |
| `M-b` | Backward word |
| `C-n` / Down | Next line |
| `C-p` / Up | Previous line |
| `C-a` / Home | Beginning of line |
| `C-e` / End | End of line |
| `C-v` / PgDn | Page down |
| `M-v` / PgUp | Page up |
| `M-<` | Beginning of buffer |
| `M->` | End of buffer |
| `M-g g` | Go to line number |

### Editing

| Key | Action |
|-----|--------|
| Printable chars | Self-insert |
| Enter | Insert newline |
| Tab | Insert 4 spaces |
| Backspace | Delete backward |
| `C-d` / Delete | Delete forward |
| `C-k` | Kill line (cut to end of line) |
| `C-/` or `C-_` | Undo |

### Mark and Region

| Key | Action |
|-----|--------|
| `C-Space` | Set mark |
| `C-w` | Cut region |
| `M-w` | Copy region |
| `C-y` | Paste |
| `C-x C-x` | Swap point and mark |

### Files and Buffers

| Key | Action |
|-----|--------|
| `C-x C-f` | Find file (open) |
| `C-x C-s` | Save |
| `C-x C-w` | Write file (save as) |
| `C-x b` | Switch buffer |
| `C-x k` | Kill buffer |

### Panes

| Key | Action |
|-----|--------|
| `C-x 2` | Split vertically |
| `C-x 3` | Split horizontally |
| `C-x 0` | Delete current pane |
| `C-x 1` | Delete other panes |
| `C-x o` | Cycle focus to next pane |

### Search

| Key | Action |
|-----|--------|
| `C-s` | Incremental search forward |
| `C-r` | Incremental search backward |

During incremental search: `C-s`/`C-r` cycle matches, Enter accepts, `C-g`
cancels and restores the original position.

### Other

| Key | Action |
|-----|--------|
| `C-g` | Cancel (clears pending keys, deactivates mark, cancels prompts) |
| `C-x C-c` | Quit (prompts to save modified buffers) |

## Testing

```sh
cargo test
```

The test suite includes unit tests for every module, integration tests that
drive the full editor through a `TestBackend`, and snapshot tests for rendered
output.

## License

TBD
