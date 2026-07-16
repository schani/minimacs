use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[cfg(test)]
use ratatui::style::{Color, Style};
#[cfg(test)]
use ropey::Rope;
use ropey::RopeSlice;
use tree_house::Syntax as TreeHouseSyntax;

use super::languages::*;
#[cfg(test)]
use super::theme::*;
use super::StyledSpan;

const PARSE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONSECUTIVE_FAILED_GENERATIONS: u8 = 3;
const HIGHLIGHT_CACHE_PADDING: usize = 8 * 1024;
const BACKGROUND_CACHE_WINDOWS: usize = 8;

pub(crate) struct SyntaxCompletion {
    pub(crate) key: usize,
    pub(crate) generation: usize,
    pub(crate) requested: Range<usize>,
    pub(crate) spans: Vec<StyledSpan>,
    pub(crate) disabled: bool,
}

struct HighlightCache {
    version: usize,
    range: Range<usize>,
    spans: Vec<StyledSpan>,
}

struct BackgroundHighlightCache {
    version: usize,
    range: Range<usize>,
    spans: Vec<StyledSpan>,
    exact: bool,
}

pub(crate) struct BackgroundSpans {
    pub(crate) spans: Vec<StyledSpan>,
    pub(crate) exact: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct BackgroundEdit {
    pub(crate) generation: usize,
    pub(crate) edit: tree_house::tree_sitter::InputEdit,
}

#[derive(Default)]
struct BackgroundState {
    confirmed_generation: Option<usize>,
    pending_edits: Vec<BackgroundEdit>,
    caches: VecDeque<BackgroundHighlightCache>,
    disabled_reported: bool,
}

static NEXT_BACKGROUND_KEY: AtomicUsize = AtomicUsize::new(1);

/// Holds a parsed tree and highlight config for a buffer.
pub struct SyntaxState {
    pub language: Language,
    background_key: usize,
    background: RefCell<BackgroundState>,
    syntax: RefCell<Option<TreeHouseSyntax>>,
    cache: RefCell<Option<HighlightCache>>,
    parse_timeout: Duration,
    failed_version: Cell<Option<usize>>,
    consecutive_failed_generations: Cell<u8>,
    disabled: Cell<bool>,
    #[cfg(test)]
    full_parse_count: Cell<usize>,
    #[cfg(test)]
    incremental_update_count: Cell<usize>,
    #[cfg(test)]
    parse_attempt_count: Cell<usize>,
}

impl SyntaxState {
    /// Create a new syntax state for the given language.
    pub fn new(lang: Language) -> Option<Self> {
        Self::new_with_timeout(lang, PARSE_TIMEOUT)
    }

    fn new_with_timeout(lang: Language, parse_timeout: Duration) -> Option<Self> {
        let loader = match tree_house_loader() {
            Ok(loader) => loader,
            Err(error) => {
                eprintln!("Failed to create tree-house config for {lang:?}: {error}");
                return None;
            }
        };
        loader.config_for_language(lang)?;

        Some(SyntaxState {
            language: lang,
            background_key: NEXT_BACKGROUND_KEY.fetch_add(1, Ordering::Relaxed),
            background: RefCell::new(BackgroundState::default()),
            syntax: RefCell::new(None),
            cache: RefCell::new(None),
            parse_timeout,
            failed_version: Cell::new(None),
            consecutive_failed_generations: Cell::new(0),
            disabled: Cell::new(false),
            #[cfg(test)]
            full_parse_count: Cell::new(0),
            #[cfg(test)]
            incremental_update_count: Cell::new(0),
            #[cfg(test)]
            parse_attempt_count: Cell::new(0),
        })
    }

    #[cfg(test)]
    fn with_timeout(lang: Language, parse_timeout: Duration) -> Option<Self> {
        Self::new_with_timeout(lang, parse_timeout)
    }

    /// Highlight a slice of source code bytes and return styled spans.
    /// The spans have byte offsets relative to the input `source`.
    #[cfg(test)]
    pub fn highlight(&self, source: &[u8]) -> Vec<StyledSpan> {
        let Ok(source) = std::str::from_utf8(source) else {
            return Vec::new();
        };
        let rope = Rope::from_str(source);
        self.highlight_rope(rope.slice(..), 0..rope.len_bytes(), 0)
    }

