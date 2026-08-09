# Future work

Larger features deliberately deferred. Near-term fixes live in TODO.md.

## Incremental injection-query scaling

Tree-house 0.4.0 incrementally reuses tree-sitter parse trees, but
`Syntax::update()` still executes each language layer's injection query over the
whole updated tree. This is particularly visible for Rust: the upstream query
self-injects Rust into every macro token tree, and a one-byte edit in a generated
568 KB file spends about 20 ms in tree-house update even when the file contains
no macros. Removing that injection query experimentally reduces update time to
about 4 ms, but loses recursive highlighting inside macros, so minimacs keeps it
for correctness.

A complete fix belongs either upstream in tree-house (incrementally update
injection matches over changed ranges), in a maintained fork, or in a lower-level
syntax layer that owns root and injection trees directly. The background worker
now protects input latency, but does not change this throughput cost.

## Missing emacs features

The README promises "emacs keybindings"; these are the standard facilities a
real emacs user expects that don't exist yet (the `Command` enum has no variant
for any of them):

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
