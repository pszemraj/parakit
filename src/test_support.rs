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

/// Disconnect a test child's stdout/stderr as if its terminal tab closed.
///
/// Call only in a dedicated child that exits without returning to the harness.
///
/// # Panics
///
/// Panics if the test terminal cannot be opened or does not reproduce EIO.
#[cfg(target_os = "linux")]
pub(crate) fn disconnect_test_terminal() {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty initializes both descriptor slots; optional settings are null.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
            )
        },
        0
    );
    // SAFETY: these are owned descriptors from the successful openpty call.
    unsafe {
        libc::close(master);
    }
    // Check the failure mode before attaching this terminal to a test child.
    assert_eq!(unsafe { libc::write(slave, b"x".as_ptr().cast(), 1) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EIO)
    );
    // SAFETY: only this isolated child uses its stdout/stderr descriptors.
    unsafe {
        assert_eq!(libc::dup2(slave, libc::STDOUT_FILENO), libc::STDOUT_FILENO);
        assert_eq!(libc::dup2(slave, libc::STDERR_FILENO), libc::STDERR_FILENO);
        libc::close(slave);
    }
}
