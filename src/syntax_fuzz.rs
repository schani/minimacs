use std::ops::Range;
use std::time::Duration;

use ratatui::style::Style;
use ropey::RopeSlice;
use tree_house::tree_sitter::{Grammar, InputEdit, Parser, Tree};

use crate::buffer::Buffer;
use crate::syntax::{Language, StyledSpan, SyntaxState};

const RAW_PARSE_TIMEOUT: Duration = Duration::from_secs(5);

const DEFAULT_SEED: u64 = 1;
const DEFAULT_RUNS: usize = 4;
const DEFAULT_STEPS: usize = 250;
/// Above this size a run resets to a fresh template so debug-mode fresh
/// parses stay fast even after many large pastes.
const MAX_BUFFER_CHARS: usize = 64_000;
const MIN_BUFFER_CHARS: usize = 32;

/// Languages fuzzed by default: Markdown exercises injections, YAML
/// indentation-sensitive error recovery, Rust macros (which self-inject),
/// TypeScript template literals.
const DEFAULT_LANGUAGES: &[Language] = &[
    Language::Rust,
    Language::Markdown,
    Language::TypeScript,
    Language::Yaml,
];

const ALL_LANGUAGES: &[Language] = &[
    Language::Rust,
    Language::JavaScript,
    Language::TypeScript,
    Language::Tsx,
    Language::Json,
    Language::Toml,
    Language::Markdown,
    Language::Go,
    Language::Html,
    Language::Bash,
    Language::Yaml,
    Language::Env,
    Language::GitCommit,
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    languages: Vec<Language>,
    seed: u64,
    runs: usize,
    steps: usize,
    flags: FuzzFlags,
    help: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FuzzFlags {
    adverse: bool,
    hotspots: bool,
    windowed: bool,
    /// Continue editing after a divergence to observe whether the
    /// incremental tree converges back to fresh-parse results.
    keep_going: bool,
    /// Maintain a raw tree-sitter tree (no tree-house) fed the same edits, to
    /// attribute divergences to tree-sitter core vs tree-house layers.
    /// Opt-in: the raw tree can reach states tree-house's never does, and the
    /// tree-sitter-md block scanner then segfaults in its serialize function
    /// (buffer overflow in the C scanner) on deeply nested block state.
    raw: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            languages: DEFAULT_LANGUAGES.to_vec(),
            seed: DEFAULT_SEED,
            runs: DEFAULT_RUNS,
            steps: DEFAULT_STEPS,
            flags: FuzzFlags {
                adverse: true,
                hotspots: true,
                windowed: true,
                keep_going: false,
                raw: false,
            },
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
                "--lang" => {
                    index += 1;
                    let value = args.get(index).ok_or("--lang requires a value")?;
                    options.languages = languages_for_name(value)
                        .ok_or_else(|| format!("unknown language '{value}'"))?;
                }
                "--seed" => {
                    index += 1;
                    let value = args.get(index).ok_or("--seed requires a value")?;
                    options.seed = value
                        .parse::<u64>()
                        .map_err(|_| "--seed requires a non-negative integer".to_string())?;
                }
                "--runs" => {
                    index += 1;
                    options.runs = positive_value(args.get(index), "--runs")?;
                }
                "--steps" => {
                    index += 1;
                    options.steps = positive_value(args.get(index), "--steps")?;
                }
                "--no-adverse" => options.flags.adverse = false,
                "--no-hotspots" => options.flags.hotspots = false,
                "--no-window" => options.flags.windowed = false,
                "--keep-going" => options.flags.keep_going = true,
                "--raw" => options.flags.raw = true,
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

fn languages_for_name(name: &str) -> Option<Vec<Language>> {
    let name = name.trim().to_ascii_lowercase();
    match name.as_str() {
        "default" => return Some(DEFAULT_LANGUAGES.to_vec()),
        "all" => return Some(ALL_LANGUAGES.to_vec()),
        _ => {}
    }
    ALL_LANGUAGES
        .iter()
        .find(|language| cli_name(**language) == name)
        .map(|language| vec![*language])
}

fn cli_name(language: Language) -> &'static str {
    match language {
        Language::Rust => "rust",
        Language::JavaScript => "javascript",
        Language::TypeScript => "typescript",
        Language::Tsx => "tsx",
        Language::Json => "json",
        Language::Toml => "toml",
        Language::Markdown => "markdown",
        Language::Go => "go",
        Language::Html => "html",
        Language::Bash => "bash",
        Language::Yaml => "yaml",
        Language::Env => "env",
        Language::GitCommit => "gitcommit",
    }
}

/// Deterministic PCG-style generator; `Date`-free so runs are reproducible
/// from the seed alone.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(
            seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(0xDEAD_BEEF),
        )
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // xorshift the state so low bits are usable
        let x = self.0;
        (x ^ (x >> 33)).wrapping_mul(0xFF51_AFD7_ED55_8CCD)
    }

    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            (self.next() as usize) % bound
        }
    }

    fn chance(&mut self, one_in: usize) -> bool {
        self.below(one_in) == 0
    }
}

