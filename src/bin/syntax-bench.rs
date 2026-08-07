#[allow(dead_code)]
#[path = "../syntax.rs"]
mod syntax;
#[path = "../syntax_bench.rs"]
mod syntax_bench;
#[allow(dead_code)]
#[path = "../syntax_worker.rs"]
mod syntax_worker;

fn main() {
    syntax_bench::main();
}
