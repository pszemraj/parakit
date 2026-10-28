//! Shared helpers for unit-test filesystem fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// Write `config.toml` in a clean fixture directory and return its path.
///
/// # Arguments
///
/// * `namespace` - Directory grouping for a related fixture suite.
/// * `name` - Specific fixture case name.
/// * `contents` - TOML text to write to the fixture config.
///
/// # Returns
///
/// Created fixture config path.
///
/// # Panics
///
/// Panics if the fixture directory or config file cannot be created.
#[allow(
    dead_code,
    reason = "configuration fixtures belong to binary tests; this module also serves library tests"
)]
pub(crate) fn write_config_fixture(namespace: &str, name: &str, contents: &str) -> PathBuf {
    let path = fixture_root(namespace, name).join("config.toml");
    fs::write(&path, contents).expect("write fixture config file");
    path
}

/// Write a script fixture and wait for Unix executable-file locks to clear.
///
/// Unix scripts must exit promptly without side effects when run without
/// arguments. The probe accepts any exit status and retries only
/// `ExecutableFileBusy`, for at most one second. Windows only needs the write.
///
/// # Arguments
///
/// * `path` - Fixture script path in an existing directory.
/// * `contents` - Complete shell script or Windows batch-file contents.
///
/// # Panics
///
/// Panics if writing, setting permissions, or probing the script fails.
pub(crate) fn write_script_fixture(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write script fixture");
    #[cfg(unix)]
    {
        use std::io::ErrorKind;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("make script fixture executable");
        // Concurrent process creation can briefly inherit the writable file
        // descriptor even after this thread has closed it.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match Command::new(path).output() {
                Ok(_) => break,
                Err(err)
                    if err.kind() == ErrorKind::ExecutableFileBusy && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(err) => panic!("probe script fixture {}: {err}", path.display()),
            }
        }
    }
}

#[test]
fn executable_fixture_preserves_script_behavior() {
    let root = fixture_root("script-fixture-tests", "executable");
    let path = root.join(if cfg!(windows) { "probe.cmd" } else { "probe" });
    let script = if cfg!(windows) {
        "@echo off\r\necho %~1\r\nexit /b 7\r\n"
    } else {
        "#!/bin/sh\n[ \"$#\" -gt 0 ] || exit 7\nprintf '%s' \"$1\"\nexit 7\n"
    };
    write_script_fixture(&path, script);

    #[cfg(target_os = "linux")]
    {
        use std::io::ErrorKind;
        use std::sync::mpsc;
        use std::time::Duration;

        let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
        assert_eq!(
            Command::new(&path).output().unwrap_err().kind(),
            ErrorKind::ExecutableFileBusy
        );
        std::thread::scope(|scope| {
            let (done_tx, done_rx) = mpsc::channel();
            let script_path = &path;
            scope.spawn(move || {
                write_script_fixture(script_path, script);
                done_tx.send(()).unwrap();
            });
            let before_close = done_rx.recv_timeout(Duration::from_millis(30));
            drop(writer);
            assert_eq!(before_close, Err(mpsc::RecvTimeoutError::Timeout));
            done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        });
    }

    let output = Command::new(&path).arg("payload").output().unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "payload");
    assert!(output.stderr.is_empty());
    fs::remove_dir_all(root).unwrap();
}

/// Build a command that reruns one test in an isolated child process.
///
/// # Arguments
///
/// * `name` - Fully qualified test name in the current test binary.
///
/// # Returns
///
/// A command selecting that test with output capture disabled.
///
/// # Panics
///
/// Panics if the current test executable cannot be located.
pub(crate) fn exact_test_command(name: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", name, "--nocapture"]);
    command
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
