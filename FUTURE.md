# Future work

Larger features deliberately deferred. Near-term fixes live in TODO.md.

## Missing emacs features

The README promises "emacs keybindings"; these are the standard facilities a
real emacs user expects that don't exist yet (the `Command` enum has no variant
for any of them):

- **M-x** — no command-by-name dispatcher at all.
- **query-replace (M-%)** — there is currently no replace command of any kind.
- **Kill ring with M-y (yank-pop)** — the clipboard is a single `String`
  (`editor.rs:43`); only `C-k` appends. A proper kill ring would also resolve
  the OS-clipboard-vs-kill priority surprise (paste currently prefers the OS
  clipboard over freshly killed text).
- **M-d** — kill word forward (only `DeleteWordBackward` exists).
- **C-x C-b** — buffer list.
- **C-t** — transpose chars.
- **M-u / M-l / M-c** — upcase/downcase/capitalize word.
- **M-q** — fill paragraph.
- **C-x u** — undo (alternative binding).
- Smart case-insensitive incremental search (search is currently
  case-sensitive only).
