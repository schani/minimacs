mod app;
mod buffer;
mod command;
mod display;
mod editor;
mod event;
mod history;
mod indent;
mod keymap;
mod minibuffer;
mod pane;
mod render;
mod runtime;
mod syntax;
mod syntax_bench;
mod syntax_fuzz;
mod syntax_worker;

/// Runs the interactive minimacs editor CLI.
pub fn run_editor() -> anyhow::Result<()> {
    runtime::run()
}

/// Runs the syntax performance harness CLI.
pub fn run_syntax_bench() {
    syntax_bench::run();
}

/// Runs the incremental syntax fuzz harness CLI.
pub fn run_syntax_fuzz() {
    syntax_fuzz::run();
}
