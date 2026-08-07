use std::hint::black_box;
use std::ops::Range;
use std::time::{Duration, Instant};

use ropey::Rope;
use tree_house::tree_sitter::{InputEdit, Point};

use crate::syntax::{Language, StyledSpan, SyntaxState};
use crate::syntax_worker::{SyntaxJob, SyntaxWorker};

const DEFAULT_LINES: usize = 10_000;
const DEFAULT_EDITS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectedMode {
    All,
    Full,
    Incremental,
    Background,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    mode: SelectedMode,
    lines: usize,
    edits: usize,
    help: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            mode: SelectedMode::All,
            lines: DEFAULT_LINES,
            edits: DEFAULT_EDITS,
            help: false,
        }
    }
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self::default();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "-h" | "--help" => options.help = true,
                "--mode" => {
                    index += 1;
                    let value = args.get(index).ok_or("--mode requires a value")?;
                    options.mode = match value.as_str() {
                        "all" => SelectedMode::All,
                        "full" => SelectedMode::Full,
                        "incremental" => SelectedMode::Incremental,
                        "background" => SelectedMode::Background,
                        "none" => SelectedMode::None,
                        _ => return Err(format!("unknown mode '{value}'")),
                    };
                }
                "--lines" => {
                    index += 1;
                    options.lines = positive_value(args.get(index), "--lines")?;
                }
                "--edits" => {
                    index += 1;
                    options.edits = positive_value(args.get(index), "--edits")?;
                }
                argument => return Err(format!("unrecognized option '{argument}'")),
            }
            index += 1;
        }
        Ok(options)
    }
}

fn positive_value(value: Option<&String>, option: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{option} requires a value"))?;
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{option} requires a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy)]
enum BenchMode {
    Full,
    Incremental,
    Background,
    None,
}

impl BenchMode {
    fn name(self) -> &'static str {
        match self {
            Self::Full => "full parsing",
            Self::Incremental => "incremental parsing",
            Self::Background => "background worker",
            Self::None => "no parsing",
        }
    }
}

struct Workload {
    source: String,
    target_byte: usize,
    target_point: Point,
    viewport: Range<usize>,
}

impl Workload {
    fn generate(lines: usize) -> Self {
        let target_line = lines / 2;
        let mut source = String::new();
        let mut target_byte = 0;
        let mut target_point = Point::ZERO;
        for line in 0..lines {
            if line == target_line {
                let prefix = "fn edit_target() -> usize { let value = ";
                target_byte = source.len() + prefix.len();
                target_point = Point {
                    row: line as u32,
                    col: prefix.len() as u32,
                };
                source.push_str(prefix);
                source.push_str("0; value }\n");
            } else {
                source.push_str(&format!(
                    "fn generated_{line}() -> usize {{ let value = {line}; value }}\n"
                ));
            }
        }
        let viewport_start = source[..target_byte]
            .rfind('\n')
            .map_or(0, |offset| offset + 1);
        let viewport_end = source[target_byte..]
            .find('\n')
            .map_or(source.len(), |offset| target_byte + offset + 1);
        Self {
            source,
            target_byte,
            target_point,
            viewport: viewport_start..viewport_end,
        }
    }

    fn edit(&self) -> InputEdit {
        InputEdit {
            start_byte: self.target_byte as u32,
            old_end_byte: (self.target_byte + 1) as u32,
            new_end_byte: (self.target_byte + 1) as u32,
            start_point: self.target_point,
            old_end_point: Point {
                row: self.target_point.row,
                col: self.target_point.col + 1,
            },
            new_end_point: Point {
                row: self.target_point.row,
                col: self.target_point.col + 1,
            },
        }
    }
}

struct RunResult {
    mode: BenchMode,
    elapsed: Duration,
    edit_elapsed: Duration,
    parse_elapsed: Duration,
    highlight_elapsed: Duration,
    dispatch_elapsed: Duration,
    text_checksum: u64,
    highlight_checksum: u64,
}