/// Ordinary edits an editing session produces constantly.
const PLAIN_REPLACEMENTS: &[&str] = &[
    "",
    "x",
    "value",
    "0",
    " ",
    "\n",
    "\"text\"",
    "(",
    ")",
    "{",
    "}",
    "/* note */",
    "// line\n",
    "*",
    "#",
    "- item\n",
    ":",
    "`",
    "fn f() {}",
];

/// Adversarial edits: CR/CRLF fragments and ex-line-break chars (content,
/// not breaks, since ropey is LF-only — inserting them stresses exactly
/// the chars whose meaning the LF-only decision changed), multi-byte and
/// combining characters, and tokens that flip parse structure (fences,
/// unbalanced quotes and comment delimiters).
const ADVERSE_REPLACEMENTS: &[&str] = &[
    "\r\n",
    "\r",
    "\u{2028}",
    "\u{2029}",
    "\u{000B}",
    "\u{000C}",
    "\u{0085}",
    "🦀",
    "e\u{0301}",
    "λ",
    "\t",
    "```",
    "```rust\n",
    "\"",
    "'",
    "*/",
    "/*",
    "<!--",
    "-->",
    "${",
    "r#\"",
];

/// Tokens whose surroundings are structurally load-bearing for the language;
/// uniform random positions almost never hit them.
fn hotspot_tokens(language: Language) -> &'static [&'static str] {
    match language {
        Language::Rust => &["\"", "/*", "*/", "//", "{", "}", "r#\"", "'", "!"],
        Language::Markdown => &["```", "`", "**", "*", "[", "](", "#", "rust", "json"],
        Language::JavaScript | Language::TypeScript | Language::Tsx => {
            &["`", "${", "\"", "'", "/*", "{", "<"]
        }
        Language::Yaml => &[":", "- ", "|", ">", "#", "&", "'", "\""],
        Language::Html => &["<", ">", "</", "<!--", "-->", "\""],
        _ => &["\"", "{", "#", ":", "="],
    }
}

fn template_for(language: Language) -> &'static str {
    match language {
        Language::Rust => RUST_TEMPLATE,
        Language::JavaScript => JAVASCRIPT_TEMPLATE,
        Language::TypeScript | Language::Tsx => TYPESCRIPT_TEMPLATE,
        Language::Json => JSON_TEMPLATE,
        Language::Toml => TOML_TEMPLATE,
        Language::Markdown => MARKDOWN_TEMPLATE,
        Language::Go => GO_TEMPLATE,
        Language::Html => HTML_TEMPLATE,
        Language::Bash | Language::Env => BASH_TEMPLATE,
        Language::Yaml => YAML_TEMPLATE,
        Language::GitCommit => GITCOMMIT_TEMPLATE,
    }
}

/// Initial buffer contents: the language template, in CRLF form for half
/// the draws. A real load would strip the \r (the rope is LF-only), so
/// the CRLF variant deliberately goes below the file boundary: the \r
/// chars are inline content the grammars must cope with, and edits can
/// split the pairs into lone carriage returns.
fn initial_source(language: Language, rng: &mut Rng) -> String {
    let template = template_for(language);
    if rng.chance(2) {
        template.replace('\n', "\r\n")
    } else {
        template.to_string()
    }
}

fn big_paste(language: Language, rng: &mut Rng) -> String {
    template_for(language).repeat(1 + rng.below(2))
}

fn pick_replacement(language: Language, rng: &mut Rng, flags: &FuzzFlags) -> String {
    if flags.adverse && rng.chance(24) {
        return big_paste(language, rng);
    }
    if flags.adverse {
        let total = PLAIN_REPLACEMENTS.len() + ADVERSE_REPLACEMENTS.len();
        let index = rng.below(total);
        if index < PLAIN_REPLACEMENTS.len() {
            PLAIN_REPLACEMENTS[index].to_string()
        } else {
            ADVERSE_REPLACEMENTS[index - PLAIN_REPLACEMENTS.len()].to_string()
        }
    } else {
        PLAIN_REPLACEMENTS[rng.below(PLAIN_REPLACEMENTS.len())].to_string()
    }
}

