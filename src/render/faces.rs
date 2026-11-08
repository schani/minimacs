//! UI faces, drawn only from the terminal's own palette.
//!
//! Every face uses the terminal's default foreground/background (`Reset`),
//! the 16 named ANSI colors, and text modifiers -- never RGB or 256-color
//! indexes. Terminal themes (Omarchy's included) rewrite exactly that
//! palette, so minimacs follows a theme switch live without knowing about
//! it, in light and dark themes alike.
//!
//! The role assignments mirror Omarchy's terminal templates: ANSI black is
//! the theme *background* there, white its foreground, and bright black
//! (`DarkGray`) its muted color. So `Black` is only ever used as text on a
//! colored background (where it reads as "background-colored text"), and
//! never on the default background, where it would be invisible.

use ratatui::style::{Color, Modifier, Style};

/// Mode line of a pane. The focused one is an inverted band, which has the
/// maximum contrast any theme offers; unfocused ones sit on the muted color.
pub(super) fn mode_line(focused: bool) -> Style {
    if focused {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().fg(Color::Reset).bg(Color::DarkGray)
    }
}

/// Text between mark and point.
pub(super) fn region() -> Style {
    Style::default().fg(Color::Reset).bg(Color::DarkGray)
}

/// The isearch match point is on (the terminal's own "focused match").
pub(super) fn search_current_match() -> Style {
    Style::default().fg(Color::Black).bg(Color::Red)
}

/// Every other isearch match.
pub(super) fn search_other_match() -> Style {
    Style::default().fg(Color::Black).bg(Color::Yellow)
}

/// The `│` bars between side-by-side panes.
pub(super) fn separator() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// The `\` wrap marker and `~` past-end-of-buffer markers.
pub(super) fn fringe() -> Style {
    Style::default().fg(Color::DarkGray)
}

/// The minibuffer completion list: the terminal's own colors, so it reads
/// like the rest of the screen instead of a fixed light-gray box.
pub(super) fn completions() -> Style {
    Style::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_faces() -> Vec<(&'static str, Style)> {
        vec![
            ("mode_line(focused)", mode_line(true)),
            ("mode_line(unfocused)", mode_line(false)),
            ("region", region()),
            ("search_current_match", search_current_match()),
            ("search_other_match", search_other_match()),
            ("separator", separator()),
            ("fringe", fringe()),
            ("completions", completions()),
        ]
    }

    #[test]
    fn faces_use_only_the_terminal_palette() {
        for (name, style) in all_faces() {
            for color in [style.fg, style.bg].into_iter().flatten() {
                assert!(
                    !matches!(color, Color::Rgb(..) | Color::Indexed(_)),
                    "{name} hardcodes {color:?}"
                );
            }
        }
    }

    #[test]
    fn black_or_white_text_only_on_a_colored_background() {
        for (name, style) in all_faces() {
            if matches!(style.fg, Some(Color::Black | Color::White)) {
                assert!(
                    !matches!(style.bg, None | Some(Color::Reset)),
                    "{name} paints {:?} text on the default background",
                    style.fg
                );
            }
        }
    }

    #[test]
    fn focused_mode_line_is_an_inverted_band() {
        let style = mode_line(true);
        assert!(style.add_modifier.contains(Modifier::REVERSED));
        assert!(style.add_modifier.contains(Modifier::BOLD));
        assert_eq!((style.fg, style.bg), (None, None));
        assert_ne!(mode_line(true), mode_line(false));
    }

    #[test]
    fn unfocused_mode_line_and_region_sit_on_the_muted_color() {
        assert_eq!(mode_line(false).bg, Some(Color::DarkGray));
        assert_eq!(region().bg, Some(Color::DarkGray));
    }

    #[test]
    fn search_matches_match_the_terminals_own_search_colors() {
        assert_eq!(search_current_match().bg, Some(Color::Red));
        assert_eq!(search_other_match().bg, Some(Color::Yellow));
        assert_ne!(search_current_match(), search_other_match());
    }

    #[test]
    fn completion_list_uses_the_default_colors() {
        assert_eq!(completions(), Style::default());
    }
}
