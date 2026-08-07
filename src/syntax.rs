mod languages;
mod state;
mod theme;

use ratatui::style::Style;

pub use languages::{detect_language, Language};
pub use state::SyntaxState;
pub(crate) use state::{BackgroundEdit, SyntaxCompletion};

/// A styled span: byte range + style.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}