/// Char offset and char length of a randomly chosen hotspot token occurrence.
fn pick_hotspot(language: Language, buf: &Buffer, rng: &mut Rng) -> Option<(usize, usize)> {
    let text = buf.text.to_string();
    let tokens = hotspot_tokens(language);
    for _ in 0..4 {
        let token = tokens[rng.below(tokens.len())];
        let occurrences: Vec<usize> = text.match_indices(token).map(|(byte, _)| byte).collect();
        if occurrences.is_empty() {
            continue;
        }
        let byte = occurrences[rng.below(occurrences.len())];
        let start = buf.text.byte_to_char(byte);
        return Some((start, token.chars().count()));
    }
    None
}

/// Choose the next edit as (start_char, end_char, replacement).
fn choose_edit(
    language: Language,
    buf: &Buffer,
    rng: &mut Rng,
    flags: &FuzzFlags,
) -> (usize, usize, String) {
    let len = buf.text.len_chars();
    if !(MIN_BUFFER_CHARS..=MAX_BUFFER_CHARS).contains(&len) || rng.chance(64) {
        // Whole-buffer replacement: what select-all-paste produces.
        return (0, len, initial_source(language, rng));
    }
    let replacement = pick_replacement(language, rng, flags);
    if flags.hotspots && rng.chance(2) {
        if let Some((start, token_chars)) = pick_hotspot(language, buf, rng) {
            return match rng.below(3) {
                0 => (start, start + token_chars, String::new()),
                1 => (start, start, replacement),
                _ => (start, start + token_chars, replacement),
            };
        }
    }
    let start = rng.below(len + 1);
    let available = len - start;
    let delete_len = if rng.chance(16) {
        rng.below(available.min(len / 4) + 1)
    } else {
        rng.below(available.min(6) + 1)
    };
    (start, start + delete_len, replacement)
}

fn signature(spans: &[StyledSpan]) -> Vec<(usize, usize, Style)> {
    spans
        .iter()
        .map(|span| (span.start, span.end, span.style))
        .collect()
}

/// Clip spans to `range` and merge adjacent equal-style runs, so windowed
/// results computed from different padded windows compare by what a user
/// would actually see rather than by span fragmentation.
fn clipped_merged(spans: &[StyledSpan], range: &Range<usize>) -> Vec<(usize, usize, Style)> {
    let mut merged: Vec<(usize, usize, Style)> = Vec::new();
    for span in spans {
        let start = span.start.max(range.start);
        let end = span.end.min(range.end);
        if start >= end {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if last.1 == start && last.2 == span.style {
                last.1 = end;
                continue;
            }
        }
        merged.push((start, end, span.style));
    }
    merged
}

#[derive(Debug)]
struct Divergence {
    step: usize,
    /// Which comparisons differed: "window", "full-file", or both. A tree
    /// that really diverged fails full-file; a window-only failure points at
    /// the viewport query/cache path instead.
    comparison: String,
    detail: String,
    edit: Range<usize>,
    replacement: String,
    window: Option<Range<usize>>,
    source: String,
}

/// A plain tree-sitter tree for the root grammar, fed the same InputEdits as
/// the buffer, with no tree-house involvement. When the fuzz oracle fails,
/// this attributes the divergence: if the raw tree differs from a fresh raw
/// parse too, tree-sitter core's incremental reuse is responsible; if the raw
/// tree is clean, the divergence came from tree-house's layer handling.
struct RawTracker {
    grammar: Grammar,
    tree: Option<Tree>,
}

fn raw_parse(grammar: Grammar, source: RopeSlice<'_>, old: Option<&Tree>) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_grammar(grammar).ok()?;
    parser.parse_with_timeout(source, old, RAW_PARSE_TIMEOUT)
}

impl RawTracker {
    fn new(language: Language, source: RopeSlice<'_>) -> Option<Self> {
        let grammar = SyntaxState::raw_grammar(language)?;
        let tree = raw_parse(grammar, source, None);
        Some(Self { grammar, tree })
    }

    fn apply_edit(&mut self, source: RopeSlice<'_>, edit: &InputEdit) {
        if let Some(tree) = self.tree.as_mut() {
            tree.edit(edit);
        }
        self.tree = raw_parse(self.grammar, source, self.tree.as_ref());
    }

