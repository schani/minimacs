use std::cell::RefCell;

use ratatui::style::{Color, Modifier, Style};
use ropey::Rope;
use tree_sitter::{Language as TsLanguage, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};
use tree_sitter_language::LanguageFn;

/// The highlight names we recognize, in order. The index into this array
/// is what `highlight_map` entries refer to.
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
        "text.uri" => Style::default()
            .fg(Color::Rgb(0, 112, 193))
            .add_modifier(Modifier::UNDERLINED),
        "text.reference" => Style::default().fg(Color::Rgb(0, 112, 193)),        // #0070C1
        "operator" | "label" | "punctuation" | "punctuation.bracket"
        | "punctuation.delimiter" | "punctuation.special" => Style::default(),
        _ => Style::default(),
    }
}

/// Custom markdown block injection query. The upstream tree-sitter-md query omits
/// `injection.include-children` on the inline injection, which causes the inline
/// parser to receive empty ranges. We add it here so inline highlighting works.
const MARKDOWN_INJECTION_QUERY: &str = r#"
(fenced_code_block
  (info_string
    (language) @injection.language)
  (code_fence_content) @injection.content)

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

    None
}

/// Get the language function and query strings for a language.
/// Returns (language_fn, highlights, injections, locals).
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
    }
}

/// Holds the compiled query and metadata for highlighting a single language.
struct HighlightConfig {
    language: TsLanguage,
    query: Query,
    /// Index of the first pattern that belongs to the highlights section.
    /// Patterns before this are injections/locals.
    highlights_pattern_index: usize,
    /// Maps capture index → HIGHLIGHT_NAMES index. None means the capture is not a highlight.
    highlight_map: Vec<Option<usize>>,
    injection_content_capture_index: Option<u32>,
    injection_language_capture_index: Option<u32>,
}

/// Match a capture name against HIGHLIGHT_NAMES using dot-prefix matching.
/// Returns the index of the best (longest) matching highlight name.
///
/// Example: capture "function.builtin" matches both "function" (1 part)
/// and "function.builtin" (2 parts). The 2-part match wins.
fn best_highlight_match(capture_name: &str) -> Option<usize> {
    let capture_parts: Vec<&str> = capture_name.split('.').collect();
    let mut best_index = None;
    let mut best_match_len = 0;

    for (j, recognized_name) in HIGHLIGHT_NAMES.iter().enumerate() {
        let mut matches = true;
        let mut match_len = 0;
        for (i, part) in recognized_name.split('.').enumerate() {
            match capture_parts.get(i) {
                Some(capture_part) if *capture_part == part => {
                    match_len += 1;
                }
                _ => {
                    matches = false;
                    break;
                }
            }
        }
        if matches && match_len > best_match_len {
            best_index = Some(j);
            best_match_len = match_len;
        }
    }

    best_index
}

impl HighlightConfig {
    /// Create a new HighlightConfig from the given language and query strings.
    /// Query source is built by concatenating injections + locals + highlights
    /// (same order as tree-sitter-highlight).
    fn new(
        language_fn: LanguageFn,
        highlights: &str,
        injections: &str,
        locals: &str,
    ) -> Option<Self> {
        let language: TsLanguage = language_fn.into();

        // Concatenate: injections, then locals, then highlights
        let mut combined = String::new();
        combined.push_str(injections);
        combined.push_str(locals);
        let highlights_start_offset = combined.len();
        combined.push_str(highlights);

        let query = match Query::new(&language, &combined) {
            Ok(q) => q,
            Err(e) => {
                eprintln!("Failed to create query: {:?}", e);
                return None;
            }
        };

        // Find the first pattern that starts at or after the highlights section
        let mut highlights_pattern_index = query.pattern_count();
        for i in 0..query.pattern_count() {
            if query.start_byte_for_pattern(i) >= highlights_start_offset {
                highlights_pattern_index = i;
                break;
            }
        }

        // Build highlight_map: for each capture, find the best HIGHLIGHT_NAMES match
        let highlight_map: Vec<Option<usize>> = query
            .capture_names()
            .iter()
            .map(|name| best_highlight_match(name))
            .collect();

        // Find injection capture indices
        let injection_content_capture_index =
            query.capture_index_for_name("injection.content");
        let injection_language_capture_index =
            query.capture_index_for_name("injection.language");

        Some(HighlightConfig {
            language,
            query,
            highlights_pattern_index,
            highlight_map,
            injection_content_capture_index,
            injection_language_capture_index,
        })
    }
}

