use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
#[cfg(test)]
use ropey::Rope;
use ropey::RopeSlice;
use tree_house::highlighter::{Highlight as TreeHouseHighlight, HighlightEvent as TreeHouseEvent};
use tree_house::{
    InjectionLanguageMarker, Language as TreeHouseLanguage,
    LanguageConfig as TreeHouseLanguageConfig, LanguageLoader as TreeHouseLanguageLoader,
    Syntax as TreeHouseSyntax,
};
use tree_sitter_language::LanguageFn;

/// The highlight names we recognize, in order. The index into this array
/// is what `Highlight.0` will be in HighlightEvents.
const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "keyword",
    "label",
    "number",
    "operator",
    "property",
    "punctuation",
    "punctuation.bracket",
    "punctuation.delimiter",
    "punctuation.special",
    "string",
    "string.special",
    "tag",
    "text.emphasis",
    "text.literal",
    "text.reference",
    "text.strong",
    "text.title",
    "text.uri",
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
    "markup.heading",
    "markup.link",
];

const PARSE_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_CONSECUTIVE_FAILED_GENERATIONS: u8 = 3;
const HIGHLIGHT_CACHE_PADDING: usize = 8 * 1024;
const BACKGROUND_CACHE_WINDOWS: usize = 8;

/// A styled span: byte range + style.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

pub(crate) struct SyntaxCompletion {
    pub(crate) key: usize,
    pub(crate) generation: usize,
    pub(crate) requested: Range<usize>,
    pub(crate) spans: Vec<StyledSpan>,
    pub(crate) disabled: bool,
}

/// Maps a highlight name index to a ratatui Style.
pub(crate) fn style_for_highlight(idx: usize) -> Style {
    let name = HIGHLIGHT_NAMES.get(idx).copied().unwrap_or("");
    match name {
        "comment" => Style::default().fg(Color::Rgb(0, 128, 0)),          // #008000
        "string" | "string.special" => Style::default().fg(Color::Rgb(163, 21, 21)), // #A31515
        "number" => Style::default().fg(Color::Rgb(9, 134, 88)),          // #098658
        "keyword" => Style::default().fg(Color::Rgb(0, 0, 255)),          // #0000FF
        "function" | "function.builtin" | "function.macro" => {
            Style::default().fg(Color::Rgb(121, 94, 38))                  // #795E26
        }
        "type" | "type.builtin" => Style::default().fg(Color::Rgb(38, 127, 153)), // #267F99
        "constant" | "constant.builtin" => Style::default().fg(Color::Rgb(0, 112, 193)), // #0070C1
        "variable.builtin" => Style::default().fg(Color::Rgb(0, 0, 255)), // #0000FF
        "variable.parameter" => Style::default().fg(Color::Rgb(0, 16, 128)), // #001080
        "variable" => Style::default().fg(Color::Rgb(0, 16, 128)),        // #001080
        "attribute" => Style::default().fg(Color::Rgb(38, 127, 153)),     // #267F99
        "constructor" => Style::default().fg(Color::Rgb(38, 127, 153)),   // #267F99
        "escape" => Style::default().fg(Color::Rgb(238, 0, 0)),           // #EE0000
        "tag" => Style::default().fg(Color::Rgb(128, 0, 0)),              // #800000
        "property" => Style::default().fg(Color::Rgb(0, 16, 128)),        // #001080
        "text.title" => Style::default()
            .fg(Color::Rgb(0, 0, 255))
            .add_modifier(Modifier::BOLD),
        "text.emphasis" => Style::default().add_modifier(Modifier::ITALIC),
        "text.strong" => Style::default().add_modifier(Modifier::BOLD),
        "text.literal" => Style::default().fg(Color::Rgb(163, 21, 21)),          // #A31515
        "text.uri" | "markup.link" => Style::default()
            .fg(Color::Rgb(0, 112, 193))
            .add_modifier(Modifier::UNDERLINED),
        "markup.heading" => Style::default()
            .fg(Color::Rgb(0, 0, 255))
            .add_modifier(Modifier::BOLD),
        "text.reference" => Style::default().fg(Color::Rgb(0, 112, 193)),        // #0070C1
        "operator" | "label" | "punctuation" | "punctuation.bracket"
        | "punctuation.delimiter" | "punctuation.special" => Style::default(),
        _ => Style::default(),
    }
}