    fn diff_from_fresh(&self, source: RopeSlice<'_>) -> Option<String> {
        let incremental = self.tree.as_ref()?;
        let fresh = raw_parse(self.grammar, source, None)?;
        tree_shape_diff(incremental, &fresh)
    }
}

/// First structural difference between two trees, or None if they have the
/// same shape. Iterative so ERROR-heavy deeply nested trees cannot overflow
/// the stack.
fn tree_shape_diff(a: &Tree, b: &Tree) -> Option<String> {
    let mut stack = vec![(a.root_node(), b.root_node())];
    while let Some((x, y)) = stack.pop() {
        if x.kind_id() != y.kind_id()
            || x.byte_range() != y.byte_range()
            || x.is_missing() != y.is_missing()
            || x.child_count() != y.child_count()
        {
            return Some(format!(
                "incremental {}@{:?}{} ({} children) vs fresh {}@{:?}{} ({} children)",
                x.kind(),
                x.byte_range(),
                if x.is_missing() { " missing" } else { "" },
                x.child_count(),
                y.kind(),
                y.byte_range(),
                if y.is_missing() { " missing" } else { "" },
                y.child_count(),
            ));
        }
        for i in 0..x.child_count() {
            stack.push((x.child(i).unwrap(), y.child(i).unwrap()));
        }
    }
    None
}

fn first_diff(incremental: &[(usize, usize, Style)], fresh: &[(usize, usize, Style)]) -> String {
    let index = incremental
        .iter()
        .zip(fresh.iter())
        .position(|(a, b)| a != b)
        .unwrap_or_else(|| incremental.len().min(fresh.len()));
    format!(
        "span {index} of {}/{}: incremental {:?} vs fresh {:?}",
        incremental.len(),
        fresh.len(),
        incremental.get(index),
        fresh.get(index),
    )
}

#[derive(Debug)]
struct RunOutcome {
    language: Language,
    seed: u64,
    steps_applied: usize,
    divergent_steps: usize,
    last_divergent_step: Option<usize>,
    final_checksum: u64,
    divergence: Option<Divergence>,
}

