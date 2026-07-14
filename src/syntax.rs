#[path = "syntax/languages.rs"]
mod languages;
#[path = "syntax/state.rs"]
mod state;
#[path = "syntax/theme.rs"]
mod theme;

use ratatui::style::Style;

#[allow(unused_imports)] // The standalone syntax harnesses use only Language.
pub use languages::{detect_language, Language};
pub use state::SyntaxState;

#[allow(unused_imports)] // Each standalone harness uses a different subset.
pub(crate) use state::{BackgroundEdit, SyntaxCompletion};

/// A styled span: byte range + style.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub start: usize,
    pub end: usize,
    pub style: Style,
}