fn run_mode(mode: BenchMode, workload: &Workload, edits: usize) -> RunResult {
    let mut rope = Rope::from_str(&workload.source);
    let target_char = rope.byte_to_char(workload.target_byte);
    let edit = workload.edit();
    let incremental = if matches!(mode, BenchMode::Incremental) {
        let state = SyntaxState::new(Language::Rust).expect("Rust syntax configuration");
        black_box(state.parse_rope(rope.slice(..)));
        black_box(state.highlight_rope(rope.slice(..), workload.viewport.clone(), 0));
        Some(state)
    } else {
        // Initialize the shared grammar/query loader outside the timed region.
        let _ = SyntaxState::new(Language::Rust);
        None
    };
    let background = matches!(mode, BenchMode::Background).then(SyntaxWorker::new);
    let background_state = matches!(mode, BenchMode::Background)
        .then(|| SyntaxState::new(Language::Rust).expect("Rust syntax configuration"));
    if let (Some(worker), Some(state)) = (background.as_ref(), background_state.as_ref()) {
        worker.submit(SyntaxJob {
            key: state.background_key(),
            language: Language::Rust,
            base_generation: None,
            generation: 0,
            source: rope.clone(),
            edits: Vec::new(),
            requested: workload.viewport.clone(),
        });
        let completion = wait_for_completion(worker, state.background_key(), 0);
        assert!(state.accept_background_completion(completion, 0));
    }

    let mut highlight_results = Vec::with_capacity(edits);
    let mut edit_elapsed = Duration::ZERO;
    let mut parse_elapsed = Duration::ZERO;
    let mut highlight_elapsed = Duration::ZERO;
    let mut dispatch_elapsed = Duration::ZERO;
    let started = Instant::now();
    for generation in 1..=edits {
        let replacement = if generation % 2 == 1 { "1" } else { "0" };
        let edit_started = Instant::now();
        rope.remove(target_char..target_char + 1);
        rope.insert(target_char, replacement);
        edit_elapsed += edit_started.elapsed();

        let spans = match mode {
            BenchMode::None => Vec::new(),
            BenchMode::Full => {
                let state = SyntaxState::new(Language::Rust).expect("Rust syntax configuration");
                let parse_started = Instant::now();
                black_box(state.parse_rope(rope.slice(..)));
                parse_elapsed += parse_started.elapsed();
                let highlight_started = Instant::now();
                let spans =
                    state.highlight_rope(rope.slice(..), workload.viewport.clone(), generation);
                highlight_elapsed += highlight_started.elapsed();
                spans
            }
            BenchMode::Incremental => {
                let state = incremental.as_ref().unwrap();
                let parse_started = Instant::now();
                state.apply_edit(rope.slice(..), edit);
                parse_elapsed += parse_started.elapsed();
                let highlight_started = Instant::now();
                let spans =
                    state.highlight_rope(rope.slice(..), workload.viewport.clone(), generation);
                highlight_elapsed += highlight_started.elapsed();
                spans
            }
            BenchMode::Background => {
                let worker = background.as_ref().unwrap();
                let state = background_state.as_ref().unwrap();
                let dispatch_started = Instant::now();
                state.note_background_edit(generation, edit);
                let (base_generation, edits) = state.background_update_for(generation);
                worker.submit(SyntaxJob {
                    key: state.background_key(),
                    language: Language::Rust,
                    base_generation,
                    generation,
                    source: rope.clone(),
                    edits,
                    requested: workload.viewport.clone(),
                });
                dispatch_elapsed += dispatch_started.elapsed();
                let wait_started = Instant::now();
                let completion = wait_for_completion(worker, state.background_key(), generation);
                parse_elapsed += wait_started.elapsed();
                let spans = completion
                    .spans
                    .iter()
                    .filter(|span| {
                        span.end > workload.viewport.start && span.start < workload.viewport.end
                    })
                    .cloned()
                    .collect();
                assert!(state.accept_background_completion(completion, generation));
                spans
            }
        };
        // Keep the spans alive so highlighting cannot be optimized away;
        // checksumming stays outside the timed region.
        highlight_results.push(spans);
    }
    let elapsed = started.elapsed();
    black_box(&highlight_results);

    let highlight_checksum = highlight_results.iter().fold(0_u64, |checksum, spans| {
        checksum.rotate_left(1) ^ spans_checksum(spans)
    });

    RunResult {
        mode,
        elapsed,
        edit_elapsed,
        parse_elapsed,
        highlight_elapsed,
        dispatch_elapsed,
        text_checksum: bytes_checksum(rope.to_string().as_bytes()),
        highlight_checksum,
    }
}

