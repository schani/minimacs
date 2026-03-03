mod app;
mod buffer;
mod command;
mod editor;
mod event;
mod history;
mod keymap;
mod minibuffer;
mod pane;
mod render;
mod syntax;

use std::io;

use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;
use editor::Editor;
use event::TerminalEventSource;

fn main() -> Result<()> {
    // Parse CLI args: optional file path
    let args: Vec<String> = std::env::args().collect();
    let file_path = args.get(1);

    // Set up editor
    let mut editor = Editor::new();
    if let Some(path) = file_path {
        let path = std::path::Path::new(path);
        if let Err(e) = editor.open_file(path) {
            editor.minibuffer.show_message(format!("{}", e));
        }
    }

    // Set up terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(terminal, editor);
    let mut event_source = TerminalEventSource;

    let result = app.run(&mut event_source);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        app.terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste
    )?;
    app.terminal.show_cursor()?;

    result
}
