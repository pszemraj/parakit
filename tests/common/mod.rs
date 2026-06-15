//! Shared integration-test filesystem fixtures.

use std::fs;
use std::path::{Path, PathBuf};

/// Return a clean test fixture root under `target/tmp`.
///
/// # Arguments
///
/// * `namespace` - Directory grouping for a related fixture suite.
/// * `name` - Specific fixture case name.
///
/// # Returns
///
/// A process-scoped fixture directory path. Any previous contents at that path
/// are removed.
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

/// Return a clean fixture root and create empty files relative to it.
///
/// # Arguments
///
/// * `namespace` - Directory grouping for a related fixture suite.
/// * `name` - Specific fixture case name.
/// * `files` - Relative file paths to create below the fixture root.
///
/// # Returns
///
/// The clean fixture root containing the requested files.
///
/// # Panics
///
/// Panics if any fixture file cannot be created.
pub(crate) fn fixture_root_with_files(namespace: &str, name: &str, files: &[&str]) -> PathBuf {
    let root = fixture_root(namespace, name);
    for file in files {
        touch(&root.join(file));
    }
    root
}

/// Create an empty fixture file, including parent directories.
///
/// # Arguments
///
/// * `path` - File path to create.
///
/// # Panics
///
/// Panics if the parent directory or fixture file cannot be created.
pub(crate) fn touch(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture file should have parent"))
        .expect("fixture parent should be created");
    fs::write(path, b"").expect("fixture file should be created");
}
