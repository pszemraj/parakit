//! Serialized native stderr redirection for noisy C/C++ calls.

use std::collections::VecDeque;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::IntoRawFd;
use std::panic::{self, AssertUnwindSafe};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const STDERR_FD: libc::c_int = 2;
const DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const CAPTURE_LIMIT: usize = 64 * 1024;
static REDIRECT: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct BoundedCapture {
    bytes: VecDeque<u8>,
    truncated: bool,
}

impl BoundedCapture {
    fn extend(&mut self, bytes: &[u8]) {
        let excess = self
            .bytes
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(CAPTURE_LIMIT);
        if excess > 0 {
            let remove = excess.min(self.bytes.len());
            self.bytes.drain(..remove);
            self.truncated = true;
        }
        let bytes = if bytes.len() > CAPTURE_LIMIT {
            self.truncated = true;
            &bytes[bytes.len() - CAPTURE_LIMIT..]
        } else {
            bytes
        };
        self.bytes.extend(bytes);
    }

    fn replay(&self, writer: &mut impl Write) {
        if self.truncated {
            let _ = writeln!(
                writer,
                "parakit: native stderr truncated to the last {CAPTURE_LIMIT} bytes"
            );
        }
        let (first, second) = self.bytes.as_slices();
        let _ = writer.write_all(first);
        let _ = writer.write_all(second);
    }
}

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
    let captured = Arc::new(Mutex::new(BoundedCapture::default()));
    let drain_capture = Arc::clone(&captured);
    let result = panic::catch_unwind(AssertUnwindSafe(|| {
        with_stderr_filtered(f, move |mut reader, _writer| {
            let mut buffer = [0; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => drain_capture
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .extend(&buffer[..count]),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
    }));
    match result {
        Ok(value) => value,
        Err(payload) => {
            captured
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .replay(&mut io::stderr());
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
        let (read, write) = open_pipe()?;
        let saved = duplicate_cloexec(STDERR_FD).ok()?;
        let forward = duplicate_cloexec(saved.0).ok()?;
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
        if error.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        #[cfg(target_os = "linux")]
        if error.raw_os_error() == Some(libc::EBUSY) {
            continue;
        }
        return Err(error);
    }
}

#[cfg(unix)]
fn open_pipe() -> Option<(Fd, Fd)> {
    let (read, write) = std::io::pipe().ok()?;
    Some((Fd(read.into_raw_fd()), Fd(write.into_raw_fd())))
}

#[cfg(windows)]
fn open_pipe() -> Option<(Fd, Fd)> {
    let mut pipe = [-1; 2];
    let result = unsafe { libc::pipe(pipe.as_mut_ptr(), 8192, libc::O_BINARY | libc::O_NOINHERIT) };
    (result >= 0).then(|| (Fd(pipe[0]), Fd(pipe[1])))
}

#[cfg(unix)]
fn duplicate_cloexec(fd: libc::c_int) -> io::Result<Fd> {
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(Fd(duplicate))
    }
}

#[cfg(windows)]
fn duplicate_cloexec(fd: libc::c_int) -> io::Result<Fd> {
    use windows::Win32::Foundation::{
        SetHandleInformation, HANDLE, HANDLE_FLAGS, HANDLE_FLAG_INHERIT,
    };

    let duplicate = unsafe { libc::dup(fd) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error());
    }
    let duplicate = Fd(duplicate);
    let handle = unsafe { libc::get_osfhandle(duplicate.0) };
    if handle == -1 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        SetHandleInformation(
            HANDLE(handle as *mut std::ffi::c_void),
            HANDLE_FLAG_INHERIT.0,
            HANDLE_FLAGS(0),
        )
    }
    .map_err(|error| io::Error::other(error.to_string()))?;
    Ok(duplicate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppressed_capture_keeps_only_the_bounded_tail() {
        let mut capture = BoundedCapture::default();
        capture.extend(&vec![b'a'; CAPTURE_LIMIT]);
        capture.extend(&[b'b'; 32]);

        assert_eq!(capture.bytes.len(), CAPTURE_LIMIT);
        assert!(capture.truncated);
        assert_eq!(
            capture.bytes.iter().filter(|&&byte| byte == b'b').count(),
            32
        );
        assert!(capture
            .bytes
            .iter()
            .take(CAPTURE_LIMIT - 32)
            .all(|&byte| byte == b'a'));
    }

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