struct HighlightCache {
    version: usize,
    cached_end_byte: usize,
    spans: Vec<StyledSpan>,
}

/// Holds a parsed tree and highlight config for a buffer.
#[allow(dead_code)]
pub struct SyntaxState {
    pub language: Language,
    config: HighlightConfig,
    /// Additional language configs for injection (e.g. markdown inline).
    injection_configs: Vec<(String, HighlightConfig)>,
    parser: RefCell<Parser>,
    tree: RefCell<Option<Tree>>,
    cache: RefCell<Option<HighlightCache>>,
}

/// TextProvider implementation for Rope. Yields byte chunks for query predicate evaluation.
/// Uses Vec<u8> per node (predicates are evaluated infrequently).
struct RopeProvider<'a>(&'a Rope);

impl<'a> tree_sitter::TextProvider<Vec<u8>> for RopeProvider<'a> {
    type I = std::iter::Once<Vec<u8>>;

    fn text(&mut self, node: Node) -> Self::I {
        let range = node.start_byte()..node.end_byte();
        let mut bytes = Vec::with_capacity(range.len());
        for chunk in self.0.byte_slice(range).chunks() {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        std::iter::once(bytes)
    }
}

/// Parse a Rope using tree-sitter's parse_with_options callback for zero-copy access.
fn parse_rope(parser: &mut Parser, rope: &Rope, old_tree: Option<&Tree>) -> Option<Tree> {
    let rope_len = rope.len_bytes();
    parser.parse_with_options(
        &mut |byte_offset: usize, _: Point| -> &[u8] {
            if byte_offset >= rope_len {
                return &[];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte_offset);
            &chunk.as_bytes()[(byte_offset - chunk_start)..]
        },
        old_tree,
        None,
    )
}

/// Convert a byte offset in a Rope to (row, column).
fn rope_byte_to_point(rope: &Rope, byte_offset: usize) -> Point {
    let byte_offset = byte_offset.min(rope.len_bytes());
    let row = rope.byte_to_line(byte_offset);
    let line_start = rope.line_to_byte(row);
    Point {
        row,
        column: byte_offset - line_start,
    }
}

impl SyntaxState {
    /// Create a new syntax state for the given language.
    pub fn new(lang: Language) -> Option<Self> {
        let (language_fn, highlights, injections, locals) = language_config(lang);
        let config = HighlightConfig::new(language_fn, &highlights, &injections, &locals)?;

        let mut parser = Parser::new();
        if parser.set_language(&config.language).is_err() {
            return None;
        }

        let mut injection_configs = Vec::new();
        if lang == Language::Markdown {
            if let Some(inline_config) = HighlightConfig::new(
                tree_sitter_md::INLINE_LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE,
                tree_sitter_md::INJECTION_QUERY_INLINE,
                "",
            ) {
                injection_configs.push(("markdown_inline".to_string(), inline_config));
            }
        }

        Some(SyntaxState {
            language: lang,
            config,
            injection_configs,
            parser: RefCell::new(parser),
            tree: RefCell::new(None),
            cache: RefCell::new(None),
        })
    }

    /// Highlight from a Rope and return styled spans with byte offsets.
    pub fn highlight_rope(&self, rope: &Rope) -> Vec<StyledSpan> {
        // Parse using zero-copy callback from Rope chunks
        let tree = {
            let mut parser = self.parser.borrow_mut();
            parser.set_language(&self.config.language).ok();
            let _ = parser.set_included_ranges(&[]);
            match parse_rope(&mut parser, rope, None) {
                Some(t) => t,
                None => return Vec::new(),
            }
        };

        // Run highlight captures on the main language
        let mut spans = run_highlight_captures(&self.config, tree.root_node(), rope);

        // Process injections
        self.process_injections(rope, tree.root_node(), &mut spans);

        // Store tree for potential incremental reuse
        *self.tree.borrow_mut() = Some(tree);

        spans
    }

