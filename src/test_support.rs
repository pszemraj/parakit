//! Shared helpers for unit-test filesystem fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Return a clean, process-scoped fixture directory under `target/tmp`.
///
/// # Arguments
///
/// * `namespace` - Directory grouping for a related fixture suite.
/// * `name` - Specific fixture case name.
///
/// # Returns
///
/// Created fixture directory path.
///
/// # Panics
///
/// Panics if the system clock is before the UNIX epoch or the fixture directory
/// cannot be created.
pub(crate) fn fixture_root(namespace: &str, name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before UNIX epoch")
        .as_nanos();
    let root = Path::new("target")
        .join("tmp")
        .join(namespace)
        .join(format!("{}-{name}-{unique}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("fixture root should be created");
    root
}
