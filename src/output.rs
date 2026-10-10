//! Command output on stdout that ends quietly when its reader goes away.

use std::fmt;
use std::io::{ErrorKind, Write as _};

/// Stdout's reader closed the pipe, as in `parakit status | head -1`.
///
/// The binary treats this as a normal exit rather than a failure.
#[derive(Debug)]
pub struct StdoutClosed;

impl fmt::Display for StdoutClosed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("stdout was closed by its reader")
    }
}

impl std::error::Error for StdoutClosed {}

/// Write formatted command output to stdout.
///
/// Unlike `print!`, a closed stdout comes back as an error instead of a panic,
/// so the command can stop early.
///
/// # Arguments
///
/// * `args` - Formatted output, usually built by [`outln!`](crate::outln).
///
/// # Returns
///
/// `Ok(())` once the output was written.
///
/// # Errors
///
/// Returns [`StdoutClosed`] when the reader closed the pipe, and the
/// underlying I/O error for any other write failure.
pub fn write_stdout(args: fmt::Arguments<'_>) -> anyhow::Result<()> {
    std::io::stdout().write_fmt(args).map_err(|err| {
        if err.kind() == ErrorKind::BrokenPipe {
            anyhow::Error::new(StdoutClosed)
        } else {
            anyhow::Error::new(err).context("failed to write to stdout")
        }
    })
}

/// `println!` for command output, returning an error instead of panicking.
///
/// Expands to a [`write_stdout`](crate::output::write_stdout) call; propagate
/// its result with `?`.
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {
        $crate::output::write_stdout(format_args!("{}\n", format_args!($($arg)*)))
    };
}
