use std::borrow::Cow;
use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::Style;
use ropey::RopeSlice;
use tree_house::highlighter::{Highlight as TreeHouseHighlight, HighlightEvent as TreeHouseEvent};
use tree_house::{
    InjectionLanguageMarker, Language as TreeHouseLanguage,
    LanguageConfig as TreeHouseLanguageConfig, LanguageLoader as TreeHouseLanguageLoader,
    Syntax as TreeHouseSyntax,
};
use tree_sitter_language::LanguageFn;

use super::theme::{style_for_highlight, HIGHLIGHT_NAMES};
use super::StyledSpan;

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
pub(super) fn language_config(lang: Language) -> (LanguageFn, String, String, String) {
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
pub(super) struct TreeHouseLoader {
    configs: Vec<TreeHouseConfigEntry>,
}

static TREE_HOUSE_LOADER: OnceLock<Result<TreeHouseLoader, String>> = OnceLock::new();

pub(super) fn tree_house_loader() -> Result<&'static TreeHouseLoader, &'static str> {
    match TREE_HOUSE_LOADER.get_or_init(TreeHouseLoader::new) {
        Ok(loader) => Ok(loader),
        Err(error) => Err(error.as_str()),
    }
}

impl TreeHouseLoader {
    pub(super) fn new() -> Result<Self, String> {
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
            let config =
                TreeHouseLanguageConfig::new(grammar, &highlights, &injections, &locals)
                    .map_err(|error| format!("failed to compile {language:?} queries: {error}"))?;
            config.configure(highlight_for_capture);
            configs.push(TreeHouseConfigEntry {
                language: Some(language),
                names: injection_names(language),
                config,
            });
        }

        let inline_grammar =
            tree_house::tree_sitter::Grammar::try_from(tree_sitter_md::INLINE_LANGUAGE)
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

    pub(super) fn id_for_language(&self, language: Language) -> Option<TreeHouseLanguage> {
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

    pub(super) fn config_for_language(
        &self,
        language: Language,
    ) -> Option<&TreeHouseLanguageConfig> {
        self.id_for_language(language)
            .and_then(|id| self.get_config(id))
    }

    pub(super) fn id_for_name(&self, name: &str) -> Option<TreeHouseLanguage> {
        let name = name.trim().to_ascii_lowercase();
        self.configs
            .iter()
            .position(|entry| entry.names.contains(&name.as_str()))
            .map(|idx| TreeHouseLanguage::new(idx as u32))
    }

    #[cfg(test)]
    pub(super) fn config_for_name(&self, name: &str) -> Option<&TreeHouseLanguageConfig> {
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
        self.configs.get(language.idx()).map(|entry| &entry.config)
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

pub(super) fn tree_house_spans(
    syntax: &TreeHouseSyntax,
    source: RopeSlice<'_>,
    loader: &TreeHouseLoader,
    range: Range<usize>,
) -> Vec<StyledSpan> {
    let start = range.start.min(source.len_bytes());
    let end = range.end.min(source.len_bytes()).max(start);
    let mut highlighter =
        tree_house::highlighter::Highlighter::new(syntax, source, loader, start as u32..end as u32);
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
