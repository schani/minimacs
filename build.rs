use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const HOOK: &str = r#"#!/bin/sh
# Managed by build.rs — edits here are overwritten on the next build.
set -e
cargo build 2>&1
if command -v cargo-llvm-cov >/dev/null 2>&1; then
    cargo llvm-cov --fail-under-lines 90 --summary-only 2>&1
else
    echo "" >&2
    echo "WARNING: cargo-llvm-cov is not installed; the 90% line-coverage" >&2
    echo "threshold is NOT being enforced. Install it with:" >&2
    echo "    cargo install cargo-llvm-cov" >&2
    echo "" >&2
    cargo test 2>&1
fi
cargo clippy -- -D warnings 2>&1
"#;

fn main() {
    let hook_path = Path::new(".git/hooks/pre-commit");
    if !Path::new(".git/hooks").is_dir() {
        return;
    }
    // Rewrite the hook whenever it differs, so updates here propagate.
    if fs::read_to_string(hook_path).is_ok_and(|current| current == HOOK) {
        return;
    }

    if fs::write(hook_path, HOOK).is_ok() {
        let _ = fs::set_permissions(hook_path, fs::Permissions::from_mode(0o755));
    }
}