    /// Update the persistent parse tree after `source` has been changed by `edit`.
    /// If there is no tree yet, the next highlight lazily performs the initial parse.
    /// A failed incremental update also falls back to a fresh parse on the next render.
    pub(crate) fn apply_edit(
        &self,
        source: RopeSlice<'_>,
        edit: tree_house::tree_sitter::InputEdit,
    ) {
        self.apply_edits(source, std::slice::from_ref(&edit));
    }

    pub(crate) fn background_key(&self) -> usize {
        self.background_key
    }

    pub(crate) fn note_background_edit(
        &self,
        generation: usize,
        edit: tree_house::tree_sitter::InputEdit,
    ) {
        let mut background = self.background.borrow_mut();
        for cache in &mut background.caches {
            rebase_background_cache(cache, generation, &edit);
        }
        background
            .pending_edits
            .push(BackgroundEdit { generation, edit });
    }

    pub(crate) fn background_update_for(
        &self,
        generation: usize,
    ) -> (Option<usize>, Vec<BackgroundEdit>) {
        let background = self.background.borrow();
        let base = background.confirmed_generation;
        let edits = base
            .map(|base| {
                background
                    .pending_edits
                    .iter()
                    .filter(|versioned| {
                        versioned.generation > base && versioned.generation <= generation
                    })
                    .copied()
                    .collect()
            })
            .unwrap_or_default();
        (base, edits)
    }

