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
fn native_macos_frontend_sources_are_versioned() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));

    for path in [
        "macos/MinimacsApp.swift",
        "macos/Info.plist",
        "macos/build.sh",
        "macos/capture-ui.sh",
        "macos/include/minimacs_native.h",
    ] {
        assert!(root.join(path).is_file(), "missing native frontend file: {path}");
    }
}

#[test]
fn native_bundle_registers_only_text_documents() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let plist = std::fs::read_to_string(root.join("macos/Info.plist")).unwrap();

    assert!(plist.contains("public.text"));
    assert!(plist.contains("public.source-code"));
    assert!(
        !plist.contains("public.data"),
        "the UTF-8-only editor must not register itself for arbitrary binary data"
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
