use std::io;
use std::path::PathBuf;

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

use crate::app::App;
use crate::editor::Editor;
use crate::event::TerminalEventSource;

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

/// Restores the terminal when dropped, so every error path between
/// `enable_raw_mode()` and the end of `main` (the `?` early returns) leaves
/// the shell usable instead of raw. `restore_terminal()` is best-effort and
/// idempotent, so the panic hook also firing is harmless.
struct RestoreGuard;

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum CliAction {
    /// Open these files, in order (possibly none).
    Open(Vec<PathBuf>),
    Help,
    Version,
    /// Bad usage; print the message and exit with status 2.
    Error(String),
}

/// Parse the CLI arguments (excluding the program name). `-h`/`--help` and
/// `-V`/`--version` win wherever they appear; `--` ends option parsing so
/// files literally named `--help` stay reachable. Other leading-dash
/// arguments are errors — except a lone `-`, which is treated as a file
/// name. Empty arguments are rejected here (opening `""` can only fail).
fn parse_args(args: &[String]) -> CliAction {
    let mut paths = Vec::new();
    let mut only_files = false;
    for arg in args {
        if !only_files {
            match arg.as_str() {
                "--help" | "-h" => return CliAction::Help,
                "--version" | "-V" => return CliAction::Version,
                "--" => {
                    only_files = true;
                    continue;
                }
                s if s.len() > 1 && s.starts_with('-') => {
                    return CliAction::Error(format!("unrecognized option '{s}'"));
                }
                _ => {}
            }
        }
        if arg.is_empty() {
            return CliAction::Error("empty file path argument".to_string());
        }
        paths.push(PathBuf::from(arg));
    }
    CliAction::Open(paths)
}

fn help_text() -> String {
    format!(
        "minimacs {}\n\
         A small terminal Emacs-clone.\n\
         \n\
         Usage: minimacs [OPTIONS] [FILE]...\n\
         \n\
         Opens every FILE; the first is displayed, the rest are reachable\n\
         via C-x b. Arguments after `--` are always treated as file names.\n\
         \n\
         Options:\n\
         \x20 -h, --help       Print this help and exit\n\
         \x20 -V, --version    Print the version and exit\n",
        env!("CARGO_PKG_VERSION")
    )
}

pub(crate) fn run() -> Result<()> {
    // Handle the CLI before any terminal setup, so --help/--version print
    // to a normal screen and bad usage never enters raw mode.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = match parse_args(&args) {
        CliAction::Help => {
            print!("{}", help_text());
            return Ok(());
        }
        CliAction::Version => {
            println!("minimacs {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        CliAction::Error(msg) => {
            eprintln!("minimacs: {msg}");
            eprintln!("Try 'minimacs --help' for more information.");
            std::process::exit(2);
        }
        CliAction::Open(paths) => paths,
    };

    // Set up editor
    let mut editor = Editor::new();
    editor.open_files(&paths);

    // Set up terminal
    install_panic_hook();
    let guard = RestoreGuard;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;

    // Enable kitty keyboard protocol so keys like Ctrl-/ are reported
    // correctly. REPORT_ALTERNATE_KEYS makes shifted keys arrive as the
    // layout-correct shifted character (M-> as Alt+'>' instead of
    // Alt+Shift+'.'); without it, Key::from_event falls back to a US-layout
    // shift table. This is best-effort: terminals that don't support it
    // will silently ignore it.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;

    let mut app = App::new(terminal, editor);
    let mut event_source = TerminalEventSource;

    let result = app.run(&mut event_source);

    // Restore the terminal now rather than relying on the guard going out
    // of scope: `std::process::exit` below skips destructors, and an error
    // from `run` should print on the normal screen.
    drop(guard);

    // An aborted quit (the `a` answer) exits non-zero so callers like git
    // abandon the operation, mirroring vim's :cq.
    if app.editor().quit_aborted() {
        std::process::exit(1);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_no_args_opens_no_files() {
        assert_eq!(parse_args(&[]), CliAction::Open(vec![]));
    }

    #[test]
    fn parse_args_opens_every_file_in_order() {
        assert_eq!(
            parse_args(&args(&["a.txt", "b.txt", "c.txt"])),
            CliAction::Open(vec![
                PathBuf::from("a.txt"),
                PathBuf::from("b.txt"),
                PathBuf::from("c.txt"),
            ])
        );
    }

    #[test]
    fn parse_args_recognizes_help_and_version_flags() {
        for flag in ["--help", "-h"] {
            assert_eq!(parse_args(&args(&[flag])), CliAction::Help);
        }
        for flag in ["--version", "-V"] {
            assert_eq!(parse_args(&args(&[flag])), CliAction::Version);
        }
    }

    #[test]
    fn parse_args_help_wins_even_after_file_arguments() {
        assert_eq!(parse_args(&args(&["a.txt", "--help"])), CliAction::Help);
    }

    #[test]
    fn parse_args_unknown_flag_is_an_error_naming_it() {
        match parse_args(&args(&["--frobnicate"])) {
            CliAction::Error(msg) => assert!(msg.contains("--frobnicate"), "got: {msg}"),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    #[test]
    fn parse_args_double_dash_makes_every_later_argument_a_file() {
        assert_eq!(
            parse_args(&args(&["--", "--help", "-x"])),
            CliAction::Open(vec![PathBuf::from("--help"), PathBuf::from("-x")])
        );
    }

    #[test]
    fn parse_args_lone_dash_is_a_file() {
        assert_eq!(
            parse_args(&args(&["-"])),
            CliAction::Open(vec![PathBuf::from("-")])
        );
    }

    #[test]
    fn parse_args_empty_argument_is_an_error() {
        for list in [args(&[""]), args(&["--", ""])] {
            match parse_args(&list) {
                CliAction::Error(msg) => assert!(msg.contains("empty"), "got: {msg}"),
                other => panic!("expected an error, got {other:?}"),
            }
        }
    }

    #[test]
    fn help_text_names_the_program_version_and_usage() {
        let text = help_text();
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
        assert!(text.contains("Usage: minimacs [OPTIONS] [FILE]..."));
        assert!(text.contains("--help"));
        assert!(text.contains("--version"));
        assert!(text.contains("--"));
    }

    #[test]
    fn restore_guard_restores_on_drop_and_double_restore_is_safe() {
        // The guard's Drop must go through the same best-effort restore as
        // the panic hook; dropping it (and restoring again afterwards, as
        // the panic hook might) must be safe off-tty.
        let guard = RestoreGuard;
        drop(guard);
        restore_terminal();
    }

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
