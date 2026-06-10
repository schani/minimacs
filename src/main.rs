mod app;
mod buffer;
mod command;
mod editor;
mod event;
mod history;
mod indent;
mod keymap;
mod minibuffer;
mod pane;
mod render;
mod syntax;

use std::io;

use anyhow::Result;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;
use editor::Editor;
use event::TerminalEventSource;

/// Best-effort terminal restoration. Used by both the normal exit path and
/// the panic hook, so every step must run even if earlier ones fail.
fn restore_terminal() {
    let mut stdout = io::stdout();
    let _ = disable_raw_mode();
    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    let _ = execute!(
        stdout,
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture,
        crossterm::cursor::Show
    );
}

/// Restore the terminal before the default panic handler prints, so the
/// message lands on the normal screen instead of being swallowed by the
/// alternate screen, and the shell is left usable.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));
}

fn main() -> Result<()> {
    // Parse CLI args: optional file path
    let args: Vec<String> = std::env::args().collect();
    let file_path = args.get(1);

    // Set up editor
    let mut editor = Editor::new();
    if let Some(path) = file_path {
        let path = std::path::Path::new(path);
        if let Err(e) = editor.open_file(path) {
            editor.minibuffer.show_message(format!("{e}"));
        }
    }

    // Set up terminal
    install_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture)?;

    // Enable kitty keyboard protocol so keys like Ctrl-/ are reported correctly.
    // This is best-effort: terminals that don't support it will silently ignore it.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(terminal, editor);
    let mut event_source = TerminalEventSource;

    let result = app.run(&mut event_source);

    // Restore terminal (best-effort: a failure in one step must not skip the rest)
    restore_terminal();

    // An aborted quit (the `a` answer) exits non-zero so callers like git
    // abandon the operation, mirroring vim's :cq.
    if app.editor.quit_abort {
        std::process::exit(1);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_terminal_is_best_effort_and_idempotent() {
        // Under `cargo test` stdout is not a tty; every step must tolerate
        // failure without panicking, and repeated calls must be safe.
        restore_terminal();
        restore_terminal();
    }

    #[test]
    fn panic_hook_restores_terminal_and_chains_to_previous_hook() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let chained = Arc::new(AtomicBool::new(false));
        let chained_clone = chained.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |_| {
            chained_clone.store(true, Ordering::SeqCst);
        }));

        install_panic_hook();
        let result = std::panic::catch_unwind(|| panic!("boom"));

        std::panic::set_hook(previous);
        assert!(result.is_err());
        assert!(
            chained.load(Ordering::SeqCst),
            "panic hook must chain to the previously installed hook"
        );
    }
}
