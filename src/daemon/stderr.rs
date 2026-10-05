//! Serialized native stderr redirection for noisy C/C++ calls.

use std::io::{self, Read, Write};
use std::sync::Mutex;

const STDERR_FD: libc::c_int = 2;
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
    with_stderr_filtered(f, |mut reader, _writer| {
        let _ = io::copy(&mut reader, &mut io::sink());
    })
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
    let Some(_redirect) = Redirect::new(filter) else {
        return f();
    };
    f()
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
}

impl Redirect {
    fn new(
        filter: impl FnOnce(Box<dyn Read + Send>, Box<dyn Write + Send>) + Send + 'static,
    ) -> Option<Self> {
        let mut pipe = [-1; 2];
        #[cfg(unix)]
        let result = unsafe { libc::pipe(pipe.as_mut_ptr()) };
        #[cfg(windows)]
        let result = unsafe { libc::pipe(pipe.as_mut_ptr(), 8192, libc::O_BINARY) };
        if result < 0 {
            return None;
        }
        let read = Fd(pipe[0]);
        let write = Fd(pipe[1]);
        let saved = unsafe { libc::dup(STDERR_FD) };
        if saved < 0 {
            return None;
        }
        let saved = Fd(saved);
        let forward = unsafe { libc::dup(saved.0) };
        if forward < 0 {
            return None;
        }
        let forward = Fd(forward);
        // Spawn before redirecting: thread creation failure must not strand stderr.
        let drain = std::thread::Builder::new()
            .name("parakit-stderr".into())
            .spawn(move || filter(Box::new(read), Box::new(forward)))
            .ok()?;
        if unsafe { libc::dup2(write.0, STDERR_FD) } < 0 {
            drop(write);
            let _ = drain.join();
            return None;
        }
        drop(write);
        Some(Self {
            saved,
            drain: Some(drain),
        })
    }
}

impl Drop for Redirect {
    fn drop(&mut self) {
        unsafe { libc::dup2(self.saved.0, STDERR_FD) };
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
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
}
