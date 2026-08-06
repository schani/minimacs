//! Reusable minimacs editor core and frontend support.
//!
//! The terminal binary and native macOS frontend both use the same [`Editor`]
//! and [`Command`] model. Frontends are responsible for translating platform
//! input into commands/events and presenting the resulting editor state.

pub mod app;
pub mod buffer;
pub mod command;
pub mod editor;
pub mod event;
pub mod history;
mod indent;
mod keymap;
pub mod minibuffer;
pub mod pane;
mod render;
pub mod syntax;
mod syntax_worker;

pub use command::Command;
pub use editor::Editor;
