use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn main() {
    let hook_path = Path::new(".git/hooks/pre-commit");
    if hook_path.exists() {
        return;
    }
    if !Path::new(".git/hooks").is_dir() {
        return;
    }

    let hook = r#"#!/bin/sh
set -e
cargo build 2>&1
cargo test 2>&1
cargo clippy -- -D warnings 2>&1
"#;

    if fs::write(hook_path, hook).is_ok() {
        let _ = fs::set_permissions(hook_path, fs::Permissions::from_mode(0o755));
    }
}
