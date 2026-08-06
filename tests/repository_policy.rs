use std::path::Path;

#[test]
fn cargo_builds_do_not_manage_git_hooks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        !root.join("build.rs").exists(),
        "building the editor must not install or overwrite Git hooks"
    );
    assert!(
        root.join(".githooks/pre-commit").is_file(),
        "the optional, versioned pre-commit hook should remain available"
    );
}

#[test]
fn hook_opt_in_is_documented_for_users_and_agents() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let command = "git config core.hooksPath .githooks";

    for document in ["README.md", "AGENTS.md"] {
        let contents = std::fs::read_to_string(root.join(document)).unwrap();
        assert!(
            contents.contains(command),
            "{document} must document `{command}`"
        );
    }
}

#[test]
fn rustfmt_check_is_mirrored_and_runs_before_expensive_checks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let format_command = "cargo fmt --all -- --check";

    for policy_file in [".github/workflows/ci.yml", ".githooks/pre-commit"] {
        let contents = std::fs::read_to_string(root.join(policy_file)).unwrap();
        let format_position = contents
            .find(format_command)
            .unwrap_or_else(|| panic!("{policy_file} must run `{format_command}`"));
        let build_position = contents
            .find("cargo build")
            .unwrap_or_else(|| panic!("{policy_file} must run the build check"));

        assert!(
            format_position < build_position,
            "{policy_file} must run `{format_command}` before the expensive checks"
        );
    }

    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    assert!(
        ci.contains("components: rustfmt, clippy, llvm-tools-preview"),
        "stable CI must explicitly request the rustfmt component"
    );
}
