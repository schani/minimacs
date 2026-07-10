#[allow(dead_code)]
#[path = "../history.rs"]
mod history;
#[allow(dead_code)]
#[path = "../syntax.rs"]
mod syntax;
#[allow(dead_code)]
#[path = "../buffer.rs"]
mod buffer;
#[path = "../syntax_fuzz.rs"]
mod syntax_fuzz;

fn main() {
    syntax_fuzz::main();
}
