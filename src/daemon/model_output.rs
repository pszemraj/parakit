//! Filter only the pinned Parakeet loader's two unconditional informational lines.
//!
//! Quiet model reloads use this filter. Unlike the discard/NUL guard used for
//! quiet startup, it forwards every other stderr line, including concurrent
//! microphone, sound, and IPC errors.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::Mutex;

static REDIRECT: Mutex<()> = Mutex::new(());

/// Run model initialization while preserving all stderr except known loader info.
///
/// # Returns
///
/// The closure result, also when redirection is unavailable.
pub(crate) fn with_model_output_filtered<T>(f: impl FnOnce() -> T) -> T {
    let _lock = REDIRECT.lock().unwrap_or_else(|err| err.into_inner());
    let Some(_guard) = Redirect::new() else {
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
        // Windows CRT uses unsigned int, POSIX size_t. Bound before casting.
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
    fn new() -> Option<Self> {
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
        let saved = unsafe { libc::dup(2) };
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
            .name("parakit-model-stderr".into())
            .spawn(move || {
                let _ = filter_lines(read, forward);
            })
            .ok()?;
        if unsafe { libc::dup2(write.0, 2) } < 0 {
            drop(write);
            let _ = drain.join();
            return None;
        }
        Some(Self {
            saved,
            drain: Some(drain),
        })
    }
}

impl Drop for Redirect {
    fn drop(&mut self) {
        unsafe { libc::dup2(self.saved.0, 2) };
        if let Some(drain) = self.drain.take() {
            let _ = drain.join();
        }
    }
}

fn filter_lines(reader: impl Read, mut writer: impl Write) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::with_capacity(8192);
    loop {
        line.clear();
        // Bound buffering even if a diagnostic contains no newline.
        if reader.by_ref().take(8192).read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if !line.starts_with(b"parakeet: vocab=")
            && !line.starts_with(b"parakeet: BN folded into conv_dw weights for ")
        {
            writer.write_all(&line)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_preserves_errors_and_concurrent_diagnostics() {
        let input = b"parakeet: vocab=8192\nparakit: warning: microphone disconnected\nparakeet: BN folded into conv_dw weights for 24 layers\nparakeet: failed to allocate backend buffer\npartial";
        let mut output = Vec::new();
        filter_lines(&input[..], &mut output).unwrap();
        assert_eq!(output, b"parakit: warning: microphone disconnected\nparakeet: failed to allocate backend buffer\npartial");
    }

    #[test]
    fn long_unknown_lines_are_forwarded_verbatim() {
        let input = vec![b'x'; 100_000];
        let mut output = Vec::new();
        filter_lines(&input[..], &mut output).unwrap();
        assert_eq!(output, input);
    }
}
