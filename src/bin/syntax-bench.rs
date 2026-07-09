#[allow(dead_code)]
#[path = "../syntax.rs"]
mod syntax;
#[path = "../syntax_bench.rs"]
mod syntax_bench;

fn main() {
    syntax_bench::main();
}
