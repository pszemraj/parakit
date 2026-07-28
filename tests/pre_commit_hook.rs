//! Repository contract tests for the checked-in pre-commit hook.
//!
//! These tests execute `.githooks/pre-commit` against scratch git
//! repositories rather than only pattern-matching its source, so a shell
//! syntax error, a dropped `set -e`, or a broken `vendor/` guard fails the
//! test suite instead of only a human reading the diff.
//!
//! Every scratch repo is created fresh under `target/tmp` (via the shared
//! [`common::fixture_root`] fixture helper, so scratch state never touches
//! the real repo or a stray `/tmp` path) and is fully isolated from the
//! host: `GIT_CONFIG_GLOBAL`/`GIT_CONFIG_SYSTEM` point at `/dev/null` so
//! `~/.gitconfig` and `/etc/gitconfig` are never read, and identity is
//! configured locally in each scratch repo. Fixtures only ever stage
//! non-Rust files, so the cargo fmt/rustdoc/clippy branch of the hook is
//! never reached and running these tests needs no cargo invocation, no
//! network access, and never touches the real repo's index.
//!
//! Gated to `cfg(unix)` because the hook is a POSIX shell script invoked
//! directly via `sh`.

#![cfg(unix)]

#[allow(dead_code)]
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Absolute path to the checked-in hook script.
fn hook_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(".githooks/pre-commit")
}

/// Fresh scratch git-repo directory for `name`, rooted under `target/tmp`
/// via the shared fixture helper (which also clears any stale directory
/// left by a previous run of the same test).
fn scratch_dir(name: &str) -> PathBuf {
    let dir = common::fixture_root("pre-commit-hook", name);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

/// Disable every git config source but the scratch repo's own local config:
/// no global, no system, so the host's identity/settings can never leak in.
fn hermetic_git_env(cmd: &mut Command) -> &mut Command {
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
}

/// Run a git command in `dir` and assert it succeeded.
fn git(dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    hermetic_git_env(&mut cmd);
    let status = cmd
        .status()
        .unwrap_or_else(|err| panic!("spawn git {args:?}: {err}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// Initialize an empty repo at `dir` with local-only identity.
fn init_repo(dir: &Path) {
    git(dir, &["init", "-q", "."]);
    git(dir, &["config", "user.name", "pre-commit-hook-test"]);
    git(
        dir,
        &[
            "config",
            "user.email",
            "pre-commit-hook-test@example.invalid",
        ],
    );
}

/// Write `contents` to `relative` inside `dir` and stage it.
fn stage_file(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir for staged file");
    }
    fs::write(&path, contents).expect("write staged file");
    git(dir, &["add", "--", relative]);
}

/// Run the checked-in hook script with `dir` as its working directory, under
/// the same git isolation as [`git`], inheriting the test process's `PATH`
/// so `sh`/`git`/`grep` (and `cargo`, on the rust-file branch) resolve
/// normally.
fn run_hook(dir: &Path) -> Output {
    let mut cmd = Command::new("sh");
    cmd.arg(hook_path()).current_dir(dir);
    hermetic_git_env(&mut cmd);
    cmd.output().expect("spawn pre-commit hook")
}

/// Stdout and stderr concatenated, for substring assertions against the
/// hook's `say`/`fail` messages and git's own diagnostics.
fn combined_output(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn staged_file_scan_includes_deletions() {
    let hook = fs::read_to_string(hook_path()).expect("pre-commit hook should be readable");
    let staged_scan = hook
        .lines()
        .find(|line| line.contains("git diff --cached --name-only"))
        .expect("pre-commit hook should scan staged file names");

    assert!(
        staged_scan.contains("--diff-filter=ACMRD"),
        "staged scan must include deletions so deletion-only commits are validated: {staged_scan}"
    );
}

/// Non-Rust staged files must pass, and must never reach the cargo branch:
/// the fixture stages only a markdown file, so `cargo fmt`/`rustdoc-checker`/
/// `cargo clippy` are never invoked. This keeps the test hermetic (no cargo
/// call, no network) and exercises the "docs-only commits stay fast" design
/// intent documented at the top of the hook.
#[test]
fn passes_on_non_rust_staged_files() {
    let dir = scratch_dir("pass-case");
    init_repo(&dir);
    stage_file(
        &dir,
        "NOTES.md",
        "# scratch fixture\n\nplain text change, no Rust files staged.\n",
    );

    let output = run_hook(&dir);
    let text = combined_output(&output);

    assert!(
        output.status.success(),
        "hook should pass for a docs-only staged change: {text}"
    );
    assert!(
        !text.contains("cargo"),
        "docs-only commit must not reach the cargo fmt/clippy branch: {text}"
    );
}

/// A staged conflict marker must be caught by `git diff --cached --check`
/// and block the commit before any cargo check runs.
#[test]
fn blocks_staged_conflict_markers() {
    let dir = scratch_dir("conflict-marker-case");
    init_repo(&dir);
    stage_file(
        &dir,
        "conflict.txt",
        "<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n",
    );

    let output = run_hook(&dir);
    let text = combined_output(&output);

    assert!(
        !output.status.success(),
        "hook should reject a staged conflict marker: {text}"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "hook fails via its fail() helper, which always exits 1: {text}"
    );
    assert!(
        text.contains("whitespace errors or conflict markers"),
        "expected the whitespace/conflict-marker failure message: {text}"
    );
}

/// A staged submodule pointer bump under `vendor/` must be blocked by the
/// vendor guard, ahead of both the conflict-marker check and any cargo
/// check. The gitlink is created directly in the index via
/// `git update-index --cacheinfo`, which reproduces a submodule pointer
/// bump (a `160000` mode entry under `vendor/CrispASR`) without checking out
/// a real submodule, keeping the fixture hermetic and network-free.
#[test]
fn blocks_staged_vendor_gitlink() {
    let dir = scratch_dir("vendor-gitlink-case");
    init_repo(&dir);
    git(
        &dir,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000,1111111111111111111111111111111111111111,vendor/CrispASR",
        ],
    );

    let output = run_hook(&dir);
    let text = combined_output(&output);

    assert!(
        !output.status.success(),
        "hook should reject a staged vendor/ pointer bump: {text}"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "hook fails via its fail() helper, which always exits 1: {text}"
    );
    assert!(
        text.contains("staged change under vendor/"),
        "expected the vendor guard failure message: {text}"
    );
}
