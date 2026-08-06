use std::path::{Component, Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{Attribute, Expr, ItemMod, Lit, Macro, Meta};

#[derive(Debug, Default, PartialEq, Eq)]
struct BinarySourceViolations {
    module_declarations: Vec<String>,
    parent_relative_paths: Vec<String>,
    include_macros: Vec<String>,
}

impl BinarySourceViolations {
    fn is_empty(&self) -> bool {
        self.module_declarations.is_empty()
            && self.parent_relative_paths.is_empty()
            && self.include_macros.is_empty()
    }
}

impl<'ast> Visit<'ast> for BinarySourceViolations {
    fn visit_item_mod(&mut self, module: &'ast ItemMod) {
        self.module_declarations.push(module.ident.to_string());
        visit::visit_item_mod(self, module);
    }

    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        if attribute.path().is_ident("path") {
            if let Meta::NameValue(name_value) = &attribute.meta {
                if let Expr::Lit(expression) = &name_value.value {
                    if let Lit::Str(path) = &expression.lit {
                        let path = path.value();
                        if Path::new(&path)
                            .components()
                            .any(|component| component == Component::ParentDir)
                        {
                            self.parent_relative_paths.push(path);
                        }
                    }
                }
            }
        }
        visit::visit_attribute(self, attribute);
    }

    fn visit_macro(&mut self, macro_call: &'ast Macro) {
        if let Some(name) = macro_call
            .path
            .segments
            .last()
            .map(|segment| &segment.ident)
        {
            if matches!(
                name.to_string().as_str(),
                "include" | "include_str" | "include_bytes"
            ) {
                self.include_macros.push(name.to_string());
            }
        }
        visit::visit_macro(self, macro_call);
    }
}

fn binary_source_violations(contents: &str) -> syn::Result<BinarySourceViolations> {
    let file = syn::parse_file(contents)?;
    let mut violations = BinarySourceViolations::default();
    violations.visit_file(&file);
    Ok(violations)
}

fn binary_sources(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.join("src/main.rs")];
    paths.extend(
        std::fs::read_dir(root.join("src/bin"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("rs")),
    );
    paths.sort();
    paths
}

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
fn binary_source_policy_detects_duplicate_ownership_without_reading_comments() {
    let old_wrapper = r###"
        mod app;
        # [ path = r#"../syntax_fuzz.rs"# ]
        mod syntax_fuzz;
        const SOURCE: &str = include_str ! ("../syntax_fuzz.rs");
        const BYTES: &[u8] = include_bytes!("../syntax_fuzz.rs");
        include ! ("../runtime.rs");
    "###;
    let violations = binary_source_violations(old_wrapper).unwrap();
    assert_eq!(violations.module_declarations, ["app", "syntax_fuzz"]);
    assert_eq!(violations.parent_relative_paths, ["../syntax_fuzz.rs"]);
    assert_eq!(
        violations.include_macros,
        ["include_str", "include_bytes", "include"]
    );

    let comments_and_strings = r###"
        // mod app;
        /*
            # [ path = "../syntax_fuzz.rs" ]
            mod syntax_fuzz;
            include!("../runtime.rs");
        */
        const DESCRIPTION: &str = "mod buffer; include!(\"../buffer.rs\");";
        fn main() {}
    "###;
    assert!(binary_source_violations(comments_and_strings)
        .unwrap()
        .is_empty());
}

#[test]
fn library_owns_shared_modules_and_binary_wrappers_do_not_reinclude_them() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        root.join("src/lib.rs").is_file(),
        "shared modules must have one owner in src/lib.rs"
    );

    for path in binary_sources(root) {
        let contents = std::fs::read_to_string(&path).unwrap();
        let violations = binary_source_violations(&contents)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        assert!(
            violations.is_empty(),
            "{} must use the minimacs library instead of owning or including shared source; \
             found {violations:?}",
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
