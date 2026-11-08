use ratatui::style::{Color, Modifier, Style};

/// The highlight names we recognize, in order. The index into this array
/// is what `Highlight.0` will be in HighlightEvents.
pub(super) const HIGHLIGHT_NAMES: &[&str] = &[
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

/// Maps a highlight name index to a ratatui Style.
///
/// Only the terminal's ANSI palette is used (no RGB), so highlighting follows
/// the terminal theme -- including live Omarchy theme switches -- in light
/// and dark themes alike. Each capture gets the ANSI slot Omarchy's own Helix
/// template gives it, so both editors color code the same way. `DarkGray`
/// (bright black) is Omarchy's "muted" color. Black and white are never used:
/// one of them is the background in any theme.
pub(crate) fn style_for_highlight(idx: usize) -> Style {
    let name = HIGHLIGHT_NAMES.get(idx).copied().unwrap_or("");
    let fg = |color| Style::default().fg(color);
    match name {
        "comment" => fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        "string" | "text.literal" => fg(Color::Green),
        "string.special" => fg(Color::Blue),
        "number" | "constant" | "constant.builtin" => fg(Color::Yellow),
        "keyword" | "function.macro" | "type.builtin" | "escape" => fg(Color::Magenta),
        "function" | "function.builtin" | "constructor" | "property" | "tag" | "label" => {
            fg(Color::Blue)
        }
        "type" | "attribute" => fg(Color::Yellow),
        "variable.builtin" => fg(Color::Red),
        "variable.parameter" => fg(Color::Magenta).add_modifier(Modifier::ITALIC),
        "operator" | "punctuation.special" => fg(Color::Cyan),
        "text.title" | "markup.heading" => fg(Color::Red).add_modifier(Modifier::BOLD),
        "text.emphasis" => Style::default().add_modifier(Modifier::ITALIC),
        "text.strong" => Style::default().add_modifier(Modifier::BOLD),
        "text.uri" | "markup.link" => {
            fg(Color::Blue).add_modifier(Modifier::ITALIC | Modifier::UNDERLINED)
        }
        "text.reference" => fg(Color::Magenta),
        // "variable", "punctuation", "punctuation.bracket",
        // "punctuation.delimiter": the terminal's default foreground.
        _ => Style::default(),
    }
}
