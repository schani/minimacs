#[allow(dead_code)]
#[path = "../syntax.rs"]
mod syntax;
#[allow(dead_code)]
#[path = "../syntax_worker.rs"]
mod syntax_worker;
#[path = "../syntax_bench.rs"]
mod syntax_bench;

fn main() {
    syntax_bench::main();
}
