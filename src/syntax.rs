use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
use ropey::{Rope, RopeSlice};
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

/// A styled span: byte range + style.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub start: usize,
    pub end: usize,
    pub style: Style,
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
    cached_end_byte: usize,
    spans: Vec<StyledSpan>,
}

/// Holds a parsed tree and highlight config for a buffer.
pub struct SyntaxState {
    pub language: Language,
    cache: RefCell<Option<HighlightCache>>,
}

impl SyntaxState {
    /// Create a new syntax state for the given language.
    pub fn new(lang: Language) -> Option<Self> {
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
            cache: RefCell::new(None),
        })
    }

    /// Highlight a slice of source code bytes and return styled spans.
    /// The spans have byte offsets relative to the input `source`.
    pub fn highlight(&self, source: &[u8]) -> Vec<StyledSpan> {
        let Ok(source) = std::str::from_utf8(source) else {
            return Vec::new();
        };
        let Ok(loader) = tree_house_loader() else {
            return Vec::new();
        };
        let Some(language) = loader.id_for_language(self.language) else {
            return Vec::new();
        };
        let rope = Rope::from_str(source);
        let Ok(syntax) = TreeHouseSyntax::new(
            rope.slice(..),
            language,
            Duration::from_secs(2),
            loader,
        ) else {
            return Vec::new();
        };
        tree_house_spans(&syntax, rope.slice(..), loader, 0..source.len())
    }

    /// Check if the cached highlight result covers the needed range at the right version.
    pub fn cache_is_valid(&self, version: usize, end_byte: usize) -> bool {
        let cache = self.cache.borrow();
        match cache.as_ref() {
            Some(c) => c.version == version && c.cached_end_byte >= end_byte,
            None => false,
        }
    }

    /// Run highlight and store the result in the cache.
    pub fn highlight_and_cache(&self, source: &[u8], version: usize) {
        let spans = self.highlight(source);
        *self.cache.borrow_mut() = Some(HighlightCache {
            version,
            cached_end_byte: source.len(),
            spans,
        });
    }

    /// Borrow the cached spans. Panics if cache is empty.
    pub fn cached_spans(&self) -> std::cell::Ref<'_, Vec<StyledSpan>> {
        std::cell::Ref::map(self.cache.borrow(), |c| &c.as_ref().unwrap().spans)
    }
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
    fn cache_hit_same_version_and_range() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = b"fn main() { let x = 42; }";
        assert!(!state.cache_is_valid(0, source.len()));

        state.highlight_and_cache(source, 0);
        assert!(state.cache_is_valid(0, source.len()));
        // Smaller range is still covered
        assert!(state.cache_is_valid(0, 10));
    }

    #[test]
    fn cache_miss_on_version_change() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = b"fn main() { let x = 42; }";
        state.highlight_and_cache(source, 0);
        assert!(state.cache_is_valid(0, source.len()));
        // Different version invalidates
        assert!(!state.cache_is_valid(1, source.len()));
    }

    #[test]
    fn cache_miss_on_range_growth() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let source = b"fn main() {}";
        state.highlight_and_cache(source, 0);
        assert!(state.cache_is_valid(0, source.len()));
        // Larger range invalidates
        assert!(!state.cache_is_valid(0, source.len() + 100));
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
