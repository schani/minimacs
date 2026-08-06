use std::path::Path;

fn active_command_line(contents: &str, command: &str, actions_run: bool) -> Option<usize> {
    contents
        .lines()
        .enumerate()
        .find_map(|(line_number, line)| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }

            let command_line = if actions_run {
                line.strip_prefix("run:")?.trim()
            } else {
                line
            };
            (command_line == command || command_line.starts_with(&format!("{command} ")))
                .then_some(line_number)
        })
}

fn active_yaml_list_contains(contents: &str, key: &str, expected: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return false;
        }

        line.strip_prefix(key).is_some_and(|value| {
            value
                .trim_start_matches(':')
                .split(',')
                .any(|item| item.trim() == expected)
        })
    })
}

#[test]
fn library_owns_shared_modules_and_binary_wrappers_do_not_reinclude_them() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        root.join("src/lib.rs").is_file(),
        "shared modules must have one owner in src/lib.rs"
    );

    for entry in std::fs::read_dir(root.join("src/bin")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            !contents.contains("#[path = \"../"),
            "{} must use the minimacs library instead of parent-relative module inclusion",
            path.display()
        );
    }
}

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

    for (policy_file, actions_run) in [
        (".github/workflows/ci.yml", true),
        (".githooks/pre-commit", false),
    ] {
        let contents = std::fs::read_to_string(root.join(policy_file)).unwrap();
        let format_line = active_command_line(&contents, format_command, actions_run)
            .unwrap_or_else(|| panic!("{policy_file} must actively run `{format_command}`"));
        let build_line = active_command_line(&contents, "cargo build", actions_run)
            .unwrap_or_else(|| panic!("{policy_file} must actively run the build check"));

        assert!(
            format_line < build_line,
            "{policy_file} must run `{format_command}` before the expensive checks"
        );
    }

    let ci = std::fs::read_to_string(root.join(".github/workflows/ci.yml")).unwrap();
    assert!(
        active_yaml_list_contains(&ci, "components", "rustfmt"),
        "stable CI must actively request the rustfmt component"
    );
}