    /// Process language injections (e.g., markdown inline content).
    fn process_injections(
        &self,
        rope: &Rope,
        root_node: Node,
        spans: &mut Vec<StyledSpan>,
    ) {
        let config = &self.config;

        if config.injection_content_capture_index.is_none() {
            return;
        }
        let content_capture_idx = config.injection_content_capture_index.unwrap();

        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(0..rope.len_bytes());

        let mut matches = cursor.matches(&config.query, root_node, RopeProvider(rope));
        let mut injections: Vec<(String, Vec<std::ops::Range<usize>>, bool)> = Vec::new();

        while let Some(m) = matches.next() {
            if m.pattern_index >= config.highlights_pattern_index {
                continue;
            }

            let mut lang_name = None;
            let mut content_ranges = Vec::new();

            for prop in config.query.property_settings(m.pattern_index) {
                if prop.key.as_ref() == "injection.language" {
                    if let Some(ref val) = prop.value {
                        lang_name = Some(val.to_string());
                    }
                }
            }

            let _include_children = config.query.property_settings(m.pattern_index).iter()
                .any(|p| p.key.as_ref() == "injection.include-children");

            for cap in m.captures {
                if cap.index == content_capture_idx {
                    content_ranges.push(cap.node.byte_range());
                }
                if let Some(lang_cap_idx) = config.injection_language_capture_index {
                    if cap.index == lang_cap_idx {
                        // Read the node text from the Rope
                        let range = cap.node.byte_range();
                        let text: String = rope.byte_slice(range).into();
                        lang_name = Some(text);
                    }
                }
            }

            if let Some(name) = lang_name {
                if !content_ranges.is_empty() {
                    injections.push((name, content_ranges, _include_children));
                }
            }
        }
        drop(matches);

        // Group injections by language name
        let mut grouped: std::collections::HashMap<String, Vec<std::ops::Range<usize>>> =
            std::collections::HashMap::new();
        for (name, ranges, _include_children) in injections {
            grouped
                .entry(name)
                .or_default()
                .push(ranges.into_iter().next().unwrap());
        }

        // Process each injection language
        for (lang_name, ranges) in &grouped {
            let injection_config = match self.injection_configs.iter()
                .find(|(name, _)| name == lang_name)
            {
                Some((_, config)) => config,
                None => continue,
            };

            // Build tree-sitter Ranges with proper points computed from the Rope
            let ts_ranges: Vec<tree_sitter::Range> = ranges.iter().map(|range| {
                tree_sitter::Range {
                    start_byte: range.start,
                    end_byte: range.end,
                    start_point: rope_byte_to_point(rope, range.start),
                    end_point: rope_byte_to_point(rope, range.end),
                }
            }).collect();

            // Parse the injection using zero-copy Rope callback
            let mut inj_parser = Parser::new();
            if inj_parser.set_language(&injection_config.language).is_err() {
                continue;
            }
            if inj_parser.set_included_ranges(&ts_ranges).is_err() {
                continue;
            }

            let inj_tree = match parse_rope(&mut inj_parser, rope, None) {
                Some(t) => t,
                None => continue,
            };

            let inj_spans = run_highlight_captures(injection_config, inj_tree.root_node(), rope);
            spans.extend(inj_spans);
        }
    }

    /// Check if the cached highlight result covers the needed range at the right version.
    pub fn cache_is_valid(&self, version: usize, end_byte: usize) -> bool {
        let cache = self.cache.borrow();
        match cache.as_ref() {
            Some(c) => c.version == version && c.cached_end_byte >= end_byte,
            None => false,
        }
    }

    /// Run highlight on a Rope and store the result in the cache.
    pub fn highlight_and_cache(&self, rope: &Rope, end_byte: usize, version: usize) {
        let spans = self.highlight_rope(rope);
        *self.cache.borrow_mut() = Some(HighlightCache {
            version,
            cached_end_byte: end_byte,
            spans,
        });
    }

    /// Borrow the cached spans. Panics if cache is empty.
    pub fn cached_spans(&self) -> std::cell::Ref<'_, Vec<StyledSpan>> {
        std::cell::Ref::map(self.cache.borrow(), |c| &c.as_ref().unwrap().spans)
    }
}

