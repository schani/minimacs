//! The color theme: one `Style` per UI role and per syntax capture.
//!
//! `Theme::light_plus()` (VS Code Light+) is the built-in default.

use std::sync::{Arc, LazyLock};

use ratatui::style::{Color, Modifier, Style};

/// The highlight names we recognize, in order. The index into this array
/// is what `Highlight.0` will be in HighlightEvents.
pub(crate) const HIGHLIGHT_NAMES: &[&str] = &[
    "attribute",
    "comment",
    "comment.documentation",
    "constant",
    "constant.builtin",
    "constructor",
    "escape",
    "function",
    "function.builtin",
    "function.macro",
    "function.method",
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

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Theme {
    /// Base style of buffer text and the minibuffer.
    pub(crate) text: Style,
    pub(crate) mode_line_focused: Style,
    pub(crate) mode_line_unfocused: Style,
    /// Patched over the syntax style of text between mark and point.
    pub(crate) region: Style,
    /// Patched over the syntax style of the isearch match at point.
    pub(crate) search_current: Style,
    /// Patched over the syntax style of every other isearch match.
    pub(crate) search_other: Style,
    /// The `│` bars between side-by-side panes.
    pub(crate) separator: Style,
    /// The `\` wrap marker and `~` past-end-of-buffer markers.
    pub(crate) fringe: Style,
    /// The minibuffer completion list.
    pub(crate) completions: Style,
    /// Indexed like `HIGHLIGHT_NAMES`.
    highlights: Vec<Style>,
}

impl Theme {
    /// VS Code's Light+ palette, for light terminals.
    pub(crate) fn light_plus() -> Self {
        // These replace the syntax style outright rather than layering on it.
        let replacing = Style::default().remove_modifier(Modifier::all());
        Self {
            text: Style::default(),
            mode_line_focused: Style::default()
                .bg(Color::Rgb(0, 122, 204))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            mode_line_unfocused: Style::default()
                .bg(Color::Rgb(223, 223, 223))
                .fg(Color::Rgb(51, 51, 51)),
            region: replacing.fg(Color::Reset).bg(Color::Rgb(173, 214, 255)),
            search_current: replacing.fg(Color::Black).bg(Color::Rgb(168, 172, 148)),
            search_other: replacing.fg(Color::Black).bg(Color::Rgb(248, 201, 171)),
            separator: Style::default()
                .fg(Color::Rgb(200, 200, 200))
                .bg(Color::Rgb(255, 255, 255)),
            fringe: Style::default().fg(Color::Rgb(35, 120, 147)),
            completions: Style::default()
                .bg(Color::Rgb(243, 243, 243))
                .fg(Color::Black),
            highlights: HIGHLIGHT_NAMES
                .iter()
                .map(|name| light_plus_highlight(name))
                .collect(),
        }
    }

    /// The process-wide `light_plus()` instance.
    pub(crate) fn shared_default() -> Arc<Self> {
        static DEFAULT: LazyLock<Arc<Theme>> = LazyLock::new(|| Arc::new(Theme::light_plus()));
        Arc::clone(&DEFAULT)
    }

    /// The style of the capture at `idx` in `HIGHLIGHT_NAMES`.
    pub(crate) fn highlight(&self, idx: usize) -> Style {
        self.highlights.get(idx).copied().unwrap_or_default()
    }
}

fn light_plus_highlight(name: &str) -> Style {
    match name {
        "comment" | "comment.documentation" => Style::default().fg(Color::Rgb(0, 128, 0)), // #008000
        "string" | "string.special" => Style::default().fg(Color::Rgb(163, 21, 21)), // #A31515
        "number" => Style::default().fg(Color::Rgb(9, 134, 88)),                     // #098658
        "keyword" => Style::default().fg(Color::Rgb(0, 0, 255)),                     // #0000FF
        "function" | "function.builtin" | "function.macro" | "function.method" => {
            Style::default().fg(Color::Rgb(121, 94, 38)) // #795E26
        }
        "type" | "type.builtin" => Style::default().fg(Color::Rgb(38, 127, 153)), // #267F99
        "constant" | "constant.builtin" => Style::default().fg(Color::Rgb(0, 112, 193)), // #0070C1
        "variable.builtin" => Style::default().fg(Color::Rgb(0, 0, 255)),         // #0000FF
        "variable.parameter" => Style::default().fg(Color::Rgb(0, 16, 128)),      // #001080
        "variable" => Style::default().fg(Color::Rgb(0, 16, 128)),                // #001080
        "attribute" => Style::default().fg(Color::Rgb(38, 127, 153)),             // #267F99
        "constructor" => Style::default().fg(Color::Rgb(38, 127, 153)),           // #267F99
        "escape" => Style::default().fg(Color::Rgb(238, 0, 0)),                   // #EE0000
        "tag" => Style::default().fg(Color::Rgb(128, 0, 0)),                      // #800000
        "property" => Style::default().fg(Color::Rgb(0, 16, 128)),                // #001080
        "text.title" => Style::default()
            .fg(Color::Rgb(0, 0, 255))
            .add_modifier(Modifier::BOLD),
        "text.emphasis" => Style::default().add_modifier(Modifier::ITALIC),
        "text.strong" => Style::default().add_modifier(Modifier::BOLD),
        "text.literal" => Style::default().fg(Color::Rgb(163, 21, 21)), // #A31515
        "text.uri" | "markup.link" => Style::default()
            .fg(Color::Rgb(0, 112, 193))
            .add_modifier(Modifier::UNDERLINED),
        "markup.heading" => Style::default()
            .fg(Color::Rgb(0, 0, 255))
            .add_modifier(Modifier::BOLD),
        "text.reference" => Style::default().fg(Color::Rgb(0, 112, 193)), // #0070C1
        _ => Style::default(),
    }
}
