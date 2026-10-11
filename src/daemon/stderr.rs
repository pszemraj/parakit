//! Native stderr suppression for noisy C/C++ calls with structured Rust output.

use std::io::{self, BufRead, BufReader, Read, Write};

const STDERR_FD: libc::c_int = 2;

/// Run a closure while temporarily suppressing native stderr.
///
/// # Arguments
///
/// * `f` - Closure to execute while stderr is redirected.
///
/// # Returns
///
/// The closure return value. If stderr cannot be redirected, the closure still
/// runs normally.
pub(crate) fn with_stderr_suppressed<T>(f: impl FnOnce() -> T) -> T {
    with_stderr_redirected(false, f)
}

/// Remove Parakeet's two informational loader lines while retaining diagnostics.
///
/// # Arguments
///
/// * `f` - Model open operation, which may run beside capture and IPC.
///
/// # Returns
///
/// The closure return value. If stderr cannot be redirected, the closure still
/// runs normally. Other stderr, including warnings and errors, is forwarded.
pub(crate) fn with_parakeet_info_suppressed<T>(f: impl FnOnce() -> T) -> T {
    with_stderr_redirected(true, f)
}

fn with_stderr_redirected<T>(keep_diagnostics: bool, f: impl FnOnce() -> T) -> T {
    struct RestoreStderr {
        saved: libc::c_int,
        drain: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved, STDERR_FD);
            }
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
            // The drain writes through this descriptor, so close it after EOF.
            unsafe {
                libc::close(self.saved);
            }
        }
    }

    let mut pipe_fds = [0; 2];
    #[cfg(unix)]
    let opened = unsafe { libc::pipe(pipe_fds.as_mut_ptr()) };
    #[cfg(windows)]
    let opened = unsafe { libc::pipe(pipe_fds.as_mut_ptr(), 8192, libc::O_BINARY) };
    if opened != 0 {
        return f();
    }
    let read_fd = pipe_fds[0];
    let write_fd = pipe_fds[1];
    let saved = unsafe { libc::dup(STDERR_FD) };
    if saved < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return f();
    }
    // POSIX dup2 returns the destination fd; Windows _dup2 returns zero.
    if unsafe { libc::dup2(write_fd, STDERR_FD) } < 0 {
        unsafe {
            libc::close(saved);
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return f();
    }
    unsafe {
        libc::close(write_fd);
    }

    let drain = std::thread::spawn(move || {
        let mut reader = BufReader::new(PipeReader(read_fd));
        if keep_diagnostics {
            // Do not acquire Rust's stderr lock: a concurrent producer can hold
            // it while filling the pipe. Write to the saved descriptor directly.
            let _ = forward_diagnostics(&mut reader, &mut StderrWriter(saved));
            // A closed terminal can reject forwarded diagnostics. Continue
            // draining so native writers do not get EPIPE or fill the pipe.
            let _ = io::copy(&mut reader, &mut io::sink());
        } else {
            let _ = io::copy(&mut reader, &mut io::sink());
        }
    });
    let _restore = RestoreStderr {
        saved,
        drain: Some(drain),
    };
    f()
}

struct PipeReader(libc::c_int);

impl Read for PipeReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = unsafe { libc::read(self.0, buf.as_mut_ptr().cast(), buf.len() as _) };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.0);
        }
    }
}

struct StderrWriter(libc::c_int);

impl Write for StderrWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let count = unsafe { libc::write(self.0, buf.as_ptr().cast(), buf.len() as _) };
        if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(count as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn forward_diagnostics(reader: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
    use regex::bytes::Regex;
    use std::sync::OnceLock;

    static LOADER_INFO: OnceLock<Regex> = OnceLock::new();
    let info = LOADER_INFO.get_or_init(|| {
        // Match the complete native fprintf formats, not their prefixes: a
        // warning that includes a model field or layer count must stay visible.
        Regex::new(concat!(
            r"\Aparakeet: (?:vocab=[0-9]+  d_model=[0-9]+  n_layers=[0-9]+  n_heads=[0-9]+  ff=[0-9]+  pred=[0-9]+  joint=[0-9]+|",
            r"BN folded into conv_dw weights for [0-9]+ layers)\r?\n\z"
        ))
        .expect("native loader info regex must compile")
    });
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        if !info.is_match(&line) {
            output.write_all(&line)?;
        }
        line.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_preserves_warnings_errors_and_incomplete_or_non_utf8_output() {
        let input = concat!(
            "parakeet: vocab=8192  d_model=1024  n_layers=24  n_heads=8  ff=4096  pred=640  joint=640\n",
            "parakeet: BN folded into conv_dw weights for 24 layers\r\n",
            "parakeet: warning: model may be incompatible\n",
            "parakeet: BN folded into conv_dw weights for 24 layers: warning\n",
            "parakit: error: capture failed\n"
        );
        let mut bytes = input.as_bytes().to_vec();
        bytes.extend_from_slice(b"native error: \xff\npartial diagnostic");
        let mut forwarded = Vec::new();
        forward_diagnostics(&mut bytes.as_slice(), &mut forwarded).unwrap();
        assert_eq!(
            forwarded,
            b"parakeet: warning: model may be incompatible\nparakeet: BN folded into conv_dw weights for 24 layers: warning\nparakit: error: capture failed\nnative error: \xff\npartial diagnostic"
        );
    }

    #[test]
    fn reload_filter_preserves_concurrent_diagnostics_and_restores_stderr() {
        const CHILD: &str = "PARAKIT_STDERR_FILTER_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let closed_terminal = std::env::var(CHILD).unwrap() == "closed";
            #[cfg(target_os = "linux")]
            if closed_terminal {
                crate::test_support::disconnect_test_terminal();
            }
            with_stderr_suppressed(|| {
                StderrWriter(STDERR_FD)
                    .write_all(b"startup noise\n")
                    .unwrap();
            });
            with_parakeet_info_suppressed(|| {
                let mut native = StderrWriter(STDERR_FD);
                native
                    .write_all(b"parakeet: BN folded into conv_dw ")
                    .unwrap();
                native.write_all(b"weights for 24 layers\n").unwrap();
                std::thread::spawn(|| {
                    for _ in 0..512 {
                        eprintln!("parakit: warning: capture remains active during reload");
                    }
                })
                .join()
                .unwrap();
                native.write_all(b"native error: loader failed\n").unwrap();
            });
            if closed_terminal {
                assert_eq!(
                    StderrWriter(STDERR_FD)
                        .write_all(b"stderr restored\n")
                        .unwrap_err()
                        .raw_os_error(),
                    Some(libc::EIO)
                );
                std::process::exit(0);
            }
            eprintln!("stderr restored");
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::stderr::tests::reload_filter_preserves_concurrent_diagnostics_and_restores_stderr",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(!stderr.contains("parakeet:"));
        assert!(!stderr.contains("startup noise"));
        assert_eq!(
            stderr
                .matches("capture remains active during reload")
                .count(),
            512
        );
        assert!(stderr
            .lines()
            .any(|line| line == "native error: loader failed"));
        assert!(stderr.ends_with("stderr restored\n") || stderr.ends_with("stderr restored\r\n"));

        #[cfg(target_os = "linux")]
        {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "daemon::stderr::tests::reload_filter_preserves_concurrent_diagnostics_and_restores_stderr",
                    "--nocapture",
                ])
                .env(CHILD, "closed")
                .output()
                .unwrap()
                .status;
            assert!(
                status.success(),
                "closed-terminal stderr filter exited: {status}"
            );
        }
    }
}