/// Run highlight captures on a tree node using a Rope for text access.
fn run_highlight_captures(
    config: &HighlightConfig,
    root_node: Node,
    rope: &Rope,
) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(0..rope.len_bytes());

    let mut captures = cursor.captures(&config.query, root_node, RopeProvider(rope));
    while let Some((m, capture_index)) = captures.next() {
        if m.pattern_index < config.highlights_pattern_index {
            continue;
        }

        let capture = &m.captures[*capture_index];
        if let Some(highlight_idx) = config.highlight_map.get(capture.index as usize).copied().flatten() {
            let style = style_for_highlight(highlight_idx);
            if style != Style::default() {
                let node = capture.node;
                spans.push(StyledSpan {
                    start: node.start_byte(),
                    end: node.end_byte(),
                    style,
                });
            }
        }
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;
    use std::path::Path;

    /// Helper to create a Rope from a byte string.
    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

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
        let r = rope("fn main() { let x = 42; }");
        let spans = state.highlight_rope(&r);
        assert!(!spans.is_empty());
        let has_keyword = spans.iter().any(|s| s.start == 0 && s.end == 2);
        assert!(has_keyword, "Should have a span for 'fn', got: {:?}", spans);
    }

    #[test]
    fn highlight_json() {
        let state = SyntaxState::new(Language::Json).unwrap();
        let r = rope(r#"{"key": "value", "num": 123}"#);
        let spans = state.highlight_rope(&r);
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
        let r = rope("fn main() { let x = 42; }");
        let len = r.len_bytes();
        assert!(!state.cache_is_valid(0, len));

        state.highlight_and_cache(&r, len, 0);
        assert!(state.cache_is_valid(0, len));
        assert!(state.cache_is_valid(0, 10));
    }

    #[test]
    fn cache_miss_on_version_change() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let r = rope("fn main() { let x = 42; }");
        let len = r.len_bytes();
        state.highlight_and_cache(&r, len, 0);
        assert!(state.cache_is_valid(0, len));
        assert!(!state.cache_is_valid(1, len));
    }

    #[test]
    fn cache_miss_on_range_growth() {
        let state = SyntaxState::new(Language::Rust).unwrap();
        let r = rope("fn main() {}");
        let len = r.len_bytes();
        state.highlight_and_cache(&r, len, 0);
        assert!(state.cache_is_valid(0, len));
        assert!(!state.cache_is_valid(0, len + 100));
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
        let r = rope("# Hello World\n\nSome text.\n");
        let spans = state.highlight_rope(&r);
        assert!(!spans.is_empty());
        assert!(
            spans.iter().any(|s| s.style != Style::default()),
            "Markdown heading should produce styled spans, got: {:?}",
            spans,
        );
    }

    #[test]
    fn highlight_markdown_inline_emphasis() {
        let state = SyntaxState::new(Language::Markdown).unwrap();
        let r = rope("This is *emphasis* and **strong**.\n");
        let spans = state.highlight_rope(&r);
        assert!(!spans.is_empty(), "Should produce spans, got: {:?}", spans);
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
        let r = rope("Use `code` here.\n");
        let spans = state.highlight_rope(&r);
        assert!(
            !spans.is_empty(),
            "Should produce spans, got: {:?}",
            spans,
        );
        assert!(
            spans.iter().any(|s| s.style != Style::default()),
            "Markdown code span should be styled, got: {:?}",
            spans,
        );
    }

    #[test]
    fn best_highlight_match_exact() {
        assert_eq!(best_highlight_match("keyword"), Some(highlight_index("keyword")));
    }

    #[test]
    fn best_highlight_match_prefix() {
        assert_eq!(best_highlight_match("keyword.return"), Some(highlight_index("keyword")));
    }

    #[test]
    fn best_highlight_match_specific() {
        assert_eq!(best_highlight_match("function.builtin"), Some(highlight_index("function.builtin")));
    }

    #[test]
    fn best_highlight_match_none() {
        assert_eq!(best_highlight_match("nonexistent"), None);
    }
}
