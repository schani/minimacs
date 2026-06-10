# Future work

Larger features deliberately deferred. Near-term fixes live in TODO.md.

## Incremental syntax highlighting

Highlighting currently re-parses from scratch on every cache miss: any edit bumps
`edit_generation`, the miss path copies bytes `0..end_of_visible_region` out of
the rope (`render.rs:434-464`) and runs `tree-sitter-highlight` over all of it
(`syntax.rs:315-368`). There is no use of tree-sitter's incremental API
(`Tree::edit` + passing the old tree to `parse`). For a 10MB file with the
viewport near the bottom this is plausibly seconds *per keystroke*, synchronous
on the render path.

- Switch from `tree-sitter-highlight`'s one-shot `Highlighter::highlight` to
  maintaining a persistent `Tree` per buffer, applying `InputEdit`s on every
  `Buffer::insert`/`remove`, and re-running only the highlight query over the
  visible region.
- Make the cache tolerate scrolling: the current check requires
  `cached_end_byte >= end_byte` exactly from the previous highlight run
  (`syntax.rs:352-368`), so every scroll-down step is a full re-parse. Highlight
  past the viewport (e.g. to the next multiple of N bytes) so small scrolls hit
  the cache.
- Avoid the rope→`Vec<u8>` copy of the whole prefix; tree-sitter can parse from
  chunked input via a callback, which ropey's chunk iterator can feed directly.

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
