use ratatui::style::{Color, Style};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};
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
    "type",
    "type.builtin",
    "variable",
    "variable.builtin",
    "variable.parameter",
];

/// A styled span: byte range + style.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}

/// Maps a highlight name index to a ratatui Style.
fn style_for_highlight(idx: usize) -> Style {
    let name = HIGHLIGHT_NAMES.get(idx).copied().unwrap_or("");
    match name {
        "comment" => Style::default().fg(Color::Indexed(243)), // gray
        "string" | "string.special" => Style::default().fg(Color::Indexed(113)), // green
        "number" => Style::default().fg(Color::Indexed(176)), // magenta/pink
        "keyword" => Style::default().fg(Color::Indexed(170)), // purple
        "function" | "function.builtin" | "function.macro" => {
            Style::default().fg(Color::Indexed(75))  // blue
        }
        "type" | "type.builtin" => Style::default().fg(Color::Indexed(186)), // yellow
        "constant" | "constant.builtin" => Style::default().fg(Color::Indexed(173)), // orange
        "variable.builtin" => Style::default().fg(Color::Indexed(204)), // red
        "variable.parameter" => Style::default().fg(Color::Indexed(252)), // light
        "attribute" => Style::default().fg(Color::Indexed(186)), // yellow
        "operator" => Style::default().fg(Color::Indexed(252)), // light
        "constructor" => Style::default().fg(Color::Indexed(186)), // yellow
        "escape" => Style::default().fg(Color::Indexed(173)), // orange
        "tag" => Style::default().fg(Color::Indexed(204)), // red
        "label" => Style::default().fg(Color::Indexed(75)), // blue
        "property" => Style::default().fg(Color::Indexed(152)), // cyan-ish
        "punctuation" | "punctuation.bracket" | "punctuation.delimiter"
        | "punctuation.special" => Style::default().fg(Color::Indexed(248)), // light gray
        "variable" => Style::default(),
        _ => Style::default(),
    }
}

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
}

/// Detect language from file extension.
pub fn detect_language(path: &std::path::Path) -> Option<Language> {
    let ext = path.extension()?.to_str()?;
    match ext {
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
        _ => None,
    }
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
            tree_sitter_md::INJECTION_QUERY_BLOCK.to_string(),
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
        Language::Bash => (
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
    }
}

/// Holds a parsed tree and highlight config for a buffer.
#[allow(dead_code)]
pub struct SyntaxState {
    pub language: Language,
    config: HighlightConfiguration,
}

impl SyntaxState {
    /// Create a new syntax state for the given language.
    pub fn new(lang: Language) -> Option<Self> {
        let (language_fn, highlights, injections, locals) = language_config(lang);
        let result =
            HighlightConfiguration::new(language_fn.into(), "source", &highlights, &injections, &locals);
        let mut config = match result {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Failed to create highlight config for {:?}: {:?}", lang, e);
                return None;
            }
        };
        config.configure(HIGHLIGHT_NAMES);
        Some(SyntaxState {
            language: lang,
            config,
        })
    }

    /// Highlight a slice of source code bytes and return styled spans.
    /// The spans have byte offsets relative to the input `source`.
    pub fn highlight(&self, source: &[u8]) -> Vec<StyledSpan> {
        let mut highlighter = Highlighter::new();
        let events = match highlighter.highlight(&self.config, source, None, |_| None) {
            Ok(events) => events,
            Err(_) => return Vec::new(),
        };

        let mut spans = Vec::new();
        let mut style_stack: Vec<Style> = Vec::new();

        for event in events {
            match event {
                Ok(HighlightEvent::Source { start, end }) => {
                    let style = style_stack.last().copied().unwrap_or_default();
                    if start < end {
                        spans.push(StyledSpan { start, end, style });
                    }
                }
                Ok(HighlightEvent::HighlightStart(highlight)) => {
                    style_stack.push(style_for_highlight(highlight.0));
                }
                Ok(HighlightEvent::HighlightEnd) => {
                    style_stack.pop();
                }
                Err(_) => break,
            }
        }

        spans
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        ];
        for lang in languages {
            assert!(
                SyntaxState::new(lang).is_some(),
                "Failed to create SyntaxState for {:?}",
                lang
            );
        }
    }
}