fn wait_for_completion(
    worker: &SyntaxWorker,
    key: usize,
    generation: usize,
) -> crate::syntax::SyntaxCompletion {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(completion) = worker
            .take_completions()
            .into_iter()
            .find(|completion| completion.key == key)
        {
            if completion.generation == generation {
                return completion;
            }
        }
        assert!(
            Instant::now() < deadline,
            "syntax worker benchmark timed out"
        );
        std::thread::yield_now();
    }
}

fn spans_checksum(spans: &[StyledSpan]) -> u64 {
    let mut checksum = 0xcbf29ce484222325_u64;
    for span in spans {
        let style = format!("{:?}", span.style);
        for byte in span
            .start
            .to_le_bytes()
            .iter()
            .chain(span.end.to_le_bytes().iter())
            .chain(style.as_bytes())
        {
            checksum ^= u64::from(*byte);
            checksum = checksum.wrapping_mul(0x100000001b3);
        }
    }
    checksum
}

fn bytes_checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

fn selected_modes(mode: SelectedMode) -> &'static [BenchMode] {
    match mode {
        SelectedMode::All => &[
            BenchMode::None,
            BenchMode::Full,
            BenchMode::Incremental,
            BenchMode::Background,
        ],
        SelectedMode::Full => &[BenchMode::Full],
        SelectedMode::Incremental => &[BenchMode::Incremental],
        SelectedMode::Background => &[BenchMode::Background],
        SelectedMode::None => &[BenchMode::None],
    }
}

fn help_text() -> &'static str {
    concat!(
        "syntax-bench - compare minimacs edit parsing strategies\n",
        "\n",
        "Usage: syntax-bench [OPTIONS]\n",
        "\n",
        "Options:\n",
        "  --mode MODE    all, full, incremental, background, or none (default: all)\n",
        "  --lines N      generated Rust source lines (default: 10000)\n",
        "  --edits N      single-character edits to apply (default: 100)\n",
        "  -h, --help     print this help\n",
    )
}