fn text_checksum(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

/// Compare the buffer's incremental highlights against a fresh parse of the
/// same text, over a random viewport window and over the whole file.
fn compare_after_edit(
    language: Language,
    buf: &Buffer,
    rng: &mut Rng,
    flags: &FuzzFlags,
    step: usize,
    edit: &Range<usize>,
    replacement: &str,
) -> Option<Divergence> {
    let syntax = buf.syntax.as_ref().expect("fuzz buffer has syntax");
    let len_bytes = buf.text.len_bytes();
    let fresh = SyntaxState::new(language).expect("fresh syntax state");
    let mut failed = Vec::new();
    let mut detail = String::new();
    let mut window_range = None;

    // Windowed comparison first: the incremental state's cache is empty
    // right after the edit, so this exercises the padded-window compute
    // path, not just cache filtering.
    if flags.windowed && len_bytes > 0 {
        let window_start = rng.below(len_bytes);
        let window_end = (window_start + 256 + rng.below(2048)).min(len_bytes);
        let window = window_start..window_end;
        let incremental =
            syntax.highlight_rope(buf.text.slice(..), window.clone(), buf.edit_generation);
        let full = fresh.highlight_rope(buf.text.slice(..), window.clone(), 0);
        let incremental = clipped_merged(&incremental, &window);
        let full = clipped_merged(&full, &window);
        if incremental != full {
            failed.push("window");
            if !detail.is_empty() {
                detail.push_str("; ");
            }
            detail.push_str(&format!("window: {}", first_diff(&incremental, &full)));
            window_range = Some(window);
        }
    }

    let incremental = syntax.highlight_rope(buf.text.slice(..), 0..len_bytes, buf.edit_generation);
    let full = fresh.highlight_rope(buf.text.slice(..), 0..len_bytes, 0);
    let incremental = signature(&incremental);
    let full = signature(&full);
    if incremental != full {
        failed.push("full-file");
        if !detail.is_empty() {
            detail.push_str("; ");
        }
        detail.push_str(&format!("full-file: {}", first_diff(&incremental, &full)));
    }

    if failed.is_empty() {
        return None;
    }
    Some(Divergence {
        step,
        comparison: failed.join("+"),
        detail,
        edit: edit.clone(),
        replacement: replacement.to_string(),
        window: window_range,
        source: buf.text.to_string(),
    })
}

fn fuzz_run(
    language: Language,
    seed: u64,
    steps: usize,
    flags: &FuzzFlags,
) -> Result<RunOutcome, String> {
    let mut rng = Rng::new(seed ^ ((cli_name(language).len() as u64) << 32));
    let mut buf = Buffer::from_str(0, "fuzz", &initial_source(language, &mut rng));
    buf.syntax = SyntaxState::new(language);
    if buf.syntax.is_none() {
        return Err(format!("no syntax configuration for {language:?}"));
    }
    // Establish the persistent tree before editing, like an opened buffer.
    buf.syntax.as_ref().unwrap().highlight_rope(
        buf.text.slice(..),
        0..buf.text.len_bytes(),
        buf.edit_generation,
    );
    let mut raw = if flags.raw {
        RawTracker::new(language, buf.text.slice(..))
    } else {
        None
    };

    let mut divergence = None;
    let mut divergent_steps = 0;
    let mut last_divergent_step = None;
    let mut steps_applied = 0;
    for step in 0..steps {
        let (start, end, replacement) = choose_edit(language, &buf, &mut rng, flags);
        let input_edit = buf.replace(start, end, &replacement);
        if let (Some(tracker), Some(edit)) = (raw.as_mut(), input_edit) {
            tracker.apply_edit(buf.text.slice(..), &edit);
        }
        steps_applied = step + 1;
        let edit = start..end;
        if let Some(mut found) =
            compare_after_edit(language, &buf, &mut rng, flags, step, &edit, &replacement)
        {
            // Attribute the divergence: a raw tree-sitter tree fed the same
            // edits, without tree-house. Only computed on divergence — raw
            // structural differences are common upstream behavior and would
            // drown the highlight oracle if gated on directly.
            if let Some(tracker) = raw.as_ref() {
                let attribution = match tracker.diff_from_fresh(buf.text.slice(..)) {
                    Some(diff) => {
                        found.comparison.push_str("+raw-tree");
                        format!("raw-tree also diverged ({diff})")
                    }
                    None => "raw-tree clean (tree-house layer difference)".to_string(),
                };
                found.detail = format!("{attribution}; {}", found.detail);
            }
            divergent_steps += 1;
            last_divergent_step = Some(step);
            if divergence.is_none() {
                divergence = Some(found);
            }
            if !flags.keep_going {
                break;
            }
        }
    }

    Ok(RunOutcome {
        language,
        seed,
        steps_applied,
        divergent_steps,
        last_divergent_step,
        final_checksum: text_checksum(&buf.text.to_string()),
        divergence,
    })
}

fn help_text() -> &'static str {
    concat!(
        "syntax-fuzz - compare incremental parsing against fresh parses under random edits\n",
        "\n",
        "Usage: syntax-fuzz [OPTIONS]\n",
        "\n",
        "Options:\n",
        "  --lang NAME    a language name, 'default' (rust, markdown, typescript,\n",
        "                 yaml), or 'all' (default: default)\n",
        "  --seed N       first seed (default: 1)\n",
        "  --runs N       seeds per language (default: 4)\n",
        "  --steps N      edits per run (default: 250)\n",
        "  --no-adverse   only plain LF-and-ASCII edits\n",
        "  --no-hotspots  uniform random edit positions only\n",
        "  --no-window    skip windowed viewport comparisons\n",
        "  --keep-going   keep editing after a divergence, report how many\n",
        "                 steps diverged (convergence probe)\n",
        "  --raw          attribute divergences with a raw tree-sitter tree\n",
        "                 (no tree-house); caution: the tree-sitter-md block\n",
        "                 scanner can segfault on raw incremental parses\n",
        "  -h, --help     print this help\n",
    )
}

