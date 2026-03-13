# Vim Mode

minimacs supports an optional vim-style modal editing mode. Launch it with
`minivim`:

```sh
minivim myfile.txt
```

(You can also pass `--vim` to `minimacs` for the same effect.)

The mode line shows `[N]` (Normal) or `[I]` (Insert) to indicate the current
mode.

## Normal Mode

In normal mode, keys execute commands rather than inserting text.

### Movement

| Key | Action |
|-----|--------|
| `h` / Left | Backward character |
| `l` / Right | Forward character |
| `j` / Down | Next line |
| `k` / Up | Previous line |
| `w` | Forward word |
| `b` | Backward word |
| `0` / Home | Beginning of line |
| `$` / End | End of line |
| `gg` | Beginning of buffer |
| `G` | End of buffer |
| `Ctrl-d` / PgDn | Page down |
| `Ctrl-u` / PgUp | Page up |

### Entering Insert Mode

| Key | Action |
|-----|--------|
| `i` | Insert before cursor |
| `a` | Insert after cursor |
| `I` | Insert at beginning of line |
| `A` | Insert at end of line |
| `o` | Open line below |
| `O` | Open line above |
| `C` | Change to end of line |
| `s` | Substitute character |

### Editing (stay in Normal mode)

| Key | Action |
|-----|--------|
| `x` | Delete character under cursor |
| `dd` | Delete entire line |
| `D` | Delete to end of line |
| `J` | Join current line with next |
| `u` | Undo |
| `Ctrl-r` | Redo |

### Clipboard

| Key | Action |
|-----|--------|
| `yy` | Yank (copy) current line |
| `p` | Paste |
| `v` | Set mark (start visual selection) |
| `y` | Copy selection (when mark is set) |
| `d` | Cut selection (when mark is set) |

### Search

| Key | Action |
|-----|--------|
| `/` | Incremental search forward |
| `?` | Incremental search backward |

During search: `Ctrl-s`/`Ctrl-r` cycle matches, Enter accepts, `Ctrl-g`
cancels.

### Commands

| Key | Action |
|-----|--------|
| `:w` | Save |
| `:q` | Quit |
| `:wq` or `:x` | Save and quit |
| `:q!` | Force quit (discard changes) |
| `:e <file>` | Open file |

### Panes

| Key | Action |
|-----|--------|
| `Ctrl-w s` | Split vertically |
| `Ctrl-w v` | Split horizontally |
| `Ctrl-w w` | Cycle focus |
| `Ctrl-w q` | Close pane |
| `Ctrl-w o` | Close other panes |

### Other

| Key | Action |
|-----|--------|
| `Esc` | Cancel pending chord / clear selection |
| `Ctrl-g` | Cancel (prompts, search, etc.) |
| `Ctrl-l` | Recenter view |

## Insert Mode

In insert mode, typing inserts text as you'd expect. The following keys have
special behavior:

| Key | Action |
|-----|--------|
| `Esc` | Return to Normal mode |
| Enter | Insert newline |
| Backspace | Delete backward |
| Delete | Delete forward |
| Tab | Indent (4 spaces) |
| Shift-Tab | Dedent |
| `Ctrl-w` | Delete word backward |
| Arrow keys | Movement |
