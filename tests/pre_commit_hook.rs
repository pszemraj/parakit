//! Repository contract tests for the checked-in pre-commit hook.

use std::fs;
use std::path::Path;

#[test]
fn staged_file_scan_includes_deletions() {
    let hook =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(".githooks/pre-commit"))
            .expect("pre-commit hook should be readable");
    let staged_scan = hook
        .lines()
        .find(|line| line.contains("git diff --cached --name-only"))
        .expect("pre-commit hook should scan staged file names");

    assert!(
        staged_scan.contains("--diff-filter=ACMRD"),
        "staged scan must include deletions so deletion-only commits are validated: {staged_scan}"
    );
}