fn describe(divergence: &Divergence, outcome: &RunOutcome, flags: &FuzzFlags) -> String {
    let mut text = format!(
        "{} seed {} step {}: {} comparison diverged after replacing chars {}..{} with {:?}",
        cli_name(outcome.language),
        outcome.seed,
        divergence.step,
        divergence.comparison,
        divergence.edit.start,
        divergence.edit.end,
        divergence.replacement,
    );
    if let Some(window) = &divergence.window {
        text.push_str(&format!(" (window {}..{})", window.start, window.end));
    }
    text.push_str(&format!("\n  {}", divergence.detail));
    let source = format!("{:?}", divergence.source);
    if source.len() <= 1024 {
        text.push_str(&format!("\n  source: {source}"));
    } else {
        let mut cut = 1024;
        while !source.is_char_boundary(cut) {
            cut -= 1;
        }
        text.push_str(&format!("\n  source: {}…", &source[..cut]));
    }
    let mut repro = format!(
        "\n  reproduce: syntax-fuzz --lang {} --seed {} --steps {}",
        cli_name(outcome.language),
        outcome.seed,
        divergence.step + 1,
    );
    if !flags.adverse {
        repro.push_str(" --no-adverse");
    }
    if !flags.hotspots {
        repro.push_str(" --no-hotspots");
    }
    if !flags.windowed {
        repro.push_str(" --no-window");
    }
    text.push_str(&repro);
    text
}

pub(crate) fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let options = match Options::parse(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("syntax-fuzz: {error}");
            eprintln!("Try 'syntax-fuzz --help' for more information.");
            std::process::exit(2);
        }
    };
    if options.help {
        print!("{}", help_text());
        return;
    }
    if cfg!(debug_assertions) {
        eprintln!("warning: debug build; use --release for realistic throughput");
    }

    let mut failures = Vec::new();
    for &language in &options.languages {
        for run in 0..options.runs {
            let seed = options.seed + run as u64;
            match fuzz_run(language, seed, options.steps, &options.flags) {
                Ok(outcome) => {
                    let status = match (&outcome.divergence, outcome.last_divergent_step) {
                        (Some(divergence), Some(last)) if options.flags.keep_going => format!(
                            "DIVERGED x{} (steps {}..={})",
                            outcome.divergent_steps, divergence.step, last,
                        ),
                        (Some(_), _) => "DIVERGED".to_string(),
                        _ => "ok".to_string(),
                    };
                    println!(
                        "{:<12} seed {:>4}  {:>6} steps  {:016x}  {}",
                        cli_name(language),
                        seed,
                        outcome.steps_applied,
                        outcome.final_checksum,
                        status,
                    );
                    if outcome.divergence.is_some() {
                        failures.push((outcome, options.flags));
                    }
                }
                Err(error) => {
                    eprintln!("syntax-fuzz: {error}");
                    std::process::exit(2);
                }
            }
        }
    }

    if failures.is_empty() {
        println!("no divergence between incremental and fresh parses");
        return;
    }
    eprintln!();
    for (outcome, flags) in &failures {
        let divergence = outcome.divergence.as_ref().unwrap();
        eprintln!("{}", describe(divergence, outcome, flags));
    }
    std::process::exit(1);
}

const RUST_TEMPLATE: &str = r##"//! Fuzz template with representative constructs.
use std::collections::HashMap;

/// Computes a value from `input`.
pub fn compute(input: &str) -> usize {
    let mut map: HashMap<String, usize> = HashMap::new();
    for (index, word) in input.split_whitespace().enumerate() {
        map.insert(word.to_string(), index);
    }
    map.len()
}

macro_rules! declare {
    ($name:ident, $value:expr) => {
        fn $name() -> u32 {
            $value
        }
    };
}

declare!(alpha, 1);
declare!(beta, 2);

#[derive(Debug, Clone)]
struct Config<'a, T: Default> {
    name: &'a str,
    payload: T,
}

impl<'a, T: Default> Config<'a, T> {
    fn new(name: &'a str) -> Self {
        Self {
            name,
            payload: T::default(),
        }
    }
}

fn main() {
    let config = Config::<u32>::new("demo");
    let raw = r#"raw "quoted" text"#;
    let escaped = "line one\nline two \"quoted\"";
    let character = 'x';
    println!("{} {} {} {}", config.name, raw, escaped, character);
    /* block comment
       spanning lines */
    let result = match compute("a b c") {
        0 => alpha(),
        _ => beta(),
    };
    assert!(result > 0, "result must be positive: {result}");
}
"##;

const MARKDOWN_TEMPLATE: &str = r##"# Fuzz Document