/// Custom markdown block injection query. The upstream tree-sitter-md query omits
/// `injection.include-children`, which can give injected parsers only the gaps
/// between child nodes. Include the complete fenced and inline content so both
/// nested language highlighting and Markdown inline highlighting see all text.
const MARKDOWN_INJECTION_QUERY: &str = r#"
((fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content)
  (#set! injection.include-children))

((html_block) @injection.content
  (#set! injection.language "html"))

((inline) @injection.content
  (#set! injection.language "markdown_inline")
  (#set! injection.include-children))
"#;

/// Supported languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    Toml,
    Markdown,
    Go,
    Html,
    Bash,
    Yaml,
    Env,
    GitCommit,
}

impl Language {
    /// Human-readable name for display in the mode line.
    pub fn name(&self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Markdown => "Markdown",
            Language::Go => "Go",
            Language::Html => "HTML",
            Language::Bash => "Bash",
            Language::Yaml => "YAML",
            Language::Env => "Env",
            Language::GitCommit => "Git Commit",
        }
    }
}

/// Detect language from file extension or filename.
pub fn detect_language(path: &std::path::Path) -> Option<Language> {
    // Try extension first.
    let by_ext = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| match ext {
            "rs" => Some(Language::Rust),
            "js" | "mjs" | "cjs" | "jsx" => Some(Language::JavaScript),
            "ts" | "mts" | "cts" => Some(Language::TypeScript),
            "tsx" => Some(Language::Tsx),
            "json" => Some(Language::Json),
            "toml" => Some(Language::Toml),
            "md" | "markdown" => Some(Language::Markdown),
            "go" => Some(Language::Go),
            "html" | "htm" => Some(Language::Html),
            "sh" | "bash" | "zsh" => Some(Language::Bash),
            "yml" | "yaml" => Some(Language::Yaml),
            "env" => Some(Language::Env),
            _ => None,
        });
    if by_ext.is_some() {
        return by_ext;
    }

    // Fall back to filename matching.
    let name = path.file_name()?.to_str()?;
    if name == ".env" || name.starts_with(".env.") {
        return Some(Language::Env);
    }
    // Git message files (commit, merge, tag, notes, branch description).
    if matches!(
        name,
        "COMMIT_EDITMSG" | "MERGE_MSG" | "TAG_EDITMSG" | "NOTES_EDITMSG" | "EDIT_DESCRIPTION"
    ) {
        return Some(Language::GitCommit);
    }

    None
}

/// Get the language function and query strings for a language.
fn language_config(lang: Language) -> (LanguageFn, String, String, String) {
    match lang {
        Language::Rust => (
            tree_sitter_rust::LANGUAGE,
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_string(),
            tree_sitter_rust::INJECTIONS_QUERY.to_string(),
            String::new(),
        ),
        Language::JavaScript => (
            tree_sitter_javascript::LANGUAGE,
            tree_sitter_javascript::HIGHLIGHT_QUERY.to_string(),
            tree_sitter_javascript::INJECTIONS_QUERY.to_string(),
            tree_sitter_javascript::LOCALS_QUERY.to_string(),
        ),
        Language::TypeScript => (
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            String::new(),
            tree_sitter_typescript::LOCALS_QUERY.to_string(),
        ),
        Language::Tsx => (
            tree_sitter_typescript::LANGUAGE_TSX,
            format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            ),
            String::new(),
            tree_sitter_typescript::LOCALS_QUERY.to_string(),
        ),
        Language::Json => (
            tree_sitter_json::LANGUAGE,
            tree_sitter_json::HIGHLIGHTS_QUERY.to_string(),
            String::new(),
            String::new(),
        ),
        Language::Toml => (
            tree_sitter_toml_ng::LANGUAGE,
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_string(),
            String::new(),
            String::new(),
        ),
        Language::Markdown => (
            tree_sitter_md::LANGUAGE,
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_string(),
            // Custom injection query: the upstream query omits injection.include-children
            // on the inline injection, which causes the inline parser to receive empty ranges
            // (the block parser's (inline) node has internal children that get excluded).
            MARKDOWN_INJECTION_QUERY.to_string(),
            String::new(),
        ),
        Language::Go => (
            tree_sitter_go::LANGUAGE,
            tree_sitter_go::HIGHLIGHTS_QUERY.to_string(),
            String::new(),
            String::new(),
        ),
        Language::Html => (
            tree_sitter_html::LANGUAGE,
            tree_sitter_html::HIGHLIGHTS_QUERY.to_string(),
            tree_sitter_html::INJECTIONS_QUERY.to_string(),
            String::new(),
        ),
        Language::Bash | Language::Env => (
            tree_sitter_bash::LANGUAGE,
            tree_sitter_bash::HIGHLIGHT_QUERY.to_string(),
            String::new(),
            String::new(),
        ),
        Language::Yaml => (
            tree_sitter_yaml::LANGUAGE,
            tree_sitter_yaml::HIGHLIGHTS_QUERY.to_string(),
            String::new(),
            String::new(),
        ),
        Language::GitCommit => (
            tree_sitter_gitcommit::LANGUAGE,
            tree_sitter_gitcommit::HIGHLIGHTS_QUERY.to_string(),
            // The upstream injections query injects a diff grammar (for
            // `commit -v`) which we don't ship; skip injections.
            String::new(),
            String::new(),
        ),
    }
}

struct TreeHouseConfigEntry {
    language: Option<Language>,
    names: &'static [&'static str],
    config: TreeHouseLanguageConfig,
}

/// Adapter between minimacs' statically linked grammar crates and tree-house's
/// language-loader API. Tree-house normally loads Helix's shared grammar
/// libraries, but its bindings also accept the `LanguageFn` values exported by
/// the grammar crates we already ship.
struct TreeHouseLoader {
    configs: Vec<TreeHouseConfigEntry>,
}

static TREE_HOUSE_LOADER: OnceLock<Result<TreeHouseLoader, String>> = OnceLock::new();

fn tree_house_loader() -> Result<&'static TreeHouseLoader, &'static str> {
    match TREE_HOUSE_LOADER.get_or_init(TreeHouseLoader::new) {
        Ok(loader) => Ok(loader),
        Err(error) => Err(error.as_str()),
    }
}

impl TreeHouseLoader {
    fn new() -> Result<Self, String> {
        const LANGUAGES: &[Language] = &[
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
            Language::GitCommit,
        ];

        let mut configs = Vec::with_capacity(LANGUAGES.len() + 1);
        for &language in LANGUAGES {
            let (language_fn, highlights, mut injections, locals) = language_config(language);
            // tree-sitter-javascript's injections query contains an editor-specific
            // `#offset!` predicate for Glimmer templates. Tree-house rejects unknown
            // predicates, and minimacs does not ship or resolve a Glimmer grammar, so
            // this query could not produce a usable injection in either implementation.
            if language == Language::JavaScript {
                injections.clear();
            }
            let grammar = tree_house::tree_sitter::Grammar::try_from(language_fn)
                .map_err(|error| format!("failed to load {language:?} grammar: {error}"))?;
            let config = TreeHouseLanguageConfig::new(
                grammar,
                &highlights,
                &injections,
                &locals,
            )
            .map_err(|error| format!("failed to compile {language:?} queries: {error}"))?;
            config.configure(highlight_for_capture);
            configs.push(TreeHouseConfigEntry {
                language: Some(language),
                names: injection_names(language),
                config,
            });
        }

        let inline_grammar = tree_house::tree_sitter::Grammar::try_from(
            tree_sitter_md::INLINE_LANGUAGE,
        )
        .map_err(|error| format!("failed to load Markdown inline grammar: {error}"))?;
        let inline_config = TreeHouseLanguageConfig::new(
            inline_grammar,
            tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
            tree_sitter_md::INJECTION_QUERY_INLINE,
            "",
        )
        .map_err(|error| format!("failed to compile Markdown inline queries: {error}"))?;
        inline_config.configure(highlight_for_capture);
        configs.push(TreeHouseConfigEntry {
            language: None,
            names: &["markdown_inline"],
            config: inline_config,
        });

        Ok(Self { configs })
    }

    fn id_for_language(&self, language: Language) -> Option<TreeHouseLanguage> {
        let language = if language == Language::Env {
            Language::Bash
        } else {
            language
        };
        self.configs
            .iter()
            .position(|entry| entry.language == Some(language))
            .map(|idx| TreeHouseLanguage::new(idx as u32))
    }

    fn config_for_language(&self, language: Language) -> Option<&TreeHouseLanguageConfig> {
        self.id_for_language(language)
            .and_then(|id| self.get_config(id))
    }

    fn id_for_name(&self, name: &str) -> Option<TreeHouseLanguage> {
        let name = name.trim().to_ascii_lowercase();
        self.configs
            .iter()
            .position(|entry| entry.names.contains(&name.as_str()))
            .map(|idx| TreeHouseLanguage::new(idx as u32))
    }

    #[cfg(test)]
    fn config_for_name(&self, name: &str) -> Option<&TreeHouseLanguageConfig> {
        self.id_for_name(name).and_then(|id| self.get_config(id))
    }
}

impl TreeHouseLanguageLoader for TreeHouseLoader {
    fn language_for_marker(
        &self,
        marker: InjectionLanguageMarker<'_>,
    ) -> Option<TreeHouseLanguage> {
        match marker {
            InjectionLanguageMarker::Name(name) => self.id_for_name(name),
            InjectionLanguageMarker::Match(text) | InjectionLanguageMarker::Shebang(text) => {
                let text: Cow<'_, str> = text.into();
                self.id_for_name(&text)
            }
            InjectionLanguageMarker::Filename(filename) => {
                let filename: Cow<'_, str> = filename.into();
                detect_language(Path::new(filename.as_ref()))
                    .and_then(|language| self.id_for_language(language))
            }
        }
    }

    fn get_config(&self, language: TreeHouseLanguage) -> Option<&TreeHouseLanguageConfig> {
        self.configs
            .get(language.idx())
            .map(|entry| &entry.config)
    }
}

fn injection_names(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &["rust", "rs"],
        Language::JavaScript => &["javascript", "js", "jsx"],
        Language::TypeScript => &["typescript", "ts"],
        Language::Tsx => &["tsx"],
        Language::Json => &["json"],
        Language::Toml => &["toml"],
        Language::Markdown => &["markdown", "md"],
        Language::Go => &["go", "golang"],
        Language::Html => &["html"],
        Language::Bash | Language::Env => &["bash", "sh", "shell", "zsh", "env"],
        Language::Yaml => &["yaml", "yml"],
        Language::GitCommit => &["gitcommit", "git-commit"],
    }
}

fn highlight_for_capture(capture: &str) -> Option<TreeHouseHighlight> {
    HIGHLIGHT_NAMES
        .iter()
        .enumerate()
        .filter(|(_, name)| {
            capture == **name
                || capture
                    .strip_prefix(**name)
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
        .max_by_key(|(_, name)| name.len())
        .map(|(idx, _)| TreeHouseHighlight::new(idx as u32))
}

fn tree_house_spans(
    syntax: &TreeHouseSyntax,
    source: RopeSlice<'_>,
    loader: &TreeHouseLoader,
    range: Range<usize>,
) -> Vec<StyledSpan> {
    let start = range.start.min(source.len_bytes());
    let end = range.end.min(source.len_bytes()).max(start);
    let mut highlighter = tree_house::highlighter::Highlighter::new(
        syntax,
        source,
        loader,
        start as u32..end as u32,
    );
    let mut spans = Vec::new();
    let mut position = start;
    let mut style = Style::default();

    loop {
        let next = (highlighter.next_event_offset() as usize).min(end);
        if position < next {
            spans.push(StyledSpan {
                start: position,
                end: next,
                style,
            });
            position = next;
        }
        if position >= end {
            break;
        }

        let (event, highlights) = highlighter.advance();
        let base = match event {
            TreeHouseEvent::Refresh => Style::default(),
            TreeHouseEvent::Push => style,
        };
        style = highlights.fold(base, |current, highlight| {
            current.patch(style_for_highlight(highlight.idx()))
        });
    }

    spans
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
        background.caches.clear();
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

    pub(crate) fn cached_background_spans(
        &self,
        requested: Range<usize>,
        generation: usize,
    ) -> Option<Vec<StyledSpan>> {
        let background = self.background.borrow();
        let cache = background.caches.iter().rev().find(|cache| {
            cache.version == generation
                && cache.range.start <= requested.start
                && cache.range.end >= requested.end
        })?;
        Some(
            cache
                .spans
                .iter()
                .filter(|span| span.end > requested.start && span.start < requested.end)
                .cloned()
                .collect(),
        )
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
        if completion.key != self.background_key
            || completion.generation != current_generation
        {
            return false;
        }
        let mut background = self.background.borrow_mut();
        self.disabled.set(completion.disabled);
        background.confirmed_generation = Some(completion.generation);
        background
            .pending_edits
            .retain(|versioned| versioned.generation > completion.generation);
        background
            .caches
            .retain(|cache| cache.version == completion.generation);
        if background.caches.len() == BACKGROUND_CACHE_WINDOWS {
            background.caches.pop_front();
        }
        background.caches.push_back(BackgroundHighlightCache {
            version: completion.generation,
            range: completion.requested,
            spans: completion.spans,
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
                cache.spans
                    .iter()
                    .filter(|span| span.end > requested.start && span.start < requested.end)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
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
        assert_eq!(
            detect_language(Path::new("App.tsx")),
            Some(Language::Tsx)
        );
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
            spans
                .iter()
                .any(|s| s.start <= comment_start && s.end > comment_start
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
        assert_eq!(
            detect_language(Path::new("foo.env")),
            Some(Language::Env)
        );
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
                .highlight_rope(
                    source.slice(..),
                    0..source.len_bytes(),
                    generation,
                )
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
                start_point: tree_house::tree_sitter::Point { row: 0, col: start_byte as u32 },
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
        assert!(has_bold, "Markdown strong emphasis should produce bold spans");
    }

    #[test]
    fn highlight_markdown_code_span() {
        let state = SyntaxState::new(Language::Markdown).unwrap();
        let source = b"Use `code` here.\n";
        let spans = state.highlight(source);
        assert!(
            !spans.is_empty(),
            "Should produce spans, got: {:?}",
            spans,
        );
        // At least one span should have a non-default style (the code span)
        assert!(
            spans.iter().any(|s| s.style != Style::default()),
            "Markdown code span should be styled, got: {:?}",
            spans,
        );
    }
}
