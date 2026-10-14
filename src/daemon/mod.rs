//! Daemon-only subsystems used by the `parakit` binary.

/// Captures microphone audio and reports input-device metadata.
pub(crate) mod audio;
/// Integrates global hotkeys, focus detection, and text insertion.
pub(crate) mod desktop;
/// Reopens and warms inference sessions using the startup device policy.
pub(crate) mod engine_runtime;
/// Shared user-facing hotkey diagnostics.
pub(crate) mod hotkey_help;
/// Coordinates single-instance daemon IPC.
pub(crate) mod ipc;
/// Writes transcript logs and runtime event records.
pub(crate) mod logging;
#[cfg(target_os = "macos")]
/// macOS desktop permission, focus, and diagnostic helpers.
pub(crate) mod macos;
/// Coordinates model residency and admission of recording/insertion activity.
pub(crate) mod model_lifecycle;
/// Sends user-visible desktop notifications.
pub(crate) mod notifications;
/// Checks runtime prerequisites before the daemon starts.
pub(crate) mod preflight;
/// Manages push-to-talk recording state and capture buffers.
pub(crate) mod recording;
/// Plays local feedback sounds for daemon events.
pub(crate) mod sounds;
/// Temporarily suppresses noisy native stderr around structured diagnostics.
pub(crate) mod stderr;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod subprocess;
/// Runs transcription and post-processing work off the hotkey thread.
pub(crate) mod worker;
/// Bounds IPC shutdown while allowing the worker to release native resources.
pub(crate) mod worker_shutdown;

/// Re-export commonly used desktop helpers for the daemon entrypoint.
pub(crate) use desktop::{hotkey, inject};

/// Yield an immediate polling attempt, then fixed-interval attempts until the deadline.
///
/// The first attempt runs even when the deadline has expired. Later attempts
/// check the deadline before sleeping, preserving a full interval and one final
/// probe when that sleep crosses the deadline. Callers own probe/error policy.
///
/// # Arguments
///
/// * `deadline` - Absolute end of the wait budget, including time spent probing.
/// * `interval` - Full sleep between unsuccessful attempts.
///
/// # Returns
///
/// An iterator yielding one unit value per allowed attempt.
pub(crate) fn poll_ticks(
    deadline: std::time::Instant,
    interval: std::time::Duration,
) -> impl Iterator<Item = ()> {
    std::iter::once(()).chain(std::iter::from_fn(move || {
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(interval);
        Some(())
    }))
}

#[cfg(target_os = "linux")]
/// Re-export Linux desktop/session helpers.
pub(crate) use desktop::{session, wsl};

#[cfg(all(target_os = "linux", test))]
pub(crate) use desktop::x11;

#[cfg(target_os = "windows")]
/// Encode a Windows string or path with its terminating NUL.
///
/// # Arguments
///
/// * `text` - Native Windows text, including paths with unpaired surrogates.
///
/// # Returns
///
/// The original UTF-16 code units followed by one NUL code unit.
pub(crate) fn encode_wide_null(text: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.as_ref().encode_wide().chain(Some(0)).collect()
}

#[cfg(target_os = "windows")]
/// Re-export Windows desktop integration helpers.
pub(crate) use desktop::{windows_input, windows_security};

#[cfg(test)]
mod tests {
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_path_encoding_preserves_unpaired_surrogates() {
        use std::os::windows::ffi::OsStringExt;
        let path = std::ffi::OsString::from_wide(&[0xd800]);
        assert_eq!(super::encode_wide_null(path), [0xd800, 0]);
        assert_eq!(super::encode_wide_null("a"), [97, 0]);
    }

    #[test]
    fn expired_poll_deadline_still_allows_one_attempt() {
        let mut ticks =
            super::poll_ticks(std::time::Instant::now(), std::time::Duration::from_secs(1));
        assert_eq!(ticks.next(), Some(()));
        assert_eq!(ticks.next(), None);
    }
}
