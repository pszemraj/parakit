//! Serialized native stderr redirection for noisy C/C++ calls.

use std::io::{self, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const STDERR_FD: libc::c_int = 2;
const DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
static REDIRECT: Mutex<()> = Mutex::new(());

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
    let captured = Arc::new(Mutex::new(Vec::new()));
    let drain_capture = Arc::clone(&captured);
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        with_stderr_filtered(f, move |mut reader, _writer| {
            let mut bytes = Vec::new();
            let _ = reader.read_to_end(&mut bytes);
            *drain_capture
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = bytes;
        })
    }));
    match result {
        Ok(value) => value,
        Err(payload) => {
            let bytes = captured.lock().unwrap_or_else(|error| error.into_inner());
            let _ = io::stderr().write_all(&bytes);
            panic::resume_unwind(payload)
        }
    }
}

/// Run a closure while a background filter owns native stderr output.
///
/// Redirection is process-global, so suppression and forwarding filters share
/// one lock. The filter receives the pipe reader and a duplicate of the real
/// stderr descriptor. If setup fails, `f` still runs with stderr unchanged.
///
/// # Arguments
///
/// * `f` - Closure to execute while stderr is redirected.
/// * `filter` - Background pipe consumer that decides which bytes to forward.
///
/// # Returns
///
/// The closure return value, whether redirection succeeds or falls back to the
/// unchanged stderr stream.
pub(crate) fn with_stderr_filtered<T>(
    f: impl FnOnce() -> T,
    filter: impl FnOnce(Box<dyn Read + Send>, Box<dyn Write + Send>) + Send + 'static,
) -> T {
    let _lock = REDIRECT.lock().unwrap_or_else(|err| err.into_inner());
    let Some(mut redirect) = Redirect::new(filter) else {
        return f();
    };
    let value = f();
    redirect.finish();
    value
}

struct Fd(libc::c_int);

impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

impl Read for Fd {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let size = buffer.len().min(i32::MAX as usize);
        let result = unsafe { libc::read(self.0, buffer.as_mut_ptr().cast(), size as _) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }
}

impl Write for Fd {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let size = buffer.len().min(i32::MAX as usize);
        let result = unsafe { libc::write(self.0, buffer.as_ptr().cast(), size as _) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Redirect {
    saved: Fd,
    drain: Option<std::thread::JoinHandle<()>>,
    drained: mpsc::Receiver<()>,
    finished: bool,
}

impl Redirect {
    fn new(
        filter: impl FnOnce(Box<dyn Read + Send>, Box<dyn Write + Send>) + Send + 'static,
    ) -> Option<Self> {
        let mut pipe = [-1; 2];
        #[cfg(unix)]
        let result = unsafe { libc::pipe(pipe.as_mut_ptr()) };
        #[cfg(windows)]
        let result =
            unsafe { libc::pipe(pipe.as_mut_ptr(), 8192, libc::O_BINARY | libc::O_NOINHERIT) };
        if result < 0 {
            return None;
        }
        let read = Fd(pipe[0]);
        let write = Fd(pipe[1]);
        if !set_close_on_exec(read.0) || !set_close_on_exec(write.0) {
            return None;
        }
        let saved = unsafe { libc::dup(STDERR_FD) };
        if saved < 0 {
            return None;
        }
        let saved = Fd(saved);
        if !set_close_on_exec(saved.0) {
            return None;
        }
        let forward = unsafe { libc::dup(saved.0) };
        if forward < 0 {
            return None;
        }
        let forward = Fd(forward);
        if !set_close_on_exec(forward.0) {
            return None;
        }
        // Spawn before redirecting: thread creation failure must not strand stderr.
        let (drained_tx, drained) = mpsc::sync_channel(1);
        let drain = std::thread::Builder::new()
            .name("parakit-stderr".into())
            .spawn(move || {
                filter(Box::new(read), Box::new(forward));
                let _ = drained_tx.send(());
            })
            .ok()?;
        if duplicate_to(write.0, STDERR_FD).is_err() {
            drop(write);
            let _ = drain.join();
            return None;
        }
        drop(write);
        Some(Self {
            saved,
            drain: Some(drain),
            drained,
            finished: false,
        })
    }

    fn finish(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let restore = duplicate_to(self.saved.0, STDERR_FD);
        let drained = self.drained.recv_timeout(DRAIN_TIMEOUT);
        if matches!(drained, Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected)) {
            if let Some(drain) = self.drain.take() {
                let _ = drain.join();
            }
        } else {
            // A subprocess may legitimately inherit redirected stderr. Do not
            // let that unrelated process hold model loading or shutdown open.
            self.drain.take();
        }
        if let Err(error) = restore {
            let _ = writeln!(
                self.saved,
                "parakit: could not restore native stderr: {error}"
            );
        }
    }
}

impl Drop for Redirect {
    fn drop(&mut self) {
        self.finish();
    }
}

fn duplicate_to(source: libc::c_int, target: libc::c_int) -> io::Result<()> {
    loop {
        if unsafe { libc::dup2(source, target) } >= 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(unix)]
fn set_close_on_exec(fd: libc::c_int) -> bool {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } >= 0
}

#[cfg(windows)]
fn set_close_on_exec(fd: libc::c_int) -> bool {
    const HANDLE_FLAG_INHERIT: u32 = 1;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }

    let handle = unsafe { libc::_get_osfhandle(fd) };
    handle != -1
        && unsafe { SetHandleInformation(handle as *mut std::ffi::c_void, HANDLE_FLAG_INHERIT, 0) }
            != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_redirect_delivers_native_stderr_to_filter() {
        let (tx, rx) = std::sync::mpsc::channel();
        with_stderr_filtered(
            || {
                let message = b"parakit-stderr-redirect-test\n";
                let written =
                    unsafe { libc::write(STDERR_FD, message.as_ptr().cast(), message.len() as _) };
                assert_eq!(usize::try_from(written).unwrap(), message.len());
            },
            move |mut reader, _writer| {
                let mut captured = Vec::new();
                reader.read_to_end(&mut captured).unwrap();
                tx.send(captured).unwrap();
            },
        );
        let captured = rx.recv().unwrap();
        assert!(captured
            .windows(b"parakit-stderr-redirect-test\n".len())
            .any(|window| window == b"parakit-stderr-redirect-test\n"));
    }

    #[cfg(unix)]
    #[test]
    fn inherited_stderr_does_not_block_redirect_completion() {
        let started = std::time::Instant::now();
        let mut child = with_stderr_filtered(
            || {
                std::process::Command::new("sleep")
                    .arg("2")
                    .spawn()
                    .unwrap()
            },
            |mut reader, _writer| {
                let _ = io::copy(&mut reader, &mut io::sink());
            },
        );

        assert!(started.elapsed() < Duration::from_secs(1));
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn suppressed_panic_is_replayed_after_stderr_restoration() {
        const CHILD: &str = "PARAKIT_STDERR_PANIC_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let result = panic::catch_unwind(|| {
                with_stderr_suppressed(|| panic!("suppressed panic remains visible"));
            });
            assert!(result.is_err());
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::stderr::tests::suppressed_panic_is_replayed_after_stderr_restoration",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("suppressed panic remains visible")
        );
    }
}