pub(crate) fn run() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let options = match Options::parse(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("syntax-bench: {error}");
            eprintln!("Try 'syntax-bench --help' for more information.");
            std::process::exit(2);
        }
    };
    if options.help {
        print!("{}", help_text());
        return;
    }
    if cfg!(debug_assertions) {
        eprintln!("warning: use --release for meaningful timings");
    }

    let workload = Workload::generate(options.lines);
    println!(
        "source: {} lines, {} bytes; edits: {}",
        options.lines,
        workload.source.len(),
        options.edits
    );
    let results = selected_modes(options.mode)
        .iter()
        .map(|mode| run_mode(*mode, &workload, options.edits))
        .collect::<Vec<_>>();

    println!(
        "mode                     total    per edit        rope    dispatch       parse   highlight"
    );
    for result in &results {
        let seconds = result.elapsed.as_secs_f64();
        println!(
            "{:<22} {:>8.2} ms {:>8.2} ms {:>8.2} ms {:>8.2} ms {:>8.2} ms {:>8.2} ms",
            result.mode.name(),
            seconds * 1_000.0,
            seconds * 1_000.0 / options.edits as f64,
            result.edit_elapsed.as_secs_f64() * 1_000.0,
            result.dispatch_elapsed.as_secs_f64() * 1_000.0,
            result.parse_elapsed.as_secs_f64() * 1_000.0,
            result.highlight_elapsed.as_secs_f64() * 1_000.0,
        );
    }

    if results
        .windows(2)
        .any(|pair| pair[0].text_checksum != pair[1].text_checksum)
    {
        eprintln!("error: benchmark modes produced different final text");
        std::process::exit(1);
    }
    let full = results
        .iter()
        .find(|result| matches!(result.mode, BenchMode::Full));
    let incremental = results
        .iter()
        .find(|result| matches!(result.mode, BenchMode::Incremental));
    if let (Some(full), Some(incremental)) = (full, incremental) {
        if full.highlight_checksum != incremental.highlight_checksum {
            eprintln!("error: full and incremental highlight results differ");
            std::process::exit(1);
        }
        println!(
            "incremental speedup over full parsing: {:.2}x (highlight checksums match)",
            full.elapsed.as_secs_f64() / incremental.elapsed.as_secs_f64()
        );
    }
    if let (Some(incremental), Some(background)) = (
        incremental,
        results
            .iter()
            .find(|result| matches!(result.mode, BenchMode::Background)),
    ) {
        if incremental.highlight_checksum != background.highlight_checksum {
            eprintln!("error: incremental and background highlight results differ");
            std::process::exit(1);
        }
        println!(
            "background UI dispatch: {:.3} ms/edit (highlight checksums match)",
            background.dispatch_elapsed.as_secs_f64() * 1_000.0 / options.edits as f64
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_benchmark_options() {
        let options = Options::parse(&[
            "--mode".into(),
            "incremental".into(),
            "--lines".into(),
            "40".into(),
            "--edits".into(),
            "7".into(),
        ])
        .unwrap();

        assert_eq!(options.mode, SelectedMode::Incremental);
        assert_eq!(options.lines, 40);
        assert_eq!(options.edits, 7);
    }

    #[test]
    fn accepts_background_worker_mode() {
        let options = Options::parse(&["--mode".into(), "background".into()]).unwrap();
        assert_eq!(options.mode, SelectedMode::Background);
    }

    #[test]
    fn rejects_zero_sized_runs_and_unknown_modes() {
        assert!(Options::parse(&["--lines".into(), "0".into()]).is_err());
        assert!(Options::parse(&["--edits".into(), "0".into()]).is_err());
        assert!(Options::parse(&["--mode".into(), "sometimes".into()]).is_err());
    }

    #[test]
    fn help_indents_options_for_scannability() {
        assert!(help_text().contains("\n  --mode MODE"));
        assert!(help_text().contains("\n  -h, --help"));
    }

    #[test]
    fn all_modes_apply_identical_edits_and_parsers_agree() {
        let workload = Workload::generate(20);
        let none = run_mode(BenchMode::None, &workload, 4);
        let full = run_mode(BenchMode::Full, &workload, 4);
        let incremental = run_mode(BenchMode::Incremental, &workload, 4);
        let background = run_mode(BenchMode::Background, &workload, 4);

        assert_eq!(none.text_checksum, full.text_checksum);
        assert_eq!(full.text_checksum, incremental.text_checksum);
        assert_eq!(full.highlight_checksum, incremental.highlight_checksum);
        assert_eq!(
            incremental.highlight_checksum,
            background.highlight_checksum
        );
        // The phase columns are measured inside the totaled loop, so they can
        // never exceed it; checksum work happens outside the timed region.
        for result in [&none, &full, &incremental, &background] {
            assert!(
                result.edit_elapsed
                    + result.dispatch_elapsed
                    + result.parse_elapsed
                    + result.highlight_elapsed
                    <= result.elapsed
            );
        }
        assert_eq!(none.parse_elapsed, Duration::ZERO);
        assert_eq!(none.highlight_elapsed, Duration::ZERO);
        assert_eq!(none.dispatch_elapsed, Duration::ZERO);
        assert!(full.parse_elapsed > Duration::ZERO);
        assert!(full.highlight_elapsed > Duration::ZERO);
        assert!(incremental.parse_elapsed > Duration::ZERO);
        assert!(incremental.highlight_elapsed > Duration::ZERO);
        assert!(background.dispatch_elapsed > Duration::ZERO);
        assert!(background.parse_elapsed > Duration::ZERO);
        assert_eq!(background.highlight_elapsed, Duration::ZERO);
    }
}
