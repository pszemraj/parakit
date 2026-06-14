//! Shared integration-test filesystem fixtures.

use std::fs;
use std::path::{Path, PathBuf};

/// Return a clean test fixture root under `target/tmp`.
///
/// # Panics
///
/// Does not panic.
pub(crate) fn fixture_root(namespace: &str, name: &str) -> PathBuf {
    let root = Path::new("target")
        .join("tmp")
        .join(namespace)
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

/// Create an empty fixture file, including parent directories.
///
/// # Panics
///
/// Panics if the parent directory or fixture file cannot be created.
pub(crate) fn touch(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture file should have parent"))
        .expect("fixture parent should be created");
    fs::write(path, b"").expect("fixture file should be created");
}
