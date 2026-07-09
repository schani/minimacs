# Future work

Larger features deliberately deferred. Near-term fixes live in TODO.md.

## Incremental syntax highlighting

The compatibility gate for the planned migration uses pinned `tree-house` 0.4.0
and `tree-house-bindings` 0.3.2. All statically linked minimacs grammars compile
through tree-house's `tree-sitter-language` adapter, including Markdown inline
and fenced-Rust injections. The unsupported Glimmer-specific JavaScript
`#offset!` injection is deliberately omitted because minimacs ships no Glimmer
grammar and could not resolve that injection in the old implementation either.

Highlighting still re-parses from scratch on every cache miss: any edit bumps
`edit_generation`, the miss path copies bytes `0..end_of_visible_region` out of
the rope and constructs a fresh tree-house `Syntax`. Persistent tree-house
syntax state and `InputEdit` propagation have not landed yet. For a 10MB file
with the viewport near the bottom this is plausibly seconds *per keystroke*,
synchronous on the render path.

- Maintain persistent tree-house syntax state per buffer, apply `InputEdit`s on
  every replacement, and run its range highlighter only over the visible region.
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
