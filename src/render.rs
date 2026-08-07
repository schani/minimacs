mod layout;
mod widgets;

/// Immutable input-state projection needed by rendering. Pending chord/ESC
/// display remains owned by App's `InputState`; the editor has no mirror.
#[derive(Debug, Clone, Copy)]
pub struct PendingInput<'a> {
    pub display: &'a str,
}

pub use widgets::render;

pub(crate) use layout::screen_layout;
