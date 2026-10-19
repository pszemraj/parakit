//! Command output and concise activity feedback that tolerate a closed terminal.

use std::fmt;
use std::io::{ErrorKind, Write as _};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

/// A temporary activity indicator for work without a measurable percentage.
///
/// Clears itself when the operation completes or returns an error.
pub struct ProgressStage(ProgressBar);

impl ProgressStage {
    /// Show an activity indicator, or one status line when stdout is redirected.
    ///
    /// # Arguments
    ///
    /// * `message` - A short description of the operation in progress.
    /// * `quiet` - Suppress both the indicator and the status line.
    ///
    /// # Returns
    ///
    /// A guard that clears the indicator when dropped.
    ///
    /// # Panics
    ///
    /// Panics if the built-in progress template is invalid.
    pub fn new(message: impl Into<String>, quiet: bool) -> Self {
        // Model loading temporarily redirects native stderr; stdout keeps the
        // activity indicator visible while that work is in progress.
        let target = if quiet {
            ProgressDrawTarget::hidden()
        } else {
            ProgressDrawTarget::stdout()
        };
        let bar = ProgressBar::with_draw_target(None, target);
        let message = message.into();
        if !quiet && bar.is_hidden() {
            let _ = writeln!(std::io::stdout(), "{message}");
        }
        bar.set_style(
            ProgressStyle::with_template("{spinner} {msg} ({elapsed})")
                .expect("valid activity template")
                .tick_chars("-\\|/ "),
        );
        bar.set_message(message);
        if !bar.is_hidden() {
            bar.enable_steady_tick(Duration::from_millis(100));
        }
        Self(bar)
    }
}

impl Drop for ProgressStage {
    fn drop(&mut self) {
        self.0.finish_and_clear();
    }
}

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
