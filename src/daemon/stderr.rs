//! Native stderr suppression for noisy C/C++ calls with structured Rust output.

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
    with_stderr_suppressed_impl(f)
}

#[cfg(unix)]
fn with_stderr_suppressed_impl<T>(f: impl FnOnce() -> T) -> T {
    use std::fs::File;
    use std::io::Read;
    use std::os::fd::FromRawFd;

    struct RestoreStderr {
        saved: i32,
        drain: Option<std::thread::JoinHandle<()>>,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved, libc::STDERR_FILENO);
                libc::close(self.saved);
            }
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
        }
    }

    let mut pipe_fds = [0_i32; 2];
    unsafe {
        if libc::pipe(pipe_fds.as_mut_ptr()) != 0 {
            return f();
        }
    }
    let read_fd = pipe_fds[0];
    let write_fd = pipe_fds[1];
    let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
    if saved < 0 {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
        return f();
    }
    if unsafe { libc::dup2(write_fd, libc::STDERR_FILENO) } < 0 {
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

    let drain = std::thread::spawn(move || unsafe {
        let mut file = File::from_raw_fd(read_fd);
        let mut buf = [0_u8; 8192];
        while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
    });
    let _restore = RestoreStderr {
        saved,
        drain: Some(drain),
    };
    f()
}

#[cfg(windows)]
const STDERR_FD: libc::c_int = 2;

#[cfg(windows)]
fn with_stderr_suppressed_impl<T>(f: impl FnOnce() -> T) -> T {
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    struct RestoreStderr {
        saved_fd: libc::c_int,
        nul_fd: libc::c_int,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved_fd, STDERR_FD);
                libc::close(self.saved_fd);
                libc::close(self.nul_fd);
            }
        }
    }

    let Ok(nul_file) = std::fs::OpenOptions::new().write(true).open("NUL") else {
        return f();
    };
    let nul_handle = nul_file.into_raw_handle();
    let nul_fd = unsafe { libc::open_osfhandle(nul_handle as isize, 0) };
    if nul_fd < 0 {
        unsafe {
            drop(std::fs::File::from_raw_handle(nul_handle));
        }
        return f();
    }

    let saved_fd = unsafe { libc::dup(STDERR_FD) };
    if saved_fd < 0 {
        unsafe {
            libc::close(nul_fd);
        }
        return f();
    }

    // MSVCRT _dup2 returns 0 on success, while POSIX dup2 returns the
    // destination fd. Both report failure as -1, so check the failure value.
    if unsafe { libc::dup2(nul_fd, STDERR_FD) } == -1 {
        unsafe {
            libc::close(saved_fd);
            libc::close(nul_fd);
        }
        return f();
    }

    let _restore = RestoreStderr { saved_fd, nul_fd };
    f()
}

#[cfg(not(any(unix, windows)))]
fn with_stderr_suppressed_impl<T>(f: impl FnOnce() -> T) -> T {
    f()
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::STDERR_FD;
    use std::os::windows::io::{FromRawHandle, IntoRawHandle};

    struct RestoreStderr {
        saved_fd: libc::c_int,
        nul_fd: libc::c_int,
    }

    impl Drop for RestoreStderr {
        fn drop(&mut self) {
            unsafe {
                libc::dup2(self.saved_fd, STDERR_FD);
                libc::close(self.saved_fd);
                libc::close(self.nul_fd);
            }
        }
    }

    #[test]
    fn windows_crt_dup2_reports_zero_on_success() {
        let nul_file = std::fs::OpenOptions::new()
            .write(true)
            .open("NUL")
            .expect("open NUL");
        let nul_handle = nul_file.into_raw_handle();
        let nul_fd = unsafe { libc::open_osfhandle(nul_handle as isize, 0) };
        if nul_fd < 0 {
            unsafe {
                drop(std::fs::File::from_raw_handle(nul_handle));
            }
            panic!("open_osfhandle failed");
        }

        let saved_fd = unsafe { libc::dup(STDERR_FD) };
        assert!(saved_fd >= 0, "dup stderr failed");
        let _restore = RestoreStderr { saved_fd, nul_fd };

        let result = unsafe { libc::dup2(nul_fd, STDERR_FD) };
        assert_eq!(result, 0);
    }
}