Intro paragraph with *emphasis*, **strong**, `inline code`, and a
[link](https://example.com/page).

## Code

```rust
fn injected(count: usize) -> String {
    format!("count = {count}")
}
```

Some text between fences with `spans` and *style*.

```json
{"name": "fuzz", "values": [1, 2, 3], "nested": {"ok": true}}
```

<div class="note">an html block</div>

- list item one
- list item two with `code`
  - nested item

> Blockquote with *emphasis* and a [link](https://example.com).

1. ordered item
2. another item

Final paragraph after everything.
"##;

const TYPESCRIPT_TEMPLATE: &str = r#"interface Point {
    x: number;
    y: number;
}

type Labeled<T> = { label: string; value: T };

enum Direction {
    Up = "UP",
    Down = "DOWN",
}

export class Grid<T extends Point> {
    private items: Map<string, T> = new Map();

    add(item: T): void {
        const key = `${item.x}:${item.y}`;
        this.items.set(key, item);
    }
}

const origin: Labeled<Point> = { label: "origin", value: { x: 0, y: 0 } };

function describe(point: Point, direction: Direction): string {
    // Template literal with interpolation.
    return `at ${point.x},${point.y} going ${direction}`;
}

/* block comment
   spanning lines */
console.log(describe(origin.value, Direction.Up));
"#;

const JAVASCRIPT_TEMPLATE: &str = r#"const registry = new Map();

function register(name, handler) {
    // Stores a handler under a template-literal key.
    registry.set(`handler:${name}`, handler);
}

register("start", async (event) => {
    const payload = { kind: "start", detail: event?.detail ?? null };
    return JSON.stringify(payload);
});

/* block comment */
class Runner {
    constructor(limit = 10) {
        this.limit = limit;
    }

    run() {
        for (let i = 0; i < this.limit; i += 1) {
            console.log(`step ${i} of ${this.limit}`);
        }
    }
}

new Runner(3).run();
"#;

const JSON_TEMPLATE: &str = r#"{
    "name": "fuzz-document",
    "version": 1,
    "enabled": true,
    "threshold": 0.75,
    "tags": ["alpha", "beta", "gamma"],
    "nested": {
        "items": [
            {"id": 1, "label": "first"},
            {"id": 2, "label": "second"}
        ],
        "empty": {},
        "nothing": null
    }
}
"#;

const TOML_TEMPLATE: &str = r#"title = "fuzz document"

[package]
name = "fuzz"
version = "0.1.0"
authors = ["someone <someone@example.com>"]

[dependencies]
serde = { version = "1", features = ["derive"] }

[settings]
enabled = true
threshold = 0.75
tags = ["alpha", "beta"]

# a comment
[[profiles]]
name = "dev"
opt-level = 0
"#;

const GO_TEMPLATE: &str = r#"package main

import (
    "fmt"
    "strings"
)

type Config struct {
    Name  string
    Count int
}

func describe(config Config) string {
    // Builds a description string.
    parts := []string{config.Name, fmt.Sprintf("%d", config.Count)}
    return strings.Join(parts, ": ")
}

func main() {
    config := Config{Name: "fuzz", Count: 3}
    /* block comment */
    for i := 0; i < config.Count; i++ {
        fmt.Println(describe(config), `raw string`)
    }
}
"#;

const HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <title>Fuzz Page</title>
    <style>
        body { margin: 0; color: #333; }
    </style>
</head>
<body>
    <!-- a comment -->
    <div class="container" id="main">
        <h1>Fuzz</h1>
        <p>Paragraph with <em>emphasis</em> and <a href="https://example.com">a link</a>.</p>
        <ul>
            <li>item one</li>
            <li>item two</li>
        </ul>
    </div>
    <script>
        console.log("inline script", 1 + 2);
    </script>
</body>
</html>
"#;

const BASH_TEMPLATE: &str = r#"#!/bin/bash
set -euo pipefail

NAME="fuzz"
COUNT=3

describe() {
    local label=$1
    echo "label: ${label} name: ${NAME}"
}

# a comment
for i in $(seq 1 "$COUNT"); do
    describe "step-$i"
done

if [[ "$NAME" == "fuzz" ]]; then
    echo 'single quoted'
fi
"#;

const YAML_TEMPLATE: &str = r#"# fuzz configuration
name: fuzz-document
version: 1
defaults: &defaults
  retries: 3
  timeout: 30
service:
  <<: *defaults
  ports:
    - 8080
    - 9090
  labels: {tier: backend, env: "prod"}
description: |
  Block scalar text
  spanning lines.
summary: >
  Folded scalar
  text.
items:
  - name: 'single quoted'
    enabled: true
  - name: "double quoted"
    enabled: false
"#;

const GITCOMMIT_TEMPLATE: &str = r#"Fix incremental parsing of injected layers

The update path re-ran injection queries over stale ranges. Track the
changed ranges explicitly and only rerun the affected layers.

Fixes #42

# Please enter the commit message for your changes. Lines starting
# with '#' will be ignored, and an empty message aborts the commit.
#
# On branch main
# Changes to be committed:
#	modified:   src/syntax.rs
#	new file:   src/syntax_fuzz.rs
#
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    #[test]
    fn parses_fuzz_options() {
        let options = Options::parse(&[
            "--lang".into(),
            "markdown".into(),
            "--seed".into(),
            "9".into(),
            "--runs".into(),
            "2".into(),
            "--steps".into(),
            "33".into(),
            "--no-adverse".into(),
            "--no-hotspots".into(),
            "--no-window".into(),
            "--keep-going".into(),
            "--raw".into(),
        ])
        .unwrap();

        assert_eq!(options.languages, vec![Language::Markdown]);
        assert_eq!(options.seed, 9);
        assert_eq!(options.runs, 2);
        assert_eq!(options.steps, 33);
        assert!(!options.flags.adverse);
        assert!(!options.flags.hotspots);
        assert!(!options.flags.windowed);
        assert!(options.flags.keep_going);
        assert!(options.flags.raw);
    }

    #[test]
    fn default_options_fuzz_the_curated_languages_with_everything_on() {
        let options = Options::parse(&[]).unwrap();
        assert_eq!(options.languages, DEFAULT_LANGUAGES.to_vec());
        assert!(options.flags.adverse);
        assert!(options.flags.hotspots);
        assert!(options.flags.windowed);
    }

    #[test]
    fn rejects_bad_options() {
        assert!(Options::parse(&["--lang".into(), "klingon".into()]).is_err());
        assert!(Options::parse(&["--steps".into(), "0".into()]).is_err());
        assert!(Options::parse(&["--runs".into(), "0".into()]).is_err());
        assert!(Options::parse(&["--seed".into(), "-1".into()]).is_err());
        assert!(Options::parse(&["--frobnicate".into()]).is_err());
    }

    #[test]
    fn lang_all_selects_every_supported_language() {
        let options = Options::parse(&["--lang".into(), "all".into()]).unwrap();
        assert_eq!(options.languages, ALL_LANGUAGES.to_vec());
    }

    #[test]
    fn every_template_parses_and_highlights() {
        for &language in ALL_LANGUAGES {
            let state = SyntaxState::new(language)
                .unwrap_or_else(|| panic!("no syntax configuration for {language:?}"));
            let rope = Rope::from_str(template_for(language));
            let spans = state.highlight_rope(rope.slice(..), 0..rope.len_bytes(), 0);
            assert!(
                !spans.is_empty(),
                "template for {language:?} produced no spans"
            );
        }
    }

    #[test]
    fn clipping_merges_adjacent_equal_styles_and_drops_outside_spans() {
        let styled = Style::default().fg(ratatui::style::Color::Red);
        let spans = vec![
            StyledSpan {
                start: 0,
                end: 4,
                style: styled,
            },
            StyledSpan {
                start: 4,
                end: 8,
                style: styled,
            },
            StyledSpan {
                start: 8,
                end: 12,
                style: Style::default(),
            },
            StyledSpan {
                start: 20,
                end: 30,
                style: styled,
            },
        ];

        let merged = clipped_merged(&spans, &(2..10));
        assert_eq!(merged, vec![(2, 8, styled), (8, 10, Style::default())]);
    }

    #[test]
    fn fuzz_runs_are_deterministic() {
        let flags = FuzzFlags {
            adverse: true,
            hotspots: true,
            windowed: true,
            keep_going: false,
            raw: false,
        };
        let first = fuzz_run(Language::Rust, 7, 12, &flags).unwrap();
        let second = fuzz_run(Language::Rust, 7, 12, &flags).unwrap();

        assert_eq!(first.steps_applied, second.steps_applied);
        assert_eq!(first.final_checksum, second.final_checksum);
        assert!(first.divergence.is_none());
        assert!(second.divergence.is_none());
    }

    #[test]
    fn short_fuzz_finds_no_divergence_in_default_languages() {
        let flags = FuzzFlags {
            adverse: true,
            hotspots: true,
            windowed: true,
            keep_going: false,
            raw: false,
        };
        for &language in DEFAULT_LANGUAGES {
            let outcome = fuzz_run(language, 1, 8, &flags).unwrap();
            assert_eq!(outcome.steps_applied, 8);
            assert!(
                outcome.divergence.is_none(),
                "unexpected divergence for {language:?}: {:?}",
                outcome.divergence
            );
        }
    }
}
