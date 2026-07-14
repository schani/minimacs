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
pub(crate) fn style_for_highlight(idx: usize) -> Style {
    let name = HIGHLIGHT_NAMES.get(idx).copied().unwrap_or("");
    match name {
        "comment" => Style::default().fg(Color::Rgb(0, 128, 0)), // #008000
        "string" | "string.special" => Style::default().fg(Color::Rgb(163, 21, 21)), // #A31515
        "number" => Style::default().fg(Color::Rgb(9, 134, 88)), // #098658
        "keyword" => Style::default().fg(Color::Rgb(0, 0, 255)), // #0000FF
        "function" | "function.builtin" | "function.macro" => {
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
        "operator"
        | "label"
        | "punctuation"
        | "punctuation.bracket"
        | "punctuation.delimiter"
        | "punctuation.special" => Style::default(),
        _ => Style::default(),
    }
}