    pub(crate) fn background_spans(
        &self,
        requested: Range<usize>,
        generation: usize,
    ) -> BackgroundSpans {
        let background = self.background.borrow();
        let exact = background.caches.iter().rev().find(|cache| {
            cache.version == generation
                && cache.exact
                && cache.range.start <= requested.start
                && cache.range.end >= requested.end
        });
        // max_by_key returns the last of equally-overlapping windows, so a
        // forward scan breaks ties toward the newest (freshest) window.
        let cache = exact.or_else(|| {
            background
                .caches
                .iter()
                .filter(|cache| cache.version == generation)
                .max_by_key(|cache| overlap_len(&cache.range, &requested))
                .filter(|cache| overlap_len(&cache.range, &requested) > 0)
        });
        let spans = cache
            .map(|cache| {
                cache
                    .spans
                    .iter()
                    .filter(|span| span.end > requested.start && span.start < requested.end)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        BackgroundSpans {
            spans,
            exact: exact.is_some(),
        }
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled.get()
    }

    pub(crate) fn take_disabled_message(&self) -> bool {
        if !self.disabled.get() {
            return false;
        }
        let mut background = self.background.borrow_mut();
        if background.disabled_reported {
            return false;
        }
        background.disabled_reported = true;
        true
    }

    pub(crate) fn accept_background_completion(
        &self,
        completion: SyntaxCompletion,
        current_generation: usize,
    ) -> bool {
        if completion.key != self.background_key || completion.generation != current_generation {
            return false;
        }
        let mut background = self.background.borrow_mut();
        self.disabled.set(completion.disabled);
        background.confirmed_generation = Some(completion.generation);
        background
            .pending_edits
            .retain(|versioned| versioned.generation > completion.generation);
        // Windows fully covered by the fresh result are superseded; keeping
        // them around would let the provisional fallback serve their stale
        // spans after the next edit. Disjoint windows (other panes) survive.
        background.caches.retain(|cache| {
            cache.version == completion.generation
                && !(completion.requested.start <= cache.range.start
                    && cache.range.end <= completion.requested.end)
        });
        if background.caches.len() == BACKGROUND_CACHE_WINDOWS {
            background.caches.pop_front();
        }
        background.caches.push_back(BackgroundHighlightCache {
            version: completion.generation,
            range: completion.requested,
            spans: completion.spans,
            exact: true,
        });
        true
    }

    /// Apply a sequence of edits to one source snapshot in a single parser
    /// update. The edits are ordered and use the coordinates in effect when
    /// each corresponding UI edit occurred.
    pub(crate) fn apply_edits(
        &self,
        source: RopeSlice<'_>,
        edits: &[tree_house::tree_sitter::InputEdit],
    ) {
        if edits.is_empty() {
            return;
        }
        *self.cache.borrow_mut() = None;
        let mut syntax = self.syntax.borrow_mut();
        let Some(parsed) = syntax.as_mut() else {
            return;
        };
        let Ok(loader) = tree_house_loader() else {
            *syntax = None;
            return;
        };
        if parsed
            .update(source, self.parse_timeout, edits, loader)
            .is_err()
        {
            *syntax = None;
        } else {
            #[cfg(test)]
            self.incremental_update_count
                .set(self.incremental_update_count.get() + 1);
        }
    }

    /// Highlight an absolute byte range in a Rope. The parse tree covers the
    /// entire buffer, while highlight queries are limited to a padded window
    /// around the requested viewport.
    pub(crate) fn highlight_rope(
        &self,
        source: RopeSlice<'_>,
        requested: Range<usize>,
        version: usize,
    ) -> Vec<StyledSpan> {
        let requested = clamp_range(requested, source.len_bytes());
        if self.disabled.get() {
            return Vec::new();
        }
        if self.failed_version.get() == Some(version) {
            return Vec::new();
        }
        if !self.cache_is_valid(version, requested.clone()) {
            if !self.ensure_syntax(source) {
                self.failed_version.set(Some(version));
                let failures = self.consecutive_failed_generations.get() + 1;
                self.consecutive_failed_generations.set(failures);
                if failures >= MAX_CONSECUTIVE_FAILED_GENERATIONS {
                    self.disabled.set(true);
                }
                return Vec::new();
            }
            self.failed_version.set(None);
            self.consecutive_failed_generations.set(0);
            let window = padded_range(source, requested.clone());
            let Ok(loader) = tree_house_loader() else {
                return Vec::new();
            };
            let syntax = self.syntax.borrow();
            let Some(parsed) = syntax.as_ref() else {
                return Vec::new();
            };
            let spans = tree_house_spans(parsed, source, loader, window.clone());
            drop(syntax);
            *self.cache.borrow_mut() = Some(HighlightCache {
                version,
                range: window,
                spans,
            });
        }

        self.cache
            .borrow()
            .as_ref()
            .map(|cache| {
                cache
                    .spans
                    .iter()
                    .filter(|span| span.end > requested.start && span.start < requested.end)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Highlight a viewport and return the complete padded cache window that
    /// was populated for it. Background consumers can publish that window to
    /// the UI without asking the highlighter to pad and query a second time.
    pub(crate) fn highlight_rope_window(
        &self,
        source: RopeSlice<'_>,
        requested: Range<usize>,
        version: usize,
    ) -> (Range<usize>, Vec<StyledSpan>) {
        let requested = clamp_range(requested, source.len_bytes());
        let _ = self.highlight_rope(source, requested.clone(), version);
        self.cache
            .borrow()
            .as_ref()
            .filter(|cache| {
                cache.version == version
                    && cache.range.start <= requested.start
                    && cache.range.end >= requested.end
            })
            .map(|cache| (cache.range.clone(), cache.spans.clone()))
            .unwrap_or((requested, Vec::new()))
    }

    /// Ensure the whole Rope has a parse tree without running highlight
    /// queries. This is exposed separately for the performance harness.
    #[allow(dead_code)]
    pub(crate) fn parse_rope(&self, source: RopeSlice<'_>) -> bool {
        self.ensure_syntax(source)
    }

    /// The raw tree-sitter grammar for `lang`, bypassing tree-house. Exposed
    /// for the fuzz harness, which uses it to attribute incremental-parse
    /// divergence to either tree-sitter core or tree-house's layer handling.
    #[allow(dead_code)]
    pub(crate) fn raw_grammar(lang: Language) -> Option<tree_house::tree_sitter::Grammar> {
        let (language_fn, _, _, _) = language_config(lang);
        tree_house::tree_sitter::Grammar::try_from(language_fn).ok()
    }

    fn ensure_syntax(&self, source: RopeSlice<'_>) -> bool {
        if self.syntax.borrow().is_some() {
            return true;
        }
        let Ok(loader) = tree_house_loader() else {
            return false;
        };
        let Some(language) = loader.id_for_language(self.language) else {
            return false;
        };
        #[cfg(test)]
        self.parse_attempt_count
            .set(self.parse_attempt_count.get() + 1);
        let Ok(syntax) = TreeHouseSyntax::new(source, language, self.parse_timeout, loader) else {
            return false;
        };
        *self.syntax.borrow_mut() = Some(syntax);
        #[cfg(test)]
        self.full_parse_count.set(self.full_parse_count.get() + 1);
        true
    }

    /// Check if the cached highlight result covers the needed range at the right version.
    fn cache_is_valid(&self, version: usize, requested: Range<usize>) -> bool {
        let cache = self.cache.borrow();
        match cache.as_ref() {
            Some(c) => {
                c.version == version
                    && c.range.start <= requested.start
                    && c.range.end >= requested.end
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn full_parse_count(&self) -> usize {
        self.full_parse_count.get()
    }

    #[cfg(test)]
    pub(crate) fn incremental_update_count(&self) -> usize {
        self.incremental_update_count.get()
    }

    #[cfg(test)]
    fn parse_attempt_count(&self) -> usize {
        self.parse_attempt_count.get()
    }
}

fn clamp_range(range: Range<usize>, len: usize) -> Range<usize> {
    let start = range.start.min(len);
    start..range.end.min(len).max(start)
}

fn overlap_len(left: &Range<usize>, right: &Range<usize>) -> usize {
    left.end
        .min(right.end)
        .saturating_sub(left.start.max(right.start))
}

fn rebase_background_cache(
    cache: &mut BackgroundHighlightCache,
    generation: usize,
    edit: &tree_house::tree_sitter::InputEdit,
) {
    let start = edit.start_byte as usize;
    let old_end = edit.old_end_byte as usize;
    let new_end = edit.new_end_byte as usize;
    cache.range = rebase_cached_range(cache.range.clone(), start, old_end, new_end);
    let old_spans = std::mem::take(&mut cache.spans);
    cache.spans.reserve(old_spans.len());
    for mut span in old_spans {
        let overlaps_edit = if start == old_end {
            span.start < start && span.end > start
        } else {
            span.start < old_end && span.end > start
        };
        if overlaps_edit {
            if span.start < start {
                // The surviving piece before the edit also covers the
                // inserted bytes: new text provisionally inherits the
                // preceding character's style until the worker's exact
                // result replaces this window.
                cache.spans.push(StyledSpan {
                    start: span.start,
                    end: new_end,
                    style: span.style,
                });
            }
            if span.end > old_end {
                cache.spans.push(StyledSpan {
                    start: new_end,
                    end: shift_after_edit(span.end, old_end, new_end),
                    style: span.style,
                });
            }
            continue;
        }
        if span.start >= old_end {
            span.start = shift_after_edit(span.start, old_end, new_end);
            span.end = shift_after_edit(span.end, old_end, new_end);
        } else if new_end > start && span.end == start && span.start < start {
            // A span ending exactly at the edit precedes the inserted
            // bytes; extend it over them for the same provisional styling.
            span.end = new_end;
        }
        cache.spans.push(span);
    }
    cache.version = generation;
    cache.exact = false;
}

fn rebase_cached_range(
    range: Range<usize>,
    start: usize,
    old_end: usize,
    new_end: usize,
) -> Range<usize> {
    if range.end <= start {
        return range;
    }
    if range.start >= old_end {
        return shift_after_edit(range.start, old_end, new_end)
            ..shift_after_edit(range.end, old_end, new_end);
    }
    let rebased_start = range.start.min(start);
    let rebased_end = if range.end <= old_end {
        new_end
    } else {
        shift_after_edit(range.end, old_end, new_end)
    };
    rebased_start..rebased_end.max(rebased_start)
}

fn shift_after_edit(position: usize, old_end: usize, new_end: usize) -> usize {
    if new_end >= old_end {
        position.saturating_add(new_end - old_end)
    } else {
        position.saturating_sub(old_end - new_end)
    }
}

fn padded_range(source: RopeSlice<'_>, requested: Range<usize>) -> Range<usize> {
    let raw_start = requested.start.saturating_sub(HIGHLIGHT_CACHE_PADDING);
    let raw_end = requested
        .end
        .saturating_add(HIGHLIGHT_CACHE_PADDING)
        .min(source.len_bytes());
    let start = source.char_to_byte(source.byte_to_char(raw_start));
    let end_char = source.byte_to_char(raw_end);
    let mut end = source.char_to_byte(end_char);
    if end < raw_end && end_char < source.len_chars() {
        end = source.char_to_byte(end_char + 1);
    }
    start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use std::path::Path;

    #[test]
    fn detect_rust() {
        assert_eq!(detect_language(Path::new("main.rs")), Some(Language::Rust));
    }

    #[test]
    fn detect_javascript() {
        assert_eq!(
            detect_language(Path::new("app.js")),
            Some(Language::JavaScript)
        );
    }

    #[test]
    fn detect_typescript() {
        assert_eq!(
            detect_language(Path::new("index.ts")),
            Some(Language::TypeScript)
        );
        assert_eq!(detect_language(Path::new("App.tsx")), Some(Language::Tsx));
    }

    #[test]
    fn detect_json() {
        assert_eq!(
            detect_language(Path::new("package.json")),
            Some(Language::Json)
        );
    }

    #[test]
    fn detect_toml() {
        assert_eq!(
            detect_language(Path::new("Cargo.toml")),
            Some(Language::Toml)
        );
    }

    #[test]
    fn detect_markdown() {
        assert_eq!(
            detect_language(Path::new("README.md")),
            Some(Language::Markdown)
        );
    }

    #[test]
    fn detect_go() {
        assert_eq!(detect_language(Path::new("main.go")), Some(Language::Go));
    }

    #[test]
    fn detect_html() {
        assert_eq!(
            detect_language(Path::new("index.html")),
            Some(Language::Html)
        );
    }

    #[test]
    fn detect_bash() {
        assert_eq!(
            detect_language(Path::new("script.sh")),
            Some(Language::Bash)
        );
    }

    #[test]
    fn detect_yaml() {
        assert_eq!(
            detect_language(Path::new("config.yml")),
            Some(Language::Yaml)
        );
        assert_eq!(
            detect_language(Path::new("config.yaml")),
            Some(Language::Yaml)
        );
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(detect_language(Path::new("file.xyz")), None);
    }

    #[test]
    fn detect_git_message_files() {
        for name in [
            "COMMIT_EDITMSG",
            "MERGE_MSG",
            "TAG_EDITMSG",
            "NOTES_EDITMSG",
            "EDIT_DESCRIPTION",
        ] {
            assert_eq!(
                detect_language(Path::new(&format!("/repo/.git/{name}"))),
                Some(Language::GitCommit),
                "{name} should be detected as a git message"
            );
        }
        assert_eq!(Language::GitCommit.name(), "Git Commit");
    }

    #[test]
    fn highlight_gitcommit_comment_lines() {
        let state = SyntaxState::new(Language::GitCommit).unwrap();
        let source =
            b"Fix the frobnicator\n\n# Please enter the commit message for your changes.\n";
        let spans = state.highlight(source);
        assert!(!spans.is_empty());
        // The `#` line must be styled as a comment.
        let comment_style = style_for_highlight(highlight_index("comment"));
        let comment_start = 21; // byte offset of '#'
        assert!(
            spans.iter().any(|s| s.start <= comment_start
                && s.end > comment_start
                && s.style == comment_style),
            "no comment span covering the # line: {spans:?}"
        );
    }

    #[test]
    fn highlight_rust_code() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = b"fn main() { let x = 42; }";
        let spans = state.highlight(source);
        // Should produce some spans
        assert!(!spans.is_empty());
        // The spans should cover the entire source
        let min_start = spans.iter().map(|s| s.start).min().unwrap();
        let max_end = spans.iter().map(|s| s.end).max().unwrap();
        assert_eq!(min_start, 0);
        assert_eq!(max_end, source.len());
    }

    #[test]
    fn highlight_json() {
        let state = SyntaxState::new(Language::Json).unwrap();
        let source = br#"{"key": "value", "num": 123}"#;
        let spans = state.highlight(source);
        assert!(!spans.is_empty());
    }

    /// Helper: look up the index of a highlight name.
    fn highlight_index(name: &str) -> usize {
        HIGHLIGHT_NAMES.iter().position(|&n| n == name).unwrap()
    }

    #[test]
    fn vscode_light_comment_color() {
        let style = style_for_highlight(highlight_index("comment"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 128, 0)));
    }

    #[test]
    fn vscode_light_string_color() {
        let style = style_for_highlight(highlight_index("string"));
        assert_eq!(style.fg, Some(Color::Rgb(163, 21, 21)));
    }

    #[test]
    fn vscode_light_keyword_color() {
        let style = style_for_highlight(highlight_index("keyword"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn vscode_light_function_color() {
        let style = style_for_highlight(highlight_index("function"));
        assert_eq!(style.fg, Some(Color::Rgb(121, 94, 38)));
    }

    #[test]
    fn vscode_light_type_color() {
        let style = style_for_highlight(highlight_index("type"));
        assert_eq!(style.fg, Some(Color::Rgb(38, 127, 153)));
    }

    #[test]
    fn vscode_light_number_color() {
        let style = style_for_highlight(highlight_index("number"));
        assert_eq!(style.fg, Some(Color::Rgb(9, 134, 88)));
    }

    #[test]
    fn vscode_light_constant_color() {
        let style = style_for_highlight(highlight_index("constant"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 112, 193)));
    }

    #[test]
    fn vscode_light_variable_builtin_color() {
        let style = style_for_highlight(highlight_index("variable.builtin"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 0, 255)));
    }

    #[test]
    fn vscode_light_variable_parameter_color() {
        let style = style_for_highlight(highlight_index("variable.parameter"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 16, 128)));
    }

    #[test]
    fn vscode_light_variable_color() {
        let style = style_for_highlight(highlight_index("variable"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 16, 128)));
    }

    #[test]
    fn vscode_light_escape_color() {
        let style = style_for_highlight(highlight_index("escape"));
        assert_eq!(style.fg, Some(Color::Rgb(238, 0, 0)));
    }

    #[test]
    fn vscode_light_tag_color() {
        let style = style_for_highlight(highlight_index("tag"));
        assert_eq!(style.fg, Some(Color::Rgb(128, 0, 0)));
    }

    #[test]
    fn vscode_light_property_color() {
        let style = style_for_highlight(highlight_index("property"));
        assert_eq!(style.fg, Some(Color::Rgb(0, 16, 128)));
    }

    #[test]
    fn vscode_light_operator_default() {
        let style = style_for_highlight(highlight_index("operator"));
        assert_eq!(style, Style::default());
    }

    #[test]
    fn vscode_light_punctuation_default() {
        let style = style_for_highlight(highlight_index("punctuation"));
        assert_eq!(style, Style::default());
    }

    #[test]
    fn highlight_all_languages_load() {
        // Verify that all language configs can be created
        let languages = [
            Language::Rust,
            Language::JavaScript,
            Language::TypeScript,
            Language::Tsx,
            Language::Json,
            Language::Toml,
            Language::Markdown,
            Language::Go,
            Language::Html,
            Language::Bash,
            Language::Yaml,
            Language::Env,
            Language::GitCommit,
        ];
        for lang in languages {
            assert!(
                SyntaxState::new(lang).is_some(),
                "Failed to create SyntaxState for {:?}",
                lang
            );
        }
    }

    #[test]
    fn tree_house_adapter_loads_every_static_grammar() {
        let loader = TreeHouseLoader::new().expect("all tree-house configs should compile");
        for lang in [
            Language::Rust,
            Language::JavaScript,
            Language::TypeScript,
            Language::Tsx,
            Language::Json,
            Language::Toml,
            Language::Markdown,
            Language::Go,
            Language::Html,
            Language::Bash,
            Language::Yaml,
            Language::Env,
            Language::GitCommit,
        ] {
            assert!(
                loader.config_for_language(lang).is_some(),
                "missing tree-house config for {lang:?}"
            );
        }
        assert!(loader.config_for_name("markdown_inline").is_some());
    }

    #[test]
    fn tree_house_adapter_preserves_representative_highlights() {
        for (lang, source) in [
            (Language::Rust, "fn main() { let answer = 42; }"),
            (Language::JavaScript, "const answer = () => 42;"),
            (Language::Json, r#"{"answer": 42}"#),
            (Language::Markdown, "# Title\n\nSome *emphasis*.\n"),
            (Language::GitCommit, "Fix parser\n\n# Comment\n"),
        ] {
            let state = SyntaxState::new(lang)
                .unwrap_or_else(|| panic!("tree-house failed to initialize {lang:?}"));
            let spans = state.highlight(source.as_bytes());
            assert!(
                spans.iter().any(|span| span.style != Style::default()),
                "tree-house emitted no styled span for {lang:?}: {spans:?}"
            );
        }
    }

    #[test]
    fn tree_house_adapter_handles_markdown_inline_and_fenced_rust_injections() {
        let source = "# Title\n\nSome *emphasis*.\n\n```rust\nfn injected() {}\n```\n";
        let state = SyntaxState::new(Language::Markdown).expect("markdown should initialize");
        let spans = state.highlight(source.as_bytes());

        let emphasis = source.find("emphasis").unwrap();
        assert!(spans.iter().any(|span| {
            span.start <= emphasis
                && span.end >= emphasis + "emphasis".len()
                && span.style.add_modifier.contains(Modifier::ITALIC)
        }));

        let function = source.find("injected").unwrap();
        assert!(
            spans.iter().any(|span| {
                span.start <= function
                    && span.end >= function + "injected".len()
                    && span.style == style_for_highlight(highlight_index("function"))
            }),
            "fenced Rust function was not highlighted: {spans:?}"
        );
    }

    #[test]
    fn detect_env() {
        assert_eq!(detect_language(Path::new(".env")), Some(Language::Env));
        assert_eq!(
            detect_language(Path::new(".env.local")),
            Some(Language::Env)
        );
        assert_eq!(detect_language(Path::new("foo.env")), Some(Language::Env));
    }

    #[test]
    fn highlight_rope_caches_a_window_covering_the_requested_range() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = Rope::from_str("fn main() { let x = 42; }\n");
        assert!(!state.cache_is_valid(0, 3..12));

        let spans = state.highlight_rope(source.slice(..), 3..12, 0);

        assert!(!spans.is_empty());
        assert!(state.cache_is_valid(0, 3..12));
        assert!(state.cache_is_valid(0, 5..10));
    }

    #[test]
    fn cache_miss_on_version_change() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = Rope::from_str("fn main() { let x = 42; }\n");
        state.highlight_rope(source.slice(..), 0..source.len_bytes(), 0);
        assert!(state.cache_is_valid(0, 0..source.len_bytes()));
        assert!(!state.cache_is_valid(1, 0..source.len_bytes()));
    }

    #[test]
    fn timed_out_parse_is_attempted_once_per_generation() {
        let state = SyntaxState::with_timeout(Language::Rust, Duration::ZERO).unwrap();
        let source = Rope::from_str(&"fn main() {}\n".repeat(10_000));

        assert!(state
            .highlight_rope(source.slice(..), 0..source.len_bytes(), 0)
            .is_empty());
        assert!(state
            .highlight_rope(source.slice(..), 0..source.len_bytes(), 0)
            .is_empty());
        assert_eq!(state.parse_attempt_count(), 1);

        assert!(state
            .highlight_rope(source.slice(..), 0..source.len_bytes(), 1)
            .is_empty());
        assert_eq!(state.parse_attempt_count(), 2);
    }

    #[test]
    fn three_consecutive_failed_generations_disable_parsing() {
        let state = SyntaxState::with_timeout(Language::Rust, Duration::ZERO).unwrap();
        let source = Rope::from_str(&"fn main() {}\n".repeat(10_000));

        for generation in 0..3 {
            assert!(state
                .highlight_rope(source.slice(..), 0..source.len_bytes(), generation,)
                .is_empty());
        }

        assert!(state.is_disabled());
        state.highlight_rope(source.slice(..), 0..source.len_bytes(), 3);
        assert_eq!(state.parse_attempt_count(), 3);
    }

    #[test]
    fn multiple_worker_edits_are_applied_in_one_incremental_update() {
        use tree_house::tree_sitter::{InputEdit, Point};

        let state = SyntaxState::new(Language::Rust).unwrap();
        let original = Rope::from_str("fn main() {}\n");
        state.highlight_rope(original.slice(..), 0..original.len_bytes(), 0);

        let final_source = Rope::from_str("fn xymain() {}\n");
        let edits = [
            InputEdit {
                start_byte: 3,
                old_end_byte: 3,
                new_end_byte: 4,
                start_point: Point { row: 0, col: 3 },
                old_end_point: Point { row: 0, col: 3 },
                new_end_point: Point { row: 0, col: 4 },
            },
            InputEdit {
                start_byte: 4,
                old_end_byte: 4,
                new_end_byte: 5,
                start_point: Point { row: 0, col: 4 },
                old_end_point: Point { row: 0, col: 4 },
                new_end_point: Point { row: 0, col: 5 },
            },
        ];

        state.apply_edits(final_source.slice(..), &edits);
        state.highlight_rope(final_source.slice(..), 0..final_source.len_bytes(), 2);

        assert_eq!(state.incremental_update_count(), 1);
        assert_eq!(state.full_parse_count(), 1);
    }

    #[test]
    fn padded_cache_covers_nearby_scrolls_but_not_distant_ranges() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = Rope::from_str(&" ".repeat(220_000));
        state.highlight_rope(source.slice(..), 100_000..100_100, 0);

        assert!(state.cache_is_valid(0, 94_000..94_100));
        assert!(state.cache_is_valid(0, 106_000..106_100));
        assert!(!state.cache_is_valid(0, 90_000..90_100));
        assert!(!state.cache_is_valid(0, 110_000..110_100));
    }

    #[test]
    fn incremental_update_matches_a_fresh_full_parse() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let mut source = Rope::from_str("fn main() { let answer = 42; }\n");
        let old = source.to_string();
        let start_byte = old.find("42").unwrap();
        state.highlight_rope(source.slice(..), 0..source.len_bytes(), 0);

        let replacement = "compute(α)";
        let start_char = source.byte_to_char(start_byte);
        source.remove(start_char..start_char + 2);
        source.insert(start_char, replacement);
        state.apply_edit(
            source.slice(..),
            tree_house::tree_sitter::InputEdit {
                start_byte: start_byte as u32,
                old_end_byte: (start_byte + 2) as u32,
                new_end_byte: (start_byte + replacement.len()) as u32,
                start_point: tree_house::tree_sitter::Point {
                    row: 0,
                    col: start_byte as u32,
                },
                old_end_point: tree_house::tree_sitter::Point {
                    row: 0,
                    col: (start_byte + 2) as u32,
                },
                new_end_point: tree_house::tree_sitter::Point {
                    row: 0,
                    col: (start_byte + replacement.len()) as u32,
                },
            },
        );

        let incremental = state.highlight_rope(source.slice(..), 0..source.len_bytes(), 1);
        let fresh = SyntaxState::new(Language::Rust).unwrap();
        let full = fresh.highlight_rope(source.slice(..), 0..source.len_bytes(), 1);

        assert_eq!(
            incremental
                .iter()
                .map(|span| (span.start, span.end, span.style))
                .collect::<Vec<_>>(),
            full.iter()
                .map(|span| (span.start, span.end, span.style))
                .collect::<Vec<_>>()
        );
        assert_eq!(state.full_parse_count(), 1);
        assert_eq!(state.incremental_update_count(), 1);
    }

    #[test]
    fn language_name() {
        assert_eq!(Language::Rust.name(), "Rust");
        assert_eq!(Language::Env.name(), "Env");
        assert_eq!(Language::Json.name(), "JSON");
    }

    #[test]
    fn highlight_markdown_heading() {
        let state = SyntaxState::new(Language::Markdown).unwrap();
        let source = b"# Hello World\n\nSome text.\n";
        let spans = state.highlight(source);
        // Should produce some spans
        assert!(!spans.is_empty());
        // At least one span should have a non-default style (the heading)
        assert!(
            spans.iter().any(|s| s.style != Style::default()),
            "Markdown heading should produce styled spans, got: {:?}",
            spans,
        );
    }

    #[test]
    fn highlight_markdown_inline_emphasis() {
        let state = SyntaxState::new(Language::Markdown).unwrap();
        let source = b"This is *emphasis* and **strong**.\n";
        let spans = state.highlight(source);
        assert!(!spans.is_empty(), "Should produce spans, got: {:?}", spans);
        // Should have italic for emphasis and bold for strong
        let has_italic = spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::ITALIC));
        let has_bold = spans
            .iter()
            .any(|s| s.style.add_modifier.contains(Modifier::BOLD));
        assert!(
            has_italic,
            "Markdown emphasis should produce italic spans, got: {:?}",
            spans,
        );
        assert!(
            has_bold,
            "Markdown strong emphasis should produce bold spans"
        );
    }

    #[test]
    fn highlight_markdown_code_span() {
        let state = SyntaxState::new(Language::Markdown).unwrap();
        let source = b"Use `code` here.\n";
        let spans = state.highlight(source);
        assert!(!spans.is_empty(), "Should produce spans, got: {:?}", spans,);
        // At least one span should have a non-default style (the code span)
        assert!(
            spans.iter().any(|s| s.style != Style::default()),
            "Markdown code span should be styled, got: {:?}",
            spans,
        );
    }
}
