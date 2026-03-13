pub mod app;
pub mod buffer;
pub mod command;
pub mod editor;
pub mod event;
pub mod history;
pub mod keymap;
pub mod minibuffer;
pub mod pane;
pub mod render;
pub mod syntax;

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
use editor::{Editor, VimMode};
use event::TerminalEventSource;

pub enum KeybindingMode {
    Emacs,
    Vim,
}

pub fn run(mode: KeybindingMode) -> Result<()> {
    // Parse CLI args: optional --vim flag and file path
    let args: Vec<String> = std::env::args().collect();
    let invoked_as_minivim = args
        .first()
        .and_then(|a| std::path::Path::new(a).file_name())
        .is_some_and(|name| name == "minivim");
    let vim_mode = matches!(mode, KeybindingMode::Vim)
        || invoked_as_minivim
        || args.iter().any(|a| a == "--vim");
    let file_path = args.iter().skip(1).find(|a| !a.starts_with('-'));

    // Set up editor
    let mut editor = Editor::new();
    if vim_mode {
        editor.vim_mode = Some(VimMode::Normal);
    }
    if let Some(path) = file_path {
        let path = std::path::Path::new(path);
        if let Err(e) = editor.open_file(path) {
            editor.minibuffer.show_message(format!("{}", e));
        }
    }

    // Set up terminal
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

    // Restore terminal
    disable_raw_mode()?;
    let _ = execute!(app.terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(
        app.terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableBracketedPaste,
        DisableMouseCapture
    )?;
    app.terminal.show_cursor()?;

    result
}
